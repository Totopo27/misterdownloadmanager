//! BitTorrent and magnet downloads (D-014): librqbit, pure Rust, behind the few calls the app
//! needs. Read what a torrent holds without starting it, add it with the files and folder the
//! person chose, then watch it (stats, peers, trackers, pieces), pause, resume and remove it.
//! Torrents are named by their info hash (40 hex digits), which the app keeps in its own list.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// The library itself, for what this wrapper doesn't cover (making torrents, tests).
pub use librqbit;
/// The order a torrent's pieces are downloaded in (#39): in order (the file's start first, for
/// playing it while it downloads) or the rarest first.
pub use librqbit::PieceOrder;
use librqbit::api::{Api, TorrentIdOrHash};
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ConnectionOptions, ListenerOptions, Session,
    SessionOptions, SessionPersistenceConfig, TorrentStatsState,
};

pub mod blocklist;
mod names;
pub mod rename_plugin;
pub mod trackers;
mod watch;
pub use names::{UnsafeName, check as check_names};

/// Check that a single file or folder name (one path component, with no `/` or `\`) is safe to
/// write on this system (`names::check`). When `folder_rules` is true, characters and trailing
/// dots that [`Meta::folder_in`] would otherwise rewrite are also refused so the chosen folder
/// name matches what is created on disk.
pub fn check_single_name(name: &str, folder_rules: bool) -> Result<(), UnsafeName> {
    if folder_rules {
        if name.chars().any(|c| r#":<>"|?*"#.contains(c)) {
            return Err(UnsafeName {
                path: name.to_owned(),
                why: "a character Windows doesn't allow in a name",
            });
        }
        if name.ends_with(['.', ' ']) {
            return Err(UnsafeName {
                path: name.to_owned(),
                why: "a name ending with a dot or a space",
            });
        }
    }
    names::check([[name.to_owned()]])
}

/// Check that a relative file or folder path has no root, prefix, `.` or `..`, and passes
/// [`names::check`] for this system.
pub fn check_relative_path(path: &Path) -> Result<(), UnsafeName> {
    let raw = path.to_string_lossy();
    let has_drive_prefix = raw
        .split(['/', '\\'])
        .next()
        .is_some_and(|first| first.as_bytes().get(1) == Some(&b':'));
    if path.is_absolute()
        || raw.starts_with(['/', '\\'])
        || has_drive_prefix
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(UnsafeName {
            path: raw.into_owned(),
            why: "an empty, `.` or `..` name",
        });
    }
    let parts: Vec<String> = raw.split(['/', '\\']).map(str::to_owned).collect();
    names::check([parts])
}

#[derive(Debug, thiserror::Error)]
pub enum TorrentError {
    #[error("this is not a magnet link or a .torrent file")]
    NotATorrent,
    /// A file name this system can't write safely (`names`, #40): the torrent is refused, nothing
    /// is written.
    #[error("this torrent has a file name that isn't safe to write here: {} ({})", .0.path, .0.why)]
    UnsafeName(UnsafeName),
    #[error("no peer sent the torrent's file list in time")]
    MetadataTimeout,
    #[error("this torrent is not in the list")]
    Unknown,
    #[error("{0}")]
    Engine(String),
}

fn engine(e: impl std::fmt::Display) -> TorrentError {
    TorrentError::Engine(e.to_string())
}

/// The most a .torrent file may be (#40): the largest real ones (a great many files) are a few
/// MB; anything bigger isn't one, and would be read whole into memory.
pub const MAX_TORRENT_FILE: u64 = 10 << 20;

/// A .torrent file from disk, at most `MAX_TORRENT_FILE` (a bigger one: `InvalidData`).
pub fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_TORRENT_FILE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_TORRENT_FILE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "larger than a .torrent file can be",
        ));
    }
    Ok(bytes)
}

/// A .torrent file's bytes: a torrent, whose names are safe for this system (`names`).
fn check_bytes(torrent: &[u8]) -> Result<(), TorrentError> {
    let meta = librqbit::torrent_from_bytes(torrent).map_err(|_| TorrentError::NotATorrent)?;
    safe_names(
        meta.info
            .data
            .validate()
            .map_err(engine)?
            .iter_file_details(),
    )
}

/// A magnet link for a .torrent: its hash, its name and its trackers (`trackers::listed`).
pub fn magnet_link(torrent: &[u8]) -> Option<String> {
    let meta = librqbit::torrent_from_bytes(torrent).ok()?;
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let mut link = format!("magnet:?xt=urn:btih:{}", meta.info_hash.as_string());
    if let Some(name) = meta.info.data.validate().ok()?.name() {
        link.push_str(&format!("&dn={}", enc(&name)));
    }
    for t in trackers::listed(torrent) {
        link.push_str(&format!("&tr={}", enc(&t)));
    }
    Some(link)
}

/// Read the safe file list from a saved torrent without starting a session.
pub fn files_from_bytes(torrent: &[u8]) -> Result<Vec<FileEntry>, TorrentError> {
    check_bytes(torrent)?;
    let meta = librqbit::torrent_from_bytes(torrent).map_err(|_| TorrentError::NotATorrent)?;
    let hash = meta.info_hash.as_string();
    let info = meta.info.data.validate().map_err(engine)?;
    let mut entries: Vec<FileEntry> = info
        .iter_file_details()
        .map(|f| FileEntry {
            path: f.filename.to_pathbuf(),
            size: f.len,
            padding: f.attrs().padding,
        })
        .collect();
    rename_plugin::apply_from_memory(&hash, &mut entries);
    Ok(entries)
}

/// Before librqbit restores its saved torrents (`session.json` and a `<hash>.torrent` for each,
/// in `dir`), take out the ones the name check refuses, and those saved without their .torrent
/// (whose names it can't check): the app adds those again itself, through `add` and its own
/// copy. The rest keep only the trackers MDM may contact (`trackers`, `proxied`), in the list and
/// in their .torrent. Rewritten through a temporary file; a list that can't be read is left to
/// librqbit.
fn prepare_saved(dir: &Path, proxied: bool) {
    let db = dir.join("session.json");
    let Ok(text) = std::fs::read_to_string(&db) else {
        return;
    };
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let Some(saved) = doc.get_mut("torrents").and_then(|t| t.as_object_mut()) else {
        return;
    };
    let mut changed = false;
    saved.retain(|_, entry| {
        let Some(hash) = entry.get("info_hash").and_then(|h| h.as_str()) else {
            changed = true;
            return false;
        };
        let hash = hash.to_ascii_lowercase();
        let file = dir.join(format!("{hash}.torrent"));
        let bytes = std::fs::read(&file).ok().filter(|b| check_bytes(b).is_ok());
        let Some(bytes) = bytes else {
            for ext in ["torrent", "bitv"] {
                let _ = std::fs::remove_file(dir.join(format!("{hash}.{ext}")));
            }
            changed = true;
            return false;
        };
        if let Some(filtered) = trackers::filter_torrent(&bytes, proxied).filter(|f| *f != bytes) {
            let _ = std::fs::write(&file, filtered);
        }
        if let Some(list) = entry.get_mut("trackers").and_then(|t| t.as_array_mut()) {
            let before = list.len();
            list.retain(|t| t.as_str().is_some_and(|t| trackers::allowed(t, proxied)));
            changed |= list.len() != before;
        }
        true
    });
    if !changed {
        return;
    }
    let Ok(out) = serde_json::to_string(&doc) else {
        return;
    };
    let tmp = dir.join("session.json.mdm-tmp");
    if std::fs::write(&tmp, &out).is_ok()
        && std::fs::read_to_string(&tmp).is_ok_and(|back| back == out)
    {
        let _ = std::fs::rename(&tmp, &db);
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The torrent's file names checked for this system (`names`); padding files are never written.
fn safe_names<'a, B: AsRef<[u8]> + 'a>(
    files: impl Iterator<Item = librqbit::FileDetails<'a, B>>,
) -> Result<(), TorrentError> {
    names::check(files.filter(|f| !f.attrs().padding).map(|f| {
        f.filename
            .iter_components()
            .map(|part| part.into_owned())
            .collect::<Vec<String>>()
    }))
    .map_err(TorrentError::UnsafeName)
}

/// Where a torrent comes from.
#[derive(Clone, Debug)]
pub enum Source {
    Magnet(String),
    /// The bytes of a .torrent file.
    File(Vec<u8>),
}

impl Source {
    /// A magnet link, or nothing.
    pub fn magnet(text: &str) -> Option<Source> {
        let t = text.trim();
        t.get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case("magnet:?"))
            .then(|| Source::Magnet(t.to_owned()))
    }
}

/// One file inside a torrent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// Relative to the torrent's folder.
    pub path: PathBuf,
    pub size: u64,
    /// BEP 47 padding files: never shown, never chosen.
    pub padding: bool,
}

/// What a torrent holds, before anything is downloaded.
#[derive(Clone, Debug)]
pub struct Meta {
    pub name: String,
    pub info_hash: String,
    pub files: Vec<FileEntry>,
    pub piece_length: u32,
    /// A .torrent file for it (for a magnet: built from the metadata the peers sent), kept by the
    /// app so the torrent can be added again without asking the swarm.
    pub torrent: Vec<u8>,
    /// Peers already met while fetching the metadata.
    pub peers: Vec<SocketAddr>,
}

impl Meta {
    pub fn total(&self) -> u64 {
        self.files
            .iter()
            .filter(|f| !f.padding)
            .map(|f| f.size)
            .sum()
    }

    /// Where its files go when saved in `dir`: a torrent of several files gets its own folder
    /// (named after it), a single file goes straight in.
    pub fn folder_in(&self, dir: &Path) -> PathBuf {
        if self.files.iter().filter(|f| !f.padding).count() > 1 {
            let name: String = self
                .name
                .chars()
                .map(|c| {
                    if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                        '_'
                    } else {
                        c
                    }
                })
                .collect();
            let name = name.trim().trim_end_matches('.');
            dir.join(if name.is_empty() {
                &self.info_hash
            } else {
                name
            })
        } else {
            dir.to_owned()
        }
    }

    /// Its files (padding never counts) that already exist where it would write them in `dir`
    /// (`folder_in`). Unticked ones count too: librqbit opens every file of the torrent, and
    /// writes into an unticked one the parts of a piece it shares with a ticked one.
    pub fn existing_in(&self, dir: &Path) -> Vec<PathBuf> {
        let base = self.folder_in(dir);
        self.files
            .iter()
            .filter(|f| !f.padding)
            .map(|f| base.join(&f.path))
            .filter(|p| p.exists())
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Checking the pieces already on disk.
    Checking,
    Downloading,
    /// Everything chosen is there (it may still share).
    Finished,
    Paused,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub phase: Phase,
    /// Bytes read and verified so far while the session is initializing or rechecking.
    pub checked: Option<u64>,
    /// Bytes of the chosen files that are there and checked.
    pub done: u64,
    /// Bytes of the chosen files.
    pub total: u64,
    pub uploaded: u64,
    /// Bytes received from peers since it last started (0 while paused; it starts again at 0
    /// each time it resumes).
    pub fetched: u64,
    /// Bytes/s.
    pub down_speed: f64,
    pub up_speed: f64,
    /// Per file, bytes there (same order as `Meta::files`).
    pub file_done: Vec<u64>,
    pub peers_live: usize,
    pub peers_seen: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub addr: String,
    pub client: Option<String>,
    /// Bytes received from it so far.
    pub fetched: u64,
    pub uploaded: u64,
    /// "tcp" or "utp".
    pub kind: Option<String>,
}

/// A torrent's peers, trackers and pieces (the progress window's tabs), as last read
/// ([`Torrents::seen_swarm`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Swarm {
    pub peers: Vec<Peer>,
    pub trackers: Vec<String>,
    /// Which pieces are there (empty while it isn't checked yet).
    pub pieces: Vec<bool>,
}

/// How the session runs (#40: each of these is the person's to choose, Torrent settings).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Where librqbit keeps its resume data and DHT nodes (in our data folder, portable-aware).
    pub state_dir: PathBuf,
    /// Accept connections from peers (better speed; Windows may ask about the firewall once).
    pub listen: bool,
    /// Ask the router (UPnP) to forward the listening port.
    pub upnp: bool,
    /// The port to listen on; None: one picked at each start.
    pub port: Option<u16>,
    /// The address to listen on; None: every address of the computer (IPv4 and IPv6), as
    /// BitTorrent clients do. Tests and test tools set 127.0.0.1: a listener on every address
    /// makes Windows Firewall ask about each new program (each new test build is one).
    pub listen_ip: Option<std::net::IpAddr>,
    /// The DHT: finding peers without trackers.
    pub dht: bool,
    /// Local peer discovery (LSD): it tells the local network which torrents this computer has.
    pub lsd: bool,
    /// A `socks5://` (or `socks5h://`) proxy for peers and HTTP trackers. librqbit sends DHT,
    /// LSD, UDP trackers, UPnP and incoming connections around it, so with a proxy those are off
    /// and UDP trackers dropped (`trackers`).
    pub proxy: Option<String>,
    /// A blocklist MDM converted (`blocklist::convert`, P2P lines): peers in it are refused.
    pub blocklist: Option<PathBuf>,
    /// Tests: no trackers, local peers only, everything on loopback; no DHT unless
    /// `dht_bootstrap` names its nodes.
    pub offline: bool,
    /// The DHT's first nodes (`host:port`); None: librqbit's own (public routers). Tests and test
    /// tools name nodes on loopback.
    pub dht_bootstrap: Option<Vec<String>>,
    /// Options → Proxy / Socks → Network adapter (#35, #143): what holds every socket of the
    /// session to the adapter. UPnP and local peer discovery are off then: they speak to the
    /// local network, not through the adapter.
    pub bind: Option<Bind>,
}

/// How a session is held to the chosen network adapter (`Config::bind`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bind {
    /// Linux and macOS: its device (librqbit's `bind_device_name`: SO_BINDTODEVICE,
    /// IP_BOUND_IF), and whether it has IPv4 only.
    Device { name: String, v4_only: bool },
    /// Windows: its address (librqbit's `bind_ip`). Windows sends a packet only through the
    /// adapter that has its source address (strong host model), so a socket bound to it can't
    /// leave another way; when the address is gone, nothing is sent. One family: IPv4 when the
    /// adapter has it (IPv6 peers are then skipped), else IPv6.
    Address(std::net::IpAddr),
}

impl Bind {
    fn v4_only(&self) -> bool {
        match self {
            Bind::Device { v4_only, .. } => *v4_only,
            Bind::Address(ip) => ip.is_ipv4(),
        }
    }
}

impl Config {
    /// Everything on, as BitTorrent clients usually are; no proxy, no blocklist.
    pub fn new(state_dir: PathBuf) -> Config {
        Config {
            state_dir,
            listen: true,
            upnp: true,
            port: None,
            listen_ip: None,
            dht: true,
            lsd: true,
            proxy: None,
            blocklist: None,
            offline: false,
            dht_bootstrap: None,
            bind: None,
        }
    }
}

/// A magnet's metadata lookup as it goes (`Torrents::metadata`), shared with whoever shows the
/// wait (#92): the sources asked, the peers they gave, and how many answered.
#[derive(Default)]
pub struct Lookup {
    engine: Arc<librqbit::MetadataProgress>,
    trackers: AtomicUsize,
    dht_on: AtomicBool,
    dht: OnceLock<librqbit::dht::Dht>,
}

/// What a lookup has got so far (`Lookup::status`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LookupStatus {
    /// The trackers asked (those MDM may contact, `trackers`).
    pub trackers: usize,
    /// Whether the DHT is asked (Torrent settings, no proxy).
    pub dht: bool,
    /// The nodes the DHT knows now: few or none means it can't reach the network.
    pub dht_nodes: usize,
    /// Peer addresses each source gave (repeats included).
    pub from_trackers: usize,
    pub from_dht: usize,
    pub from_lsd: usize,
    /// Different peers asked for the file list.
    pub peers: usize,
    /// Peers that answered.
    pub connected: usize,
    /// Peers given up on.
    pub failed: usize,
}

impl Lookup {
    pub fn status(&self) -> LookupStatus {
        let e = &self.engine;
        let n = |c: &AtomicUsize| c.load(Ordering::Relaxed);
        LookupStatus {
            trackers: n(&self.trackers),
            dht: self.dht_on.load(Ordering::Relaxed),
            dht_nodes: self.dht.get().map_or(0, |d| {
                let s = d.stats();
                s.routing_table_size + s.routing_table_size_v6
            }),
            from_trackers: n(&e.from_trackers),
            from_dht: n(&e.from_dht),
            from_lsd: n(&e.from_lsd),
            peers: n(&e.peers),
            connected: n(&e.connected),
            failed: n(&e.failed),
        }
    }
}

pub struct Torrents {
    session: Arc<Session>,
    api: Api,
    /// Through a proxy: UDP trackers are dropped (`trackers::allowed`).
    proxied: bool,
    /// One add at a time: librqbit gives a new torrent the saved ids' highest plus one, so two
    /// adds at once both took id 0, and the second was answered "already managed" with the
    /// first one's handle: it was never added (two torrents started together, #39).
    adding: tokio::sync::Mutex<()>,
    /// librqbit's saved state (`session.json`, and each torrent's .torrent and piece map).
    session_dir: PathBuf,
    /// Each checking torrent's bytes checked and when they last moved: librqbit leaves a check
    /// it was asked to pause in Initializing, its count frozen, which isn't checking any more.
    /// Shared with the thread that reads every torrent (`watch`).
    checks: Arc<Checks>,
    /// Every torrent as last read, on a thread of its own (#693): what the window's thread
    /// asks ([`Torrents::seen`] and the rest that never wait).
    watch: watch::Watch,
    /// Torrents waiting for their check to end before they pause ([`Torrents::pause`]); a Start
    /// meanwhile takes the pause back ([`Torrents::resume`]).
    pause_after_check: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

pub(crate) type Handle = Arc<librqbit::ManagedTorrent>;

type Checks = std::sync::Mutex<std::collections::HashMap<String, (u64, std::time::Instant)>>;

/// A paused torrent whose checked bytes haven't moved for this long has stopped checking.
const CHECK_HALTED_AFTER: Duration = Duration::from_secs(5);

/// The lowest limit a torrent can have, bytes/s: librqbit's limiter holds one second's budget
/// and asks it for a whole 16 KiB block at once; under that, every request fails and each peer
/// is dropped, so the torrent would stall instead of going slowly.
pub const MIN_RATE: u64 = 16 * 1024;

/// A rate for librqbit's limiter: 0 and None are unlimited; at least `MIN_RATE`, and above
/// u32::MAX bytes/s is clamped.
fn rate(bps: Option<u64>) -> Option<NonZeroU32> {
    bps.filter(|b| *b > 0)
        .and_then(|b| NonZeroU32::new(b.clamp(MIN_RATE, u32::MAX as u64) as u32))
}

fn id(hash: &str) -> Result<TorrentIdOrHash, TorrentError> {
    TorrentIdOrHash::parse(hash).map_err(|_| TorrentError::Unknown)
}

/// Session state captured by [`Torrents::detach`] and restored by [`Torrents::attach`].
pub struct DetachedTorrent {
    files: Option<Vec<usize>>,
    paused: bool,
    limits: Option<(Option<u64>, Option<u64>)>,
    order: PieceOrder,
    piece_map: Option<Vec<u8>>,
}

/// The DHT's first nodes on the internet (#650): the public routers BitTorrent clients use. Any
/// one that answers is enough; they are asked together, and the rest no more once one has
/// answered. librqbit's own two alone left the DHT empty where neither answered (a name
/// resolving to an old address, a network blocking one), and magnet links without trackers
/// then found no peer. Later starts begin with the nodes saved in `dht.json`.
pub const DHT_BOOTSTRAP: &[&str] = &[
    "dht.libtorrent.org:25401",
    "dht.transmissionbt.com:6881",
    "router.bittorrent.com:6881",
    "router.utorrent.com:6881",
];

/// The DHT's first nodes: the ones a test names, else [`DHT_BOOTSTRAP`].
fn bootstrap_nodes(cfg: &Config) -> Vec<String> {
    cfg.dht_bootstrap
        .clone()
        .unwrap_or_else(|| DHT_BOOTSTRAP.iter().map(|a| (*a).to_owned()).collect())
}

/// The file in the state folder that keeps the port picked when Torrent settings name none
/// (#650): the next session listens on it again, so peers that learned it, and a router's or
/// firewall's rule for it, still reach MDM.
const PORT_FILE: &str = "listen-port";

fn saved_port(state_dir: &Path) -> Option<u16> {
    std::fs::read_to_string(state_dir.join(PORT_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|p| *p != 0)
}

/// Where the session listens, in MDM's log (#650): a listener on every address is one IPv6
/// socket that takes IPv4 too, which Windows' `netstat -p tcp` (IPv4 only) doesn't list.
/// `saved_nodes`: the DHT started from the nodes the last session saved (`dht.json`).
fn log_network(session: &Session, saved_nodes: bool) {
    match session.listen_addr() {
        Some(a) if a.ip().is_unspecified() && a.is_ipv6() => tracing::info!(
            "torrents: listening for peers on TCP port {} (every address, IPv4 and IPv6: {a})",
            a.port()
        ),
        Some(a) => tracing::info!("torrents: listening for peers on TCP {a}"),
        None => tracing::info!("torrents: not listening for peers (Torrent settings or a proxy)"),
    }
    if let Some(dht) = session.get_dht() {
        tracing::info!(
            "torrents: DHT on UDP {}, starting from {}",
            dht.listen_addr(),
            if saved_nodes {
                "the nodes the last session saved, and the bootstrap nodes"
            } else {
                "the bootstrap nodes"
            }
        );
    }
}

impl Torrents {
    pub async fn start(cfg: Config) -> Result<Torrents, TorrentError> {
        std::fs::create_dir_all(&cfg.state_dir).map_err(engine)?;
        // The single files renamed in Add torrent, for the lists read without waiting
        // (`seen_files`, `files_from_bytes`) before a torrent's storage first asks for them.
        rename_plugin::load_into_memory(&cfg.state_dir.join("session"));
        let proxied = cfg.proxy.is_some();
        let v4_only = cfg.bind.as_ref().is_some_and(Bind::v4_only);
        let bind_ip = match &cfg.bind {
            Some(Bind::Address(ip)) => Some(*ip),
            _ => None,
        };
        // Offline only without one named: a test's DHT on loopback is still on.
        let dht_on = cfg.dht && !proxied && (!cfg.offline || cfg.dht_bootstrap.is_some());
        let dht = || {
            dht_on.then(|| librqbit::DhtSessionConfig {
                bootstrap_addrs: Some(bootstrap_nodes(&cfg)),
                persistence: Some(librqbit::dht::DhtPersistenceConfig {
                    dump_interval: None,
                    config_filename: Some(cfg.state_dir.join("dht.json")),
                }),
                ..Default::default()
            })
        };
        // Offline (tests): loopback only. A set address: that one (with the port asked for).
        // Bound to an address: on it.
        let ip = cfg.listen_ip.or(bind_ip).or(cfg
            .offline
            .then_some(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)));
        let listening = cfg.listen && !proxied;
        // No port in Torrent settings: the one the last session listened on, when there was one.
        let saved = if listening && cfg.port.is_none() {
            saved_port(&cfg.state_dir)
        } else {
            None
        };
        let listen = |port: u16| ListenerOptions {
            listen_addr: match ip {
                Some(ip) => (ip, port).into(),
                None => {
                    let mut any = ListenerOptions::default().listen_addr;
                    any.set_port(port);
                    any
                }
            },
            // UPnP forwards a port on the router: not through a chosen adapter (#35), and
            // pointless (and noisy) on loopback.
            enable_upnp_port_forwarding: cfg.upnp
                && !cfg.offline
                && cfg.bind.is_none()
                && ip.is_none_or(|i| !i.is_loopback()),
            ipv4_only: cfg.offline || v4_only || ip.is_some_and(|i| i.is_ipv4()),
            ..Default::default()
        };
        // A list librqbit can't read would stop the session from starting: only a file there.
        let blocklist_url = cfg
            .blocklist
            .as_deref()
            .filter(|p| p.is_file())
            .and_then(|p| url::Url::from_file_path(p).ok())
            .map(|u| u.to_string());
        let opts = |port: u16| SessionOptions {
            dht: dht(),
            listen: listening.then(|| listen(port)),
            connect: cfg.proxy.clone().map(|proxy| ConnectionOptions {
                proxy_url: Some(proxy),
                ..Default::default()
            }),
            blocklist_url: blocklist_url.clone(),
            disable_trackers: cfg.offline,
            // Behind a proxy, UDP trackers are dropped (they would go around it): their client
            // sends from loopback, so no UDP can leave. A set listen address: that one (tests,
            // on loopback). Else librqbit's choice (every address, or loopback offline).
            udp_tracker_bind_ip: if proxied {
                Some(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST))
            } else {
                cfg.listen_ip
            },
            disable_local_service_discovery: cfg.offline
                || !cfg.lsd
                || proxied
                || cfg.bind.is_some(),
            ipv4_only: cfg.offline || v4_only,
            // Every socket of the session held to the chosen adapter (#35, #143).
            bind_device_name: match &cfg.bind {
                Some(Bind::Device { name, .. }) => Some(name.clone()),
                _ => None,
            },
            bind_ip,
            fastresume: true,
            default_storage_factory: Some(rename_plugin::storage_factory(
                cfg.state_dir.join("session"),
            )),
            persistence: Some(SessionPersistenceConfig::Json {
                folder: Some(cfg.state_dir.join("session")),
            }),
            // Major and minor only: the exact build would help someone aiming at one (#40).
            client_name_and_version: Some(format!(
                "MisterDownloadManager/{}.{}",
                env!("CARGO_PKG_VERSION_MAJOR"),
                env!("CARGO_PKG_VERSION_MINOR")
            )),
            // Its HTTP clients (trackers, UPnP) with MDM's TLS setup: on a system without CA
            // certificates librqbit's own can't be built and the session wouldn't start; with
            // it, plain HTTP trackers work and HTTPS ones fail saying why (mdm_engine::tls).
            configure_http_client: Some(Arc::new(mdm_engine::tls::configure)),
            ..Default::default()
        };
        prepare_saved(&cfg.state_dir.join("session"), proxied);
        let saved_nodes = dht_on && cfg.state_dir.join("dht.json").is_file();
        let output = cfg.state_dir.join("unsorted");
        let port = cfg.port.or(saved).unwrap_or(0);
        let session = match Session::new_with_opts(output.clone(), opts(port)).await {
            // The saved port taken (another program has it now): any other one. A port set in
            // Torrent settings is kept, and its error says why.
            Err(e) if saved.is_some() => {
                tracing::warn!(
                    "torrents: can't listen on port {port} again ({e:#}); taking another one"
                );
                Session::new_with_opts(output, opts(0)).await
            }
            started => started,
        }
        // With its causes: "error starting listeners: error binding: …" says why (an adapter's
        // address gone, #143).
        .map_err(|e| engine(format!("{e:#}")))?;
        log_network(&session, saved_nodes);
        if let (true, None, Some(addr)) = (listening, cfg.port, session.listen_addr())
            && saved != Some(addr.port())
        {
            let _ = std::fs::write(cfg.state_dir.join(PORT_FILE), addr.port().to_string());
        }
        let api = Api::new(session.clone(), None);
        let checks: Arc<Checks> = Default::default();
        // The first reading here, so the torrents restored are known from the start.
        let first = read_all(&session, &api, &checks, &HashSet::new());
        let watch = {
            let (session, api, checks) = (session.clone(), api.clone(), checks.clone());
            watch::Watch::start(first, watch::EVERY, move |wanted| {
                read_all(&session, &api, &checks, wanted)
            })
        };
        Ok(Torrents {
            session,
            api,
            proxied,
            adding: tokio::sync::Mutex::new(()),
            session_dir: cfg.state_dir.join("session"),
            checks,
            watch,
            pause_after_check: Default::default(),
        })
    }

    /// The file list, without downloading. A magnet asks the swarm, for at most `wait`; how
    /// that goes counts in `lookup`, if given.
    pub async fn metadata(
        &self,
        src: &Source,
        wait: Duration,
        lookup: Option<&Lookup>,
    ) -> Result<Meta, TorrentError> {
        let add = match src {
            // Only the trackers MDM may contact (#40, `trackers`).
            Source::Magnet(m) => {
                let m = trackers::filter_magnet(m, self.proxied);
                if let Some(l) = lookup {
                    l.trackers
                        .store(trackers::in_magnet(&m).len(), Ordering::Relaxed);
                }
                AddTorrent::from_url(m)
            }
            Source::File(b) => AddTorrent::from_bytes(
                trackers::filter_torrent(b, self.proxied).unwrap_or_else(|| b.clone()),
            ),
        };
        if let Some(l) = lookup {
            let dht = self.session.get_dht();
            l.dht_on.store(dht.is_some(), Ordering::Relaxed);
            if let Some(d) = dht {
                let _ = l.dht.set(d.clone());
            }
        }
        let opts = AddTorrentOptions {
            list_only: true,
            metadata_progress: lookup.map(|l| l.engine.clone()),
            ..Default::default()
        };
        let res = tokio::time::timeout(wait, self.session.add_torrent(add, Some(opts)))
            .await
            .map_err(|_| TorrentError::MetadataTimeout)?
            .map_err(|e| {
                let text = format!("{e:#}");
                if text.contains("decoding torrent") || text.contains("valid magnet") {
                    TorrentError::NotATorrent
                } else {
                    engine(text)
                }
            })?;
        let AddTorrentResponse::ListOnly(r) = res else {
            return Err(engine("the torrent was started instead of listed"));
        };
        // Refused before anyone picks files from it (#40).
        safe_names(r.info.iter_file_details())?;
        let files = r
            .info
            .iter_file_details()
            .map(|f| FileEntry {
                path: f.filename.to_pathbuf(),
                size: f.len,
                padding: f.attrs().padding,
            })
            .collect();
        Ok(Meta {
            name: r
                .info
                .name()
                .map(|n| n.into_owned())
                .unwrap_or_else(|| r.info_hash.as_string()),
            info_hash: r.info_hash.as_string(),
            files,
            piece_length: r.info.lengths().default_piece_length(),
            torrent: r.torrent_bytes.to_vec(),
            peers: r.seen_peers,
        })
    }

    /// Start (or keep) a torrent: `files` are indexes into `Meta::files` (None: all), each saved
    /// at `folder` + its path (see `Meta::folder_in`).
    pub async fn add(
        &self,
        torrent: &[u8],
        folder: &Path,
        files: Option<Vec<usize>>,
        peers: Vec<SocketAddr>,
        paused: bool,
    ) -> Result<String, TorrentError> {
        // Checked here too, whatever the caller (the app, the command line, a saved torrent).
        check_bytes(torrent)?;
        let wanted = librqbit::torrent_from_bytes(torrent)
            .map_err(|_| TorrentError::NotATorrent)?
            .info_hash
            .as_string();
        let torrent =
            trackers::filter_torrent(torrent, self.proxied).unwrap_or_else(|| torrent.to_vec());
        let opts = AddTorrentOptions {
            paused,
            only_files: files,
            output_folder: Some(folder.to_string_lossy().into_owned()),
            // Resuming after a restart finds our own partial files there.
            overwrite: true,
            initial_peers: (!peers.is_empty()).then_some(peers),
            ..Default::default()
        };
        let res = {
            let _one = self.adding.lock().await;
            self.session
                .add_torrent(AddTorrent::from_bytes(torrent), Some(opts))
                .await
                .map_err(engine)?
        };
        let handle = res
            .into_handle()
            .ok_or_else(|| engine("the torrent was listed instead of started"))?;
        let got = handle.info_hash().as_string();
        if got != wanted {
            return Err(engine(format!(
                "the session answered with another torrent ({got}) for {wanted}"
            )));
        }
        self.refresh(&got);
        Ok(got)
    }

    /// Whether the session still has it (resume data from an earlier run).
    pub fn has(&self, hash: &str) -> bool {
        id(hash).is_ok_and(|i| self.session.get(i).is_some())
    }

    /// Its numbers now, from librqbit: this waits while the torrent's state is held (a pause
    /// waits for the writes under way, #693), so never on the window's thread, which takes
    /// [`Self::seen`].
    pub fn stats(&self, hash: &str) -> Result<Stats, TorrentError> {
        let h = self.session.get(id(hash)?).ok_or(TorrentError::Unknown)?;
        Ok(stats_of(&h, hash, &self.checks))
    }

    /// Read it again now, after a change made here: what the window takes ([`Self::seen`])
    /// says so at once, not at the next reading. Waits as [`Self::stats`] does.
    fn refresh(&self, hash: &str) {
        let seen = id(hash)
            .ok()
            .and_then(|i| self.session.get(i))
            .map(|h| read_one(&h, hash, &self.checks));
        self.watch.put(hash, seen);
    }

    /// Its numbers as last read (None: not in the session). They are read every 200 ms on a
    /// thread of their own, and at once after each change made here (#693). Never waits on
    /// librqbit: the window's thread asks this.
    pub fn seen(&self, hash: &str) -> Option<Stats> {
        self.watch.get(hash).map(|s| s.stats.clone())
    }

    /// Its files (as in `Meta::files`), which are chosen (None: all), and the piece size, as
    /// last read. Never waits, as [`Self::seen`].
    pub fn seen_files(&self, hash: &str) -> Option<(Vec<FileEntry>, Option<Vec<usize>>, u32)> {
        let seen = self.watch.get(hash)?;
        let (mut files, piece) = files_of(seen.handle.as_ref()?)?;
        // A single file renamed in Add torrent, from memory: no disk read here.
        rename_plugin::apply_from_memory(hash, &mut files);
        Some((files, seen.chosen.clone(), piece))
    }

    /// Its peers, trackers and pieces as last read; asking keeps them read for 2 s more (the
    /// first time, they come with the next reading: empty until then). Never waits, as
    /// [`Self::seen`].
    pub fn seen_swarm(&self, hash: &str) -> Swarm {
        self.watch.want(hash);
        self.watch
            .get(hash)
            .and_then(|s| s.swarm.clone())
            .unwrap_or_default()
    }

    /// Its librqbit handle as last read: for what doesn't wait on its state (its limits, its
    /// piece order, its metadata). librqbit's own list waits while an add opens the files.
    fn handle(&self, hash: &str) -> Option<Handle> {
        self.watch.get(hash)?.handle.clone()
    }

    /// The peers it is talking to.
    pub fn peers(&self, hash: &str) -> Vec<Peer> {
        match id(hash) {
            Ok(i) => peers_of(&self.api, i),
            Err(_) => Vec::new(),
        }
    }

    /// Its files (as in `Meta::files`), which are chosen (None: all), and the piece size.
    pub fn files(&self, hash: &str) -> Option<(Vec<FileEntry>, Option<Vec<usize>>, u32)> {
        let h = self.session.get(id(hash).ok()?)?;
        let (mut files, piece) = files_of(&h)?;
        rename_plugin::apply_to_entries(&self.session_dir, hash, &mut files);
        Some((files, h.only_files(), piece))
    }

    pub fn trackers(&self, hash: &str) -> Vec<String> {
        match id(hash).ok().and_then(|i| self.session.get(i)) {
            Some(h) => trackers_of(&h),
            None => Vec::new(),
        }
    }

    /// Which pieces are there (empty while it isn't checked yet).
    pub fn pieces(&self, hash: &str) -> Vec<bool> {
        match id(hash) {
            Ok(i) => pieces_of(&self.api, i),
            Err(_) => Vec::new(),
        }
    }

    pub async fn pause(&self, hash: &str) -> Result<(), TorrentError> {
        let res = self.pause_now(hash).await;
        self.refresh(hash);
        res
    }

    async fn pause_now(&self, hash: &str) -> Result<(), TorrentError> {
        let h = self.session.get(id(hash)?).ok_or(TorrentError::Unknown)?;
        match h.stats().state {
            TorrentStatsState::Paused => Ok(()),
            // librqbit pausing a check stops it where it is and leaves the torrent Initializing
            // for good (a Recheck stuck at 17 %): the check ends first, then it is paused.
            TorrentStatsState::Initializing { .. } => {
                let session = self.session.clone();
                let (wanted, hash) = (self.pause_after_check.clone(), hash.to_owned());
                wanted.lock().unwrap().insert(hash.clone());
                tokio::spawn(async move {
                    let checked = h.wait_until_initialized().await.is_ok();
                    let still_wanted = wanted.lock().unwrap().remove(&hash);
                    if checked
                        && still_wanted
                        && !matches!(h.stats().state, TorrentStatsState::Paused)
                    {
                        let _ = session.pause(&h).await;
                    }
                });
                Ok(())
            }
            _ => self.session.pause(&h).await.map_err(engine),
        }
    }

    /// Start it again. One still checking its files (added paused, or restored at start) is
    /// waited for: it is Paused only once checked, and unpausing it before did nothing, so it
    /// stayed paused (seen on Linux, where the check took longer than the call).
    pub async fn resume(&self, hash: &str) -> Result<(), TorrentError> {
        let res = self.resume_now(hash).await;
        self.refresh(hash);
        res
    }

    async fn resume_now(&self, hash: &str) -> Result<(), TorrentError> {
        let h = self.session.get(id(hash)?).ok_or(TorrentError::Unknown)?;
        // A pause waiting for the check to end is taken back: the last click wins.
        self.pause_after_check.lock().unwrap().remove(hash);
        let st = h.stats();
        // A check librqbit stopped (paused while checking) never ends by itself: starting the
        // torrent starts the check again. One still checking is waited for, as above.
        let halted = matches!(st.state, TorrentStatsState::Initializing { paused: true })
            && check_halted(&self.checks, hash, st.progress_bytes);
        if matches!(st.state, TorrentStatsState::Initializing { .. }) && !halted {
            h.wait_until_initialized().await.map_err(engine)?;
        }
        if !halted && !matches!(h.stats().state, TorrentStatsState::Paused) {
            return Ok(());
        }
        self.session.unpause(&h).await.map_err(engine)
    }

    /// The order its pieces are downloaded in (#39). Never waits, as [`Self::seen`].
    pub fn order(&self, hash: &str) -> Option<PieceOrder> {
        Some(self.handle(hash)?.piece_order())
    }

    /// Change the order its pieces are downloaded in, running or not: the next piece each peer
    /// is asked for follows it (#39). Never waits, as [`Self::seen`].
    pub fn set_order(&self, hash: &str, order: PieceOrder) -> Result<(), TorrentError> {
        let h = self.handle(hash).ok_or(TorrentError::Unknown)?;
        h.set_piece_order(order);
        Ok(())
    }

    /// Change which files are downloaded.
    pub async fn choose(&self, hash: &str, files: &HashSet<usize>) -> Result<(), TorrentError> {
        let h = self.session.get(id(hash)?).ok_or(TorrentError::Unknown)?;
        let res = self
            .session
            .update_only_files(&h, files)
            .await
            .map_err(engine);
        self.refresh(hash);
        res
    }

    /// Take it out of the session and add it again from `torrent` (its .torrent, perhaps with
    /// trackers added): the same folder, chosen files, paused or not, own limits and piece
    /// order. With
    /// `recheck`, every piece is read and checked again (Recheck); without, the pieces it had
    /// are kept (librqbit's saved piece map, which it checks on a sample) and only the .torrent
    /// changes (Add trackers).
    pub async fn readd(
        &self,
        hash: &str,
        torrent: &[u8],
        recheck: bool,
    ) -> Result<(), TorrentError> {
        let res = self.readd_now(hash, torrent, recheck).await;
        self.refresh(hash);
        res
    }

    async fn readd_now(
        &self,
        hash: &str,
        torrent: &[u8],
        recheck: bool,
    ) -> Result<(), TorrentError> {
        check_bytes(torrent)?;
        let h = self.session.get(id(hash)?).ok_or(TorrentError::Unknown)?;
        let folder = PathBuf::from(
            self.api
                .api_torrent_details(id(hash)?)
                .map_err(engine)?
                .output_folder,
        );
        let (files, paused) = (h.only_files(), h.is_paused());
        let limits = self.torrent_limits(hash);
        let order = h.piece_order();
        drop(h);
        let map = self
            .session_dir
            .join(format!("{}.bitv", hash.to_ascii_lowercase()));
        let kept = if recheck {
            None
        } else {
            std::fs::read(&map).ok()
        };
        self.session
            .delete(id(hash)?, false)
            .await
            .map_err(engine)?;
        // librqbit removes the piece map with the torrent; on Windows a file still mapped is
        // removed only once unmapped, so it is waited for: a recheck that found it would check
        // only a sample.
        if recheck {
            for _ in 0..50 {
                if std::fs::remove_file(&map).is_ok() || !map.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        } else if let Some(bits) = kept {
            let _ = std::fs::write(&map, bits);
        }
        self.add(torrent, &folder, files, Vec::new(), paused)
            .await?;
        if let Some((down, up)) = limits {
            let _ = self.set_torrent_limits(hash, down, up);
        }
        let _ = self.set_order(hash, order);
        Ok(())
    }

    /// Ask its trackers (and the DHT) for peers again now: a running torrent is paused and
    /// started again, which announces it afresh; a paused one announces when it starts.
    pub async fn reannounce(&self, hash: &str) -> Result<(), TorrentError> {
        let h = self.session.get(id(hash)?).ok_or(TorrentError::Unknown)?;
        if !matches!(h.stats().state, TorrentStatsState::Live) {
            return Ok(());
        }
        let res = match self.session.pause(&h).await {
            Ok(()) => self.session.unpause(&h).await,
            Err(e) => Err(e),
        };
        self.refresh(hash);
        res.map_err(engine)
    }

    /// Take it out of the session; the files stay unless `delete_files`.
    pub async fn remove(&self, hash: &str, delete_files: bool) -> Result<(), TorrentError> {
        let res = self
            .session
            .delete(id(hash)?, delete_files)
            .await
            .map_err(engine);
        self.refresh(hash);
        res
    }

    /// Detach a torrent from the session before renaming its folder on disk, preserving its
    /// saved piece bitmap, chosen files, pause state, rate limits and piece order so [`Self::attach`]
    /// can restore sharing at the new folder (or back at the old folder if the rename fails).
    pub async fn detach(&self, hash: &str) -> Result<Option<DetachedTorrent>, TorrentError> {
        let tid = id(hash)?;
        let Some(h) = self.session.get(tid) else {
            return Ok(None);
        };
        let (files, paused) = (h.only_files(), h.is_paused());
        let limits = self.torrent_limits(hash);
        let order = h.piece_order();
        drop(h);
        let map = self
            .session_dir
            .join(format!("{}.bitv", hash.to_ascii_lowercase()));
        let piece_map = std::fs::read(&map).ok();
        self.session.delete(tid, false).await.map_err(engine)?;
        Ok(Some(DetachedTorrent {
            files,
            paused,
            limits,
            order,
            piece_map,
        }))
    }

    /// Re-attach a torrent detached by [`Self::detach`] at `folder`, restoring its saved piece
    /// bitmap, chosen files, pause state, rate limits and piece order.
    pub async fn attach(
        &self,
        hash: &str,
        torrent: &[u8],
        folder: &Path,
        fallback_files: Option<Vec<usize>>,
        detached: Option<DetachedTorrent>,
    ) -> Result<(), TorrentError> {
        let (files, paused, limits, order, piece_map) = match detached {
            Some(d) => (d.files, d.paused, d.limits, Some(d.order), d.piece_map),
            None => (fallback_files, false, None, None, None),
        };
        if let Some(bits) = piece_map {
            let map = self
                .session_dir
                .join(format!("{}.bitv", hash.to_ascii_lowercase()));
            let _ = std::fs::write(map, bits);
        }
        self.add(torrent, folder, files, Vec::new(), paused).await?;
        if let Some((down, up)) = limits {
            let _ = self.set_torrent_limits(hash, down, up);
        }
        if let Some(order) = order {
            let _ = self.set_order(hash, order);
        }
        Ok(())
    }

    /// Every torrent the session holds (to forget ones the app no longer lists).
    pub fn hashes(&self) -> Vec<String> {
        self.session
            .with_torrents(|all| all.map(|(_, h)| h.info_hash().as_string()).collect())
    }

    /// The whole session's download limit, bytes/s (None: unlimited): every torrent together.
    /// Setting the same rate again changes nothing (a new limiter would start a new burst), so
    /// it may be called often.
    pub fn set_download_limit(&self, bps: Option<u64>) {
        let want = rate(bps);
        if self.session.ratelimits.get_config().download_bps != want {
            self.session.ratelimits.set_download_bps(want);
        }
    }

    /// The whole session's download limit now, bytes/s (None: unlimited).
    pub fn download_limit(&self) -> Option<u64> {
        let c = self.session.ratelimits.get_config();
        c.download_bps.map(|b| u64::from(b.get()))
    }

    /// The whole session's upload limit, bytes/s (None: unlimited): every torrent together.
    /// The same rate again changes nothing, as for [`Self::set_download_limit`].
    pub fn set_upload_limit(&self, bps: Option<u64>) {
        let want = rate(bps);
        if self.session.ratelimits.get_config().upload_bps != want {
            self.session.ratelimits.set_upload_bps(want);
        }
    }

    /// The whole session's upload limit now, bytes/s (None: unlimited).
    pub fn upload_limit(&self) -> Option<u64> {
        let c = self.session.ratelimits.get_config();
        c.upload_bps.map(|b| u64::from(b.get()))
    }

    /// One torrent's own limits, bytes/s (None: unlimited), on top of the session's. They apply
    /// at once, running or paused, and last while the session runs (librqbit doesn't save them).
    /// Never waits, as [`Self::seen`].
    pub fn set_torrent_limits(
        &self,
        hash: &str,
        down: Option<u64>,
        up: Option<u64>,
    ) -> Result<(), TorrentError> {
        let h = self.handle(hash).ok_or(TorrentError::Unknown)?;
        let want = librqbit::limits::LimitsConfig {
            download_bps: rate(down),
            upload_bps: rate(up),
        };
        // The same limits again change nothing (a new limiter would start a new burst), so
        // they may be set often.
        if h.ratelimits() != want {
            h.set_ratelimits(want);
        }
        Ok(())
    }

    /// One torrent's own limits (down, up), as `set_torrent_limits` stored them. Never waits.
    pub fn torrent_limits(&self, hash: &str) -> Option<(Option<u64>, Option<u64>)> {
        let h = self.handle(hash)?;
        let c = h.ratelimits();
        let get = |v: Option<NonZeroU32>| v.map(|b| u64::from(b.get()));
        Some((get(c.download_bps), get(c.upload_bps)))
    }

    /// Where peers can reach us (None when not listening).
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.session.listen_addr()
    }

    pub async fn stop(&self) {
        self.watch.close();
        self.session.stop().await;
    }
}

/// Whether a paused torrent's check has stopped: its bytes checked haven't moved for
/// [`CHECK_HALTED_AFTER`].
fn check_halted(checks: &Checks, hash: &str, checked: u64) -> bool {
    let now = std::time::Instant::now();
    let mut checks = checks.lock().unwrap();
    let seen = checks.entry(hash.to_owned()).or_insert((checked, now));
    if seen.0 != checked {
        *seen = (checked, now);
    }
    now.duration_since(seen.1) >= CHECK_HALTED_AFTER
}

/// A torrent's numbers now: waits while librqbit holds its state (#693).
fn stats_of(h: &librqbit::ManagedTorrent, hash: &str, checks: &Checks) -> Stats {
    let s = h.stats();
    let live = s.live.as_ref();
    let mib = |m: f64| m * 1024.0 * 1024.0;
    let phase = match s.state {
        // Paused while checking: librqbit stopped the check (it stays Initializing).
        TorrentStatsState::Initializing { paused } => {
            if paused && check_halted(checks, hash, s.progress_bytes) {
                Phase::Paused
            } else {
                Phase::Checking
            }
        }
        TorrentStatsState::Paused => Phase::Paused,
        TorrentStatsState::Error => Phase::Error,
        TorrentStatsState::Live if s.finished => Phase::Finished,
        TorrentStatsState::Live => Phase::Downloading,
    };
    if !matches!(s.state, TorrentStatsState::Initializing { .. }) {
        checks.lock().unwrap().remove(hash);
    }
    Stats {
        phase,
        checked: (phase == Phase::Checking).then_some(s.progress_bytes),
        done: s.progress_bytes,
        total: s.total_bytes,
        uploaded: s.uploaded_bytes,
        fetched: live.map_or(0, |l| l.snapshot.fetched_bytes),
        down_speed: live.map_or(0.0, |l| mib(l.download_speed.mbps)),
        up_speed: live.map_or(0.0, |l| mib(l.upload_speed.mbps)),
        file_done: s.file_progress.clone(),
        peers_live: live.map_or(0, |l| l.snapshot.peer_stats.live as usize),
        peers_seen: live.map_or(0, |l| l.snapshot.peer_stats.seen as usize),
        error: s.error.clone(),
    }
}

/// A torrent's files and piece size, from its metadata (which doesn't wait on its state).
fn files_of(h: &librqbit::ManagedTorrent) -> Option<(Vec<FileEntry>, u32)> {
    h.with_metadata(|m| {
        let files = m
            .info
            .iter_file_details()
            .map(|f| FileEntry {
                path: f.filename.to_pathbuf(),
                size: f.len,
                padding: f.attrs().padding,
            })
            .collect::<Vec<_>>();
        (files, m.info.lengths().default_piece_length())
    })
    .ok()
}

fn trackers_of(h: &librqbit::ManagedTorrent) -> Vec<String> {
    let mut t: Vec<String> = h.shared().trackers.iter().map(|u| u.to_string()).collect();
    t.sort();
    t
}

fn peers_of(api: &Api, i: TorrentIdOrHash) -> Vec<Peer> {
    let Ok(snap) = api.api_peer_stats(i, Default::default()) else {
        return Vec::new();
    };
    let mut out: Vec<Peer> = snap
        .peers
        .into_iter()
        .map(|(addr, p)| Peer {
            addr,
            client: p.client_name,
            fetched: p.counters.fetched_bytes,
            uploaded: p.counters.uploaded_bytes,
            kind: p.conn_kind.map(|k| format!("{k:?}").to_lowercase()),
        })
        .collect();
    out.sort_by(|a, b| b.fetched.cmp(&a.fetched).then(a.addr.cmp(&b.addr)));
    out
}

fn pieces_of(api: &Api, i: TorrentIdOrHash) -> Vec<bool> {
    match api.api_dump_haves(i) {
        Ok((bits, len)) => bits.iter().by_vals().take(len as usize).collect(),
        Err(_) => Vec::new(),
    }
}

/// One torrent read now, for the window's [`Torrents::seen`] (its details only when wanted, by
/// [`read_all`]). Waits as [`stats_of`] does.
fn read_one(h: &Handle, hash: &str, checks: &Checks) -> watch::Seen {
    watch::Seen {
        stats: stats_of(h, hash, checks),
        chosen: h.only_files(),
        handle: Some(h.clone()),
        swarm: None,
    }
}

/// Every torrent of the session read now, by info hash; with its peers, trackers and pieces
/// when `wanted`. On the watch's thread (and once as the session starts): it waits as
/// [`stats_of`] does.
fn read_all(
    session: &Session,
    api: &Api,
    checks: &Checks,
    wanted: &HashSet<String>,
) -> std::collections::HashMap<String, Arc<watch::Seen>> {
    let all: Vec<(String, Handle)> = session.with_torrents(|all| {
        all.map(|(_, h)| (h.info_hash().as_string(), h.clone()))
            .collect()
    });
    all.into_iter()
        .map(|(hash, h)| {
            let mut seen = read_one(&h, &hash, checks);
            if wanted.contains(&hash) {
                let i = TorrentIdOrHash::Hash(h.info_hash());
                seen.swarm = Some(Swarm {
                    peers: peers_of(api, i),
                    trackers: trackers_of(&h),
                    pieces: pieces_of(api, i),
                });
            }
            (hash, Arc::new(seen))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where packets and connections to it came from: a UDP socket (`udp`) or a TCP listener
    /// that answers each connection with `reply`, on 127.0.0.1.
    struct Recorder {
        addr: SocketAddr,
        from: Arc<std::sync::Mutex<Vec<SocketAddr>>>,
    }

    impl Recorder {
        async fn udp() -> Recorder {
            let sock = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let from = Arc::new(std::sync::Mutex::new(Vec::new()));
            let (addr, seen) = (sock.local_addr().unwrap(), from.clone());
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                loop {
                    if let Ok((_, f)) = sock.recv_from(&mut buf).await {
                        seen.lock().unwrap().push(f);
                    }
                }
            });
            Recorder { addr, from }
        }

        // Its tests bind 127.0.0.2, which macOS hasn't.
        #[cfg(not(target_os = "macos"))]
        async fn tcp(reply: &'static [u8]) -> Recorder {
            let l = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let from = Arc::new(std::sync::Mutex::new(Vec::new()));
            let (addr, seen) = (l.local_addr().unwrap(), from.clone());
            tokio::spawn(async move {
                while let Ok((mut s, f)) = l.accept().await {
                    seen.lock().unwrap().push(f);
                    tokio::spawn(async move {
                        use tokio::io::{AsyncReadExt, AsyncWriteExt};
                        let mut buf = [0u8; 4096];
                        let _ = s.read(&mut buf).await;
                        let _ = s.write_all(reply).await;
                    });
                }
            });
            Recorder { addr, from }
        }

        fn seen(&self) -> Vec<SocketAddr> {
            self.from.lock().unwrap().clone()
        }
    }

    /// #143: a session bound to an address (Windows' way of holding it to a network adapter)
    /// sends everything from that address: its listener, outgoing peer connections, the DHT, UDP
    /// and HTTP trackers. 127.0.0.2 stands for the adapter's address: Windows and Linux route
    /// all of 127.0.0.0/8 to loopback (macOS only 127.0.0.1, so not there).
    #[cfg(not(target_os = "macos"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_bound_session_sends_only_from_its_address() {
        let bound: std::net::IpAddr = Ipv4Addr::new(127, 0, 0, 2).into();
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed").join("set");
        std::fs::create_dir_all(&src).unwrap();
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 241) as u8).collect();
        std::fs::write(src.join("a.bin"), &data).unwrap();
        let torrent = make_torrent(&src).await;
        let seeder = Torrents::start(offline(&tmp.path().join("s1")))
            .await
            .unwrap();
        let hash = seeder
            .add(&torrent, &src, None, Vec::new(), false)
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while seeder.stats(&hash).unwrap().phase != Phase::Finished {
            assert!(std::time::Instant::now() < deadline, "seeder never checked");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let seed_addr = seeder.listen_addr().expect("seeder listens");

        let dht = Recorder::udp().await;
        let udp_tracker = Recorder::udp().await;
        let http_tracker = Recorder::tcp(
            b"HTTP/1.1 200 OK\r\nContent-Length: 27\r\n\r\nd8:intervali1800e5:peers0:e",
        )
        .await;
        let other_peer = Recorder::tcp(b"").await;
        let leecher = Torrents::start(Config {
            upnp: false,
            lsd: false,
            dht: true,
            dht_bootstrap: Some(vec![dht.addr.to_string()]),
            bind: Some(Bind::Address(bound)),
            ..Config::new(tmp.path().join("s2"))
        })
        .await
        .unwrap();
        assert_eq!(
            leecher.listen_addr().map(|a| a.ip()),
            Some(bound),
            "it listens on its address only"
        );
        // Loopback trackers straight to the session: MDM's own `add` drops them (#40).
        let out = tmp.path().join("got");
        let added = leecher
            .session
            .add_torrent(
                AddTorrent::from_bytes(torrent.clone()),
                Some(AddTorrentOptions {
                    output_folder: Some(out.to_string_lossy().into_owned()),
                    overwrite: true,
                    initial_peers: Some(vec![seed_addr, other_peer.addr]),
                    trackers: Some(vec![
                        format!("udp://{}/announce", udp_tracker.addr),
                        format!("http://{}/announce", http_tracker.addr),
                    ]),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let h2 = added.into_handle().unwrap().info_hash().as_string();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let done = leecher.stats(&h2).unwrap().phase == Phase::Finished;
            let all = [&dht, &udp_tracker, &http_tracker, &other_peer]
                .iter()
                .all(|r| !r.seen().is_empty());
            if done && all {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "done {done}; dht {:?} udp {:?} http {:?} peer {:?}",
                dht.seen(),
                udp_tracker.seen(),
                http_tracker.seen(),
                other_peer.seen()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // One file: librqbit writes it straight into the folder.
        let file = [out.join("a.bin"), out.join("set").join("a.bin")]
            .into_iter()
            .find(|p| p.exists())
            .expect("the file was written");
        assert_eq!(std::fs::read(file).unwrap(), data);
        for (what, r) in [
            ("the DHT", &dht),
            ("the UDP tracker", &udp_tracker),
            ("the HTTP tracker", &http_tracker),
            ("a peer", &other_peer),
        ] {
            let seen = r.seen();
            assert!(
                seen.iter().all(|f| f.ip() == bound),
                "{what} was reached from {seen:?}, not only from {bound}"
            );
            eprintln!("#143: {what} reached from {seen:?}");
        }
        seeder.stop().await;
        leecher.stop().await;
    }

    /// #143: through a SOCKS5 proxy (Torrent settings), the proxy is reached from the bound
    /// address too, for peers and HTTP trackers alike.
    #[cfg(not(target_os = "macos"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_bound_session_reaches_its_proxy_from_its_address() {
        let bound: std::net::IpAddr = Ipv4Addr::new(127, 0, 0, 2).into();
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed").join("set");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.bin"), vec![1u8; 20_000]).unwrap();
        let torrent = make_torrent(&src).await;
        // It answers nothing: what matters is where the connections come from.
        let proxy = Recorder::tcp(b"").await;
        let t = Torrents::start(Config {
            upnp: false,
            lsd: false,
            dht: false,
            proxy: Some(format!("socks5://{}", proxy.addr)),
            bind: Some(Bind::Address(bound)),
            ..Config::new(tmp.path().join("s"))
        })
        .await
        .unwrap();
        t.session
            .add_torrent(
                AddTorrent::from_bytes(torrent),
                Some(AddTorrentOptions {
                    output_folder: Some(tmp.path().join("got").to_string_lossy().into_owned()),
                    overwrite: true,
                    initial_peers: Some(vec![SocketAddr::from((Ipv4Addr::new(10, 9, 8, 7), 6881))]),
                    trackers: Some(vec!["http://192.0.2.9/announce".into()]),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while proxy.seen().len() < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "the proxy saw {:?}",
                proxy.seen()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let seen = proxy.seen();
        assert!(seen.iter().all(|f| f.ip() == bound), "{seen:?}");
        eprintln!("#143: the proxy reached from {seen:?}");
        t.stop().await;
    }

    /// #143: bound to an address no adapter has (the adapter disconnected): the session doesn't
    /// start, says why, and nothing was sent; without a listener and a DHT as well.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_gone_address_sends_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        // TEST-NET-1 (RFC 5737): no adapter has it.
        let gone: std::net::IpAddr = Ipv4Addr::new(192, 0, 2, 1).into();
        let dht = Recorder::udp().await;
        for (n, (listen, dht_on)) in [(true, true), (false, false)].into_iter().enumerate() {
            let started = Torrents::start(Config {
                upnp: false,
                lsd: false,
                listen,
                dht: dht_on,
                dht_bootstrap: Some(vec![dht.addr.to_string()]),
                bind: Some(Bind::Address(gone)),
                ..Config::new(tmp.path().join(format!("s{n}")))
            })
            .await;
            let Err(TorrentError::Engine(why)) = started else {
                panic!("started bound to an address that isn't there");
            };
            assert!(why.contains("bind"), "says why: {why}");
            eprintln!("#143: listen {listen}, DHT {dht_on}: {why}");
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(dht.seen().is_empty(), "nothing was sent");
    }

    #[test]
    fn magnets_are_recognised() {
        assert!(matches!(
            Source::magnet("  MAGNET:?xt=urn:btih:abc"),
            Some(Source::Magnet(_))
        ));
        assert!(Source::magnet("https://x/y.torrent").is_none());
        assert!(Source::magnet("magnet").is_none());
    }

    /// A torrent of `dir`'s files, made by librqbit.
    async fn make_torrent(dir: &Path) -> Vec<u8> {
        let spawner = librqbit::spawn_utils::BlockingSpawner::new(1);
        let made = librqbit::create_torrent(
            dir,
            librqbit::CreateTorrentOptions {
                name: Some("set"),
                trackers: Vec::new(),
                piece_length: Some(16 * 1024),
            },
            &spawner,
        )
        .await
        .unwrap();
        made.as_bytes().unwrap().to_vec()
    }

    fn offline(state: &Path) -> Config {
        Config {
            dht: false,
            offline: true,
            ..Config::new(state.to_owned())
        }
    }

    /// A .torrent with one 5-byte file at `folder/<name>`, written by hand (librqbit makes only
    /// valid ones).
    fn torrent_naming(name: &str) -> Vec<u8> {
        let mut t = b"d4:infod5:filesld6:lengthi5e4:pathl6:folder".to_vec();
        t.extend(format!("{}:{name}", name.len()).bytes());
        t.extend(b"eee4:name4:test12:piece lengthi16384e6:pieces20:");
        t.extend([0u8; 20]);
        t.extend(b"ee");
        t
    }

    /// #40: a name the system can't write safely refuses the torrent, listed or added, and
    /// nothing is written.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsafe_names_refuse_the_torrent() {
        let tmp = tempfile::tempdir().unwrap();
        let t = Torrents::start(offline(&tmp.path().join("state")))
            .await
            .unwrap();
        let out = tmp.path().join("out");
        let bad = if cfg!(windows) {
            ["Q:mdm-test.bin", "x.bin:hidden", "NUL.txt"].as_slice()
        } else {
            ["tab\there.bin"].as_slice()
        };
        for name in bad {
            let bytes = torrent_naming(name);
            let listed = t
                .metadata(&Source::File(bytes.clone()), Duration::from_secs(5), None)
                .await;
            assert!(
                matches!(listed, Err(TorrentError::UnsafeName(ref u)) if u.path == format!("folder/{name}")),
                "{name}: {listed:?}"
            );
            let added = t.add(&bytes, &out, None, Vec::new(), false).await;
            assert!(
                matches!(added, Err(TorrentError::UnsafeName(_))),
                "{name}: {added:?}"
            );
        }
        assert!(!out.exists(), "nothing was written");
        // A plain name still lists.
        let fine = t
            .metadata(
                &Source::File(torrent_naming("fine.bin")),
                Duration::from_secs(5),
                None,
            )
            .await
            .unwrap();
        assert_eq!(fine.files.len(), 1);
        t.stop().await;
    }

    /// #40: the torrents librqbit saved go through the name check when the session starts again;
    /// one saved without its .torrent is taken out too (the app adds it again from its own copy).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn saved_torrents_are_checked_at_start() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let out = tmp.path().join("out");
        let t = Torrents::start(offline(&state)).await.unwrap();
        let good = t
            .add(&torrent_naming("fine.bin"), &out, None, Vec::new(), true)
            .await
            .unwrap();
        t.stop().await;

        // Beside it, as librqbit saves them: an unsafe torrent, and one without its .torrent.
        let dir = state.join("session");
        let db = dir.join("session.json");
        let mut doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&db).unwrap()).unwrap();
        let saved = doc["torrents"].as_object_mut().unwrap();
        let entry = saved.values().next().unwrap().clone();
        assert_eq!(entry["info_hash"], good.as_str(), "librqbit's format");
        // Names librqbit itself would write (inside `out`), so the check is what refuses them.
        let bad = torrent_naming(if cfg!(windows) {
            "x.bin:hidden"
        } else {
            "tab\there.bin"
        });
        let bad_hash = librqbit::torrent_from_bytes(&bad)
            .unwrap()
            .info_hash
            .as_string();
        std::fs::write(dir.join(format!("{bad_hash}.torrent")), &bad).unwrap();
        let lost = "ab".repeat(20);
        for (n, hash) in [(1000, &bad_hash), (1001, &lost)] {
            let mut e = entry.clone();
            e["info_hash"] = hash.as_str().into();
            saved.insert(n.to_string(), e);
        }
        std::fs::write(&db, serde_json::to_string(&doc).unwrap()).unwrap();

        // A magnet can't resolve offline: without the check this start would wait for ever.
        let t = tokio::time::timeout(Duration::from_secs(30), Torrents::start(offline(&state)))
            .await
            .expect("the session started")
            .unwrap();
        assert!(t.has(&good), "the safe one is kept");
        assert!(!t.has(&bad_hash), "the unsafe one is taken out");
        assert!(!t.has(&lost), "the one without its .torrent is taken out");
        assert!(!dir.join(format!("{bad_hash}.torrent")).exists());
        t.stop().await;
    }

    /// #40: a kept torrent keeps only the trackers MDM may contact, in session.json and in its
    /// .torrent (whose info, so its hash, stays the same); with a proxy, UDP ones go too.
    /// #40: a fixed port is the one listened on; with a proxy nothing listens (incoming
    /// connections would go around it). Nothing else of the network is on (no DHT, UPnP, LSD).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_port_and_the_proxy_are_honoured() {
        let tmp = tempfile::tempdir().unwrap();
        let free = std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        // Not offline (its trackers' and listener's own settings are what is tested), but on
        // loopback: a listener on every address makes Windows Firewall ask about each new
        // test build.
        let quiet = |dir: &str| Config {
            upnp: false,
            dht: false,
            lsd: false,
            listen_ip: Some(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)),
            ..Config::new(tmp.path().join(dir))
        };
        let t = Torrents::start(Config {
            port: Some(free),
            ..quiet("fixed")
        })
        .await
        .unwrap();
        assert_eq!(t.listen_addr().map(|a| a.port()), Some(free));
        assert!(
            t.listen_addr().is_some_and(|a| a.ip().is_loopback()),
            "on loopback: {:?}",
            t.listen_addr()
        );
        t.stop().await;
        let t = Torrents::start(Config {
            proxy: Some("socks5://127.0.0.1:9".into()),
            ..quiet("proxied")
        })
        .await
        .unwrap();
        assert_eq!(t.listen_addr(), None, "nothing listens behind a proxy");
        t.stop().await;
    }

    /// #650: with no port in Torrent settings, the one picked is kept: the next session
    /// listens on it again. When another program has it by then, another one is taken, and kept
    /// from then on. A port set in Torrent settings isn't saved.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_picked_port_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        // On loopback: a listener on every address makes Windows Firewall ask about each new
        // test build.
        let cfg = |dir: &str| Config {
            upnp: false,
            dht: false,
            lsd: false,
            listen_ip: Some(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)),
            ..Config::new(tmp.path().join(dir))
        };
        let port = |t: &Torrents| t.listen_addr().expect("listens").port();
        let saved = || saved_port(&tmp.path().join("auto"));

        let t = Torrents::start(cfg("auto")).await.unwrap();
        let first = port(&t);
        assert_eq!(saved(), Some(first));
        t.stop().await;
        let t = Torrents::start(cfg("auto")).await.unwrap();
        assert_eq!(port(&t), first, "the same port again");
        t.stop().await;

        let other = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, first)).unwrap();
        let t = Torrents::start(cfg("auto")).await.unwrap();
        let second = port(&t);
        assert_ne!(second, first, "another program has it");
        assert_eq!(saved(), Some(second));
        t.stop().await;
        drop(other);

        let free = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let t = Torrents::start(Config {
            port: Some(free),
            ..cfg("fixed")
        })
        .await
        .unwrap();
        assert_eq!(port(&t), free);
        assert_eq!(saved_port(&tmp.path().join("fixed")), None);
        t.stop().await;
    }

    /// #650: the DHT starts from several public routers, not librqbit's two alone; a test's
    /// own nodes replace them.
    #[test]
    fn the_dht_has_fallback_routers() {
        let cfg = Config::new(PathBuf::from("state"));
        let nodes = bootstrap_nodes(&cfg);
        for router in [
            "dht.libtorrent.org:25401",
            "dht.transmissionbt.com:6881",
            "router.bittorrent.com:6881",
            "router.utorrent.com:6881",
        ] {
            assert!(nodes.iter().any(|n| n == router), "{router} in {nodes:?}");
        }
        let own = Config {
            dht_bootstrap: Some(vec!["127.0.0.1:6881".into()]),
            ..cfg
        };
        assert_eq!(bootstrap_nodes(&own), ["127.0.0.1:6881"]);
    }

    #[test]
    fn torrent_files_are_read_up_to_their_cap() {
        let dir = tempfile::tempdir().unwrap();
        let small = dir.path().join("small.torrent");
        std::fs::write(&small, torrent_naming("a.bin")).unwrap();
        assert_eq!(read_file(&small).unwrap(), torrent_naming("a.bin"));
        let big = dir.path().join("big.torrent");
        std::fs::write(&big, vec![b'x'; MAX_TORRENT_FILE as usize + 1]).unwrap();
        assert_eq!(
            read_file(&big).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert!(read_file(&dir.path().join("none.torrent")).is_err());
    }

    #[test]
    fn kept_torrents_lose_the_trackers_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let plain = torrent_naming("fine.bin");
        let hash = librqbit::torrent_from_bytes(&plain)
            .unwrap()
            .info_hash
            .as_string();
        let local = "http://127.0.0.1:9/announce";
        let mut with_tracker = format!("d8:announce{}:{local}", local.len()).into_bytes();
        with_tracker.extend(&plain[1..]);
        assert_eq!(
            librqbit::torrent_from_bytes(&with_tracker)
                .unwrap()
                .info_hash
                .as_string(),
            hash
        );
        std::fs::write(dir.path().join(format!("{hash}.torrent")), &with_tracker).unwrap();
        let doc = serde_json::json!({ "torrents": { "0": {
            "info_hash": hash,
            "trackers": ["udp://t.example:1337/a", local, "https://t.example/a"],
            "output_folder": "x", "only_files": null, "is_paused": true,
        }}});
        std::fs::write(dir.path().join("session.json"), doc.to_string()).unwrap();
        prepare_saved(dir.path(), true);
        let back: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("session.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            back["torrents"]["0"]["trackers"],
            serde_json::json!(["https://t.example/a"])
        );
        let rewritten = std::fs::read(dir.path().join(format!("{hash}.torrent"))).unwrap();
        assert_eq!(
            rewritten, plain,
            "the announce key is gone, the rest byte for byte"
        );
        assert!(check_bytes(&rewritten).is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn seed_to_a_second_session_with_only_the_chosen_file() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed").join("set");
        std::fs::create_dir_all(&src).unwrap();
        let a: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let b: Vec<u8> = (0..120_000u32).map(|i| (i % 13) as u8).collect();
        std::fs::write(src.join("a.bin"), &a).unwrap();
        std::fs::write(src.join("b.bin"), &b).unwrap();
        let torrent = make_torrent(&src).await;

        // The seeder: its files are already there.
        let seeder = Torrents::start(offline(&tmp.path().join("s1")))
            .await
            .unwrap();
        let meta = seeder
            .metadata(
                &Source::File(torrent.clone()),
                Duration::from_secs(10),
                None,
            )
            .await
            .unwrap();
        assert_eq!(meta.name, "set");
        assert_eq!(meta.total(), 420_000);
        let names: Vec<String> = meta
            .files
            .iter()
            .map(|f| f.path.to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n.ends_with("a.bin")), "{names:?}");
        let hash = seeder
            .add(&torrent, &src, None, Vec::new(), false)
            .await
            .unwrap();
        assert_eq!(hash, meta.info_hash);
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while seeder.stats(&hash).unwrap().phase != Phase::Finished {
            assert!(std::time::Instant::now() < deadline, "seeder never checked");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(seeder.pieces(&hash).iter().all(|p| *p));
        let addr = seeder.listen_addr().expect("seeder listens");

        // The leecher wants only b.bin.
        let only_b = meta
            .files
            .iter()
            .position(|f| f.path.ends_with("b.bin"))
            .unwrap();
        let leecher = Torrents::start(offline(&tmp.path().join("s2")))
            .await
            .unwrap();
        let out = meta.folder_in(&tmp.path().join("got"));
        assert!(out.ends_with("set"), "several files: their own folder");
        let h2 = leecher
            .add(&torrent, &out, Some(vec![only_b]), vec![addr], false)
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let s = leecher.stats(&h2).unwrap();
            if s.phase == Phase::Finished {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "no transfer: {s:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let got = std::fs::read(out.join(&meta.files[only_b].path)).unwrap();
        assert_eq!(got, b);
        assert!(
            leecher.trackers(&h2).is_empty(),
            "a torrent without trackers"
        );

        leecher.pause(&h2).await.unwrap();
        assert_eq!(leecher.stats(&h2).unwrap().phase, Phase::Paused);
        leecher.remove(&h2, false).await.unwrap();
        assert!(!leecher.has(&h2));
        assert!(
            out.join(&meta.files[only_b].path).exists(),
            "files stay unless asked"
        );
        seeder.stop().await;
        leecher.stop().await;
    }

    /// The app's Recheck of a finished torrent that left the session: added again paused, it
    /// checks every piece and ends Paused and whole. Also after a restart of the session.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn recheck_of_a_released_torrent_checks_it_all() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("big.bin"), vec![9u8; 512 * 1024 * 1024]).unwrap();
        let torrent = make_torrent(&src).await;
        let state = tmp.path().join("session");
        let wait = |t: &Torrents, hash: &str, what: &str| {
            let deadline = std::time::Instant::now() + Duration::from_secs(60);
            let mut last = None;
            loop {
                let st = t.stats(hash).unwrap();
                if st.phase == Phase::Paused && st.done == st.total {
                    return;
                }
                if last.as_ref() != Some(&(st.phase, st.done)) {
                    eprintln!("{what}: {:?} {} / {}", st.phase, st.done, st.total);
                    last = Some((st.phase, st.done));
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "{what}: stuck at {st:?}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        let s1 = Torrents::start(offline(&state)).await.unwrap();
        let hash = s1
            .add(&torrent, &src, None, Vec::new(), true)
            .await
            .unwrap();
        wait(&s1, &hash, "first check");
        // Released when finished, then Recheck adds it again paused.
        s1.remove(&hash, false).await.unwrap();
        s1.add(&torrent, &src, None, Vec::new(), true)
            .await
            .unwrap();
        wait(&s1, &hash, "recheck after release");
        // Recheck while it is in the session (readd).
        s1.readd(&hash, &torrent, true).await.unwrap();
        wait(&s1, &hash, "recheck in session");
        s1.stop().await;
        // A new session restores it (session.json); the app removes it, then Recheck adds it.
        let s2 = Torrents::start(offline(&state)).await.unwrap();
        for h in s2.hashes() {
            s2.remove(&h, false).await.unwrap();
        }
        s2.add(&torrent, &src, None, Vec::new(), true)
            .await
            .unwrap();
        wait(&s2, &hash, "recheck after restart");
        s2.stop().await;
    }

    /// Paused while it checks its files, a torrent finishes the check and is then paused:
    /// librqbit's own pause stopped the check and left it Initializing for good, which the app
    /// showed as "Checking 17%" forever.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pausing_during_the_check_lets_it_finish() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("big.bin"), vec![7u8; 256 * 1024 * 1024]).unwrap();
        let torrent = make_torrent(&src).await;
        let session = Torrents::start(offline(&tmp.path().join("session")))
            .await
            .unwrap();
        let hash = session
            .add(&torrent, &src, None, Vec::new(), false)
            .await
            .unwrap();
        session.pause(&hash).await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let st = session.stats(&hash).unwrap();
            if st.phase == Phase::Paused && st.done == st.total {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the check never finished: {st:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        session.stop().await;
    }

    /// A Start clicked during the check takes back the pause clicked before it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn starting_during_the_check_takes_the_pause_back() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("big.bin"), vec![5u8; 256 * 1024 * 1024]).unwrap();
        let torrent = make_torrent(&src).await;
        let session = Torrents::start(offline(&tmp.path().join("session")))
            .await
            .unwrap();
        let hash = session
            .add(&torrent, &src, None, Vec::new(), false)
            .await
            .unwrap();
        session.pause(&hash).await.unwrap();
        session.resume(&hash).await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let st = session.stats(&hash).unwrap();
            if st.phase == Phase::Finished {
                break;
            }
            assert!(st.phase != Phase::Paused, "the Start was lost: {st:?}");
            assert!(
                std::time::Instant::now() < deadline,
                "never finished: {st:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // And it stays started.
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(session.stats(&hash).unwrap().phase, Phase::Finished);
        session.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn removing_finished_torrent_releases_file_while_session_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("data.bin"), vec![42u8; 64 * 1024]).unwrap();
        let torrent = make_torrent(&src).await;
        let session = Torrents::start(offline(&tmp.path().join("session")))
            .await
            .unwrap();
        let hash = session
            .add(&torrent, &src, None, Vec::new(), false)
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while session.stats(&hash).unwrap().phase != Phase::Finished {
            assert!(
                std::time::Instant::now() < deadline,
                "torrent never finished"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        session.remove(&hash, false).await.unwrap();
        let original = src.join("data.bin");
        let moved = src.join("released.bin");
        std::fs::rename(&original, &moved).unwrap();
        assert!(moved.is_file());
        assert!(session.listen_addr().is_some(), "session remains running");
        session.stop().await;
    }

    /// #693: what the window reads (`seen` and the rest) follows the session without asking
    /// librqbit: known from the add on, moving by itself, and changed at once by a pause or a
    /// removal made here.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_window_s_numbers_follow_the_session() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.bin"), vec![7u8; 96 * 1024]).unwrap();
        std::fs::write(src.join("b.bin"), vec![9u8; 32 * 1024]).unwrap();
        let torrent = make_torrent(&src).await;
        let session = Torrents::start(offline(&tmp.path().join("session")))
            .await
            .unwrap();
        let hash = session
            .add(&torrent, &src, None, Vec::new(), false)
            .await
            .unwrap();
        assert!(session.seen(&hash).is_some(), "known as soon as added");
        assert!(
            session.seen(&hash.to_ascii_uppercase()).is_some(),
            "by its hash in either case"
        );
        assert!(
            session
                .set_torrent_limits(&hash, Some(1 << 20), None)
                .is_ok(),
            "its limits can be set at once"
        );
        assert_eq!(session.torrent_limits(&hash), Some((Some(1 << 20), None)));
        assert_eq!(session.seen_files(&hash), session.files(&hash));
        // Checked and finished by the watch's own readings.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while session.seen(&hash).unwrap().phase != Phase::Finished {
            assert!(std::time::Instant::now() < deadline, "never seen finished");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let st = session.seen(&hash).unwrap();
        assert_eq!((st.done, st.total), (128 * 1024, 128 * 1024));
        // Per file, in the torrent's order (the folder's listing order, which differs by system).
        let sizes: Vec<u64> = session
            .seen_files(&hash)
            .unwrap()
            .0
            .iter()
            .map(|f| f.size)
            .collect();
        assert_eq!(st.file_done, sizes);
        // Its details come with the next reading once asked for.
        session.seen_swarm(&hash);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let swarm = loop {
            let s = session.seen_swarm(&hash);
            if !s.pieces.is_empty() {
                break s;
            }
            assert!(std::time::Instant::now() < deadline, "details never read");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(swarm.pieces, session.pieces(&hash));
        assert_eq!(swarm.trackers, session.trackers(&hash));
        // A pause and a removal show at once, not at the next reading.
        session.pause(&hash).await.unwrap();
        assert_eq!(session.seen(&hash).unwrap().phase, Phase::Paused);
        session.remove(&hash, false).await.unwrap();
        assert!(session.seen(&hash).is_none());
        tokio::time::sleep(watch::EVERY * 3).await;
        assert!(session.seen(&hash).is_none(), "still gone after readings");
        session.stop().await;
    }

    /// #39: a torrent's piece order is librqbit's in-order one until set; set, it is kept when
    /// the torrent is added again (Add trackers, Recheck).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_piece_order_is_set_and_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let t = Torrents::start(offline(&tmp.path().join("state")))
            .await
            .unwrap();
        let bytes = torrent_naming("fine.bin");
        let hash = t
            .add(&bytes, &tmp.path().join("out"), None, Vec::new(), true)
            .await
            .unwrap();
        assert_eq!(t.order(&hash), Some(PieceOrder::default()));
        t.set_order(&hash, PieceOrder::RarestFirst).unwrap();
        assert_eq!(t.order(&hash), Some(PieceOrder::RarestFirst));
        t.readd(&hash, &bytes, false).await.unwrap();
        assert_eq!(t.order(&hash), Some(PieceOrder::RarestFirst), "kept");
        assert!(
            t.set_order(&"0".repeat(40), PieceOrder::RarestFirst)
                .is_err()
        );
        t.stop().await;
    }

    /// #39, more torrent actions: adding again with trackers added keeps the pieces and the
    /// torrent's state; Recheck reads every piece again (a damaged one is found); a paused
    /// torrent stays paused; Reannounce leaves it as it was; the magnet link has its trackers.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn readd_keeps_or_rechecks_the_pieces() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed").join("set");
        std::fs::create_dir_all(&src).unwrap();
        let a: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let b: Vec<u8> = (0..120_000u32).map(|i| (i % 13) as u8).collect();
        std::fs::write(src.join("a.bin"), &a).unwrap();
        std::fs::write(src.join("b.bin"), &b).unwrap();
        let torrent = make_torrent(&src).await;
        let t = Torrents::start(offline(&tmp.path().join("s")))
            .await
            .unwrap();
        let hash = t
            .add(&torrent, &src, None, Vec::new(), false)
            .await
            .unwrap();
        let settled = |want: Phase| {
            let t = &t;
            let hash = hash.clone();
            async move {
                let deadline = std::time::Instant::now() + Duration::from_secs(30);
                loop {
                    let s = t.stats(&hash).unwrap();
                    if s.phase == want {
                        return s;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "never {want:?}: {s:?}"
                    );
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        };
        assert_eq!(settled(Phase::Finished).await.done, 420_000);
        t.set_torrent_limits(&hash, None, Some(64 * 1024)).unwrap();

        // Trackers added: the same torrent, finished, its own limit kept, the trackers listed.
        let tracker = "http://tracker.example/announce".to_owned();
        let more = trackers::add(&torrent, std::slice::from_ref(&tracker)).unwrap();
        t.readd(&hash, &more, false).await.unwrap();
        assert_eq!(settled(Phase::Finished).await.done, 420_000);
        assert_eq!(t.trackers(&hash), std::slice::from_ref(&tracker));
        assert_eq!(t.torrent_limits(&hash), Some((None, Some(64 * 1024))));
        t.reannounce(&hash).await.unwrap();
        assert_eq!(settled(Phase::Finished).await.done, 420_000);

        // A piece damaged on disk: Recheck finds it (and fetches nothing offline).
        let mut damaged = a.clone();
        damaged[100_000..100_100].fill(0xff);
        std::fs::write(src.join("a.bin"), &damaged).unwrap();
        t.pause(&hash).await.unwrap();
        t.readd(&hash, &more, true).await.unwrap();
        let s = settled(Phase::Paused).await;
        assert!(s.done < 420_000, "the damaged piece is missing: {s:?}");
        assert!(s.done >= 420_000 - 2 * 16 * 1024, "only it: {s:?}");
        assert!(!t.pieces(&hash).iter().all(|p| *p));
        // Paused stays paused; Reannounce doesn't start it.
        t.reannounce(&hash).await.unwrap();
        assert_eq!(t.stats(&hash).unwrap().phase, Phase::Paused);

        let link = magnet_link(&more).unwrap();
        assert_eq!(
            link,
            format!("magnet:?xt=urn:btih:{hash}&dn=set&tr=http%3A%2F%2Ftracker.example%2Fannounce")
        );
        assert_eq!(magnet_link(b"junk"), None);
        t.stop().await;
    }

    /// #39: two torrents added at the same moment are both in the session. (librqbit gave both
    /// the id 0 and answered the second with the first's handle: it was never added.)
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_torrents_added_at_once_are_both_there() {
        let tmp = tempfile::tempdir().unwrap();
        let t = Torrents::start(offline(&tmp.path().join("state")))
            .await
            .unwrap();
        let (a, b) = (torrent_naming("a.bin"), torrent_naming("b.bin"));
        let (oa, ob) = (tmp.path().join("out-a"), tmp.path().join("out-b"));
        let (ha, hb) = tokio::join!(
            t.add(&a, &oa, None, Vec::new(), true),
            t.add(&b, &ob, None, Vec::new(), true),
        );
        let (ha, hb) = (ha.unwrap(), hb.unwrap());
        assert_ne!(ha, hb);
        assert!(t.has(&ha) && t.has(&hb), "both in the session");
        let mut all = t.hashes();
        all.sort();
        let mut want = vec![ha, hb];
        want.sort();
        assert_eq!(all, want);
        t.stop().await;
    }

    /// #39: what a seeder sends is held back by the session's upload limit, then by its
    /// torrent's own one; the seeder counts what it sent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn upload_limits_hold_the_seeder_back() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed").join("up");
        std::fs::create_dir_all(&src).unwrap();
        let data: Vec<u8> = (0..1_048_576u32).map(|i| (i % 239) as u8).collect();
        std::fs::write(src.join("data.bin"), &data).unwrap();
        let torrent = make_torrent(&src).await;
        let seeder = Torrents::start(offline(&tmp.path().join("s1")))
            .await
            .unwrap();
        let meta = seeder
            .metadata(
                &Source::File(torrent.clone()),
                Duration::from_secs(10),
                None,
            )
            .await
            .unwrap();
        let hash = seeder
            .add(&torrent, &src, None, Vec::new(), false)
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while seeder.stats(&hash).unwrap().phase != Phase::Finished {
            assert!(std::time::Instant::now() < deadline, "seeder never checked");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let addr = seeder.listen_addr().expect("seeder listens");

        // 1 MiB at 256 KiB/s: about 3 s after the first second's burst; unlimited, well under 1.
        for (n, by_torrent) in [(1, false), (2, true)] {
            if by_torrent {
                seeder.set_upload_limit(None);
                seeder
                    .set_torrent_limits(&hash, None, Some(256 * 1024))
                    .unwrap();
            } else {
                seeder.set_upload_limit(Some(256 * 1024));
                assert_eq!(seeder.upload_limit(), Some(256 * 1024));
            }
            let leecher = Torrents::start(offline(&tmp.path().join(format!("l{n}"))))
                .await
                .unwrap();
            let out = meta.folder_in(&tmp.path().join(format!("got{n}")));
            let t = std::time::Instant::now();
            let h2 = leecher
                .add(&torrent, &out, None, vec![addr], false)
                .await
                .unwrap();
            let deadline = t + Duration::from_secs(40);
            while leecher.stats(&h2).unwrap().phase != Phase::Finished {
                assert!(std::time::Instant::now() < deadline, "no transfer ({n})");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let secs = t.elapsed().as_secs_f64();
            assert!(
                secs >= 2.0,
                "1 MiB under a 256 KiB/s upload limit ({}) took {secs:.2}s",
                if by_torrent {
                    "the torrent's"
                } else {
                    "the session's"
                }
            );
            leecher.stop().await;
        }
        assert!(
            seeder.stats(&hash).unwrap().uploaded >= 2 * data.len() as u64,
            "the seeder counts what it sent"
        );
        seeder.stop().await;
    }

    /// #39: a torrent's own limit holds it back, the session's stays as it was, and the bytes
    /// it fetched are counted (the app leaves file downloads the global cap's rest by them).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_torrents_own_limit_holds_it_back() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed").join("one");
        std::fs::create_dir_all(&src).unwrap();
        let data: Vec<u8> = (0..1_048_576u32).map(|i| (i % 241) as u8).collect();
        std::fs::write(src.join("data.bin"), &data).unwrap();
        let torrent = make_torrent(&src).await;

        let seeder = Torrents::start(offline(&tmp.path().join("s1")))
            .await
            .unwrap();
        let meta = seeder
            .metadata(
                &Source::File(torrent.clone()),
                Duration::from_secs(10),
                None,
            )
            .await
            .unwrap();
        let hash = seeder
            .add(&torrent, &src, None, Vec::new(), false)
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while seeder.stats(&hash).unwrap().phase != Phase::Finished {
            assert!(std::time::Instant::now() < deadline, "seeder never checked");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let addr = seeder.listen_addr().expect("seeder listens");

        let leecher = Torrents::start(offline(&tmp.path().join("s2")))
            .await
            .unwrap();
        let out = meta.folder_in(&tmp.path().join("got"));
        let h2 = leecher
            .add(&torrent, &out, None, vec![addr], true)
            .await
            .unwrap();
        assert_eq!(leecher.torrent_limits(&h2), Some((None, None)));
        // 1 MiB at 256 KiB/s: about 3 s after the first second's burst.
        leecher
            .set_torrent_limits(&h2, Some(256 * 1024), None)
            .unwrap();
        assert_eq!(leecher.torrent_limits(&h2), Some((Some(256 * 1024), None)));
        leecher.set_torrent_limits(&h2, Some(1), None).unwrap();
        assert_eq!(
            leecher.torrent_limits(&h2),
            Some((Some(MIN_RATE), None)),
            "never under one block a second"
        );
        leecher
            .set_torrent_limits(&h2, Some(256 * 1024), None)
            .unwrap();
        assert_eq!(leecher.download_limit(), None, "the session's untouched");
        assert!(matches!(
            leecher.set_torrent_limits(&"0".repeat(40), None, None),
            Err(TorrentError::Unknown)
        ));

        let t = std::time::Instant::now();
        leecher.resume(&h2).await.unwrap();
        let deadline = t + Duration::from_secs(40);
        let fetched = loop {
            let s = leecher.stats(&h2).unwrap();
            if s.phase == Phase::Finished {
                break s.fetched;
            }
            assert!(std::time::Instant::now() < deadline, "no transfer: {s:?}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let secs = t.elapsed().as_secs_f64();
        assert!(secs >= 2.0, "1 MiB at 256 KiB/s took {secs:.2}s");
        assert!(fetched >= data.len() as u64, "fetched {fetched}");
        let got = std::fs::read(out.join(&meta.files[0].path)).unwrap();
        assert!(got == data, "the file arrived whole");
        seeder.stop().await;
        leecher.stop().await;
    }

    /// A DHT node on loopback for tests: it answers every query with itself and the `gone` nodes
    /// as the nodes it knows (their ids next to the target, so they get asked), and a
    /// `get_peers` with `peer` (BEP 5, enough for librqbit).
    async fn dht_node_with(peer: SocketAddr, gone: Vec<SocketAddr>) -> SocketAddr {
        let sock = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = sock.local_addr().unwrap();
        let compact = |a: SocketAddr| match a {
            SocketAddr::V4(a) => [a.ip().octets().as_slice(), &a.port().to_be_bytes()].concat(),
            SocketAddr::V6(_) => unreachable!("loopback IPv4"),
        };
        tokio::spawn(async move {
            let id = [0x42u8; 20];
            let mut buf = [0u8; 2048];
            loop {
                let Ok((n, from)) = sock.recv_from(&mut buf).await else {
                    continue;
                };
                let q = &buf[..n];
                // The transaction id: "1:t" then its length and bytes.
                let Some(at) = q.windows(3).position(|w| w == b"1:t") else {
                    continue;
                };
                let rest = &q[at + 3..];
                let Some(colon) = rest.iter().position(|b| *b == b':') else {
                    continue;
                };
                let Some(len) = std::str::from_utf8(&rest[..colon])
                    .ok()
                    .and_then(|l| l.parse::<usize>().ok())
                    .filter(|l| colon + 1 + l <= rest.len())
                else {
                    continue;
                };
                let mut nodes = id.to_vec();
                nodes.extend(compact(addr));
                let target = q
                    .windows(11)
                    .position(|w| w == b"6:target20:")
                    .and_then(|i| q.get(i + 11..i + 31));
                if let Some(target) = target {
                    for (k, g) in gone.iter().enumerate() {
                        let mut near = target.to_vec();
                        near[19] ^= k as u8 + 1;
                        nodes.extend(near);
                        nodes.extend(compact(*g));
                    }
                }
                let mut r = b"d1:rd2:id20:".to_vec();
                r.extend(id);
                r.extend(format!("5:nodes{}:", nodes.len()).as_bytes());
                r.extend(nodes);
                if q.windows(11).any(|w| w == b"9:get_peers") {
                    r.extend(b"5:token2:tk6:valuesl6:");
                    r.extend(compact(peer));
                    r.extend(b"e");
                }
                r.extend(format!("e1:t{len}:").as_bytes());
                r.extend(&rest[colon + 1..colon + 1 + len]);
                r.extend(b"1:y1:re");
                let _ = sock.send_to(&r, from).await;
            }
        });
        addr
    }

    /// #92: a magnet's file list comes through the DHT after nodes that are gone answered with
    /// an ICMP "port unreachable". On Windows that error makes the DHT socket's next receive
    /// fail, and librqbit's DHT (9.0.1, in MDM 0.2.4) stopped for the rest of the session on it:
    /// a magnet the trackers didn't serve then never resolved. The lookup counts what it got.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_magnet_resolves_through_the_dht_after_unreachable_nodes() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed").join("set");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.bin"), vec![7u8; 100_000]).unwrap();
        std::fs::write(src.join("b.bin"), vec![9u8; 50_000]).unwrap();
        let torrent = make_torrent(&src).await;
        let seeder = Torrents::start(offline(&tmp.path().join("s1")))
            .await
            .unwrap();
        let hash = seeder
            .add(&torrent, &src, None, Vec::new(), false)
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while seeder.stats(&hash).unwrap().phase != Phase::Finished {
            assert!(std::time::Instant::now() < deadline, "seeder never checked");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // Nodes that are gone: nothing listens there any more, so each request to one comes
        // back as an ICMP error.
        let gone = (0..7)
            .map(|_| {
                std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                    .unwrap()
                    .local_addr()
                    .unwrap()
            })
            .collect();
        let node = dht_node_with(seeder.listen_addr().expect("seeder listens"), gone).await;
        let leecher = Torrents::start(Config {
            dht: true,
            dht_bootstrap: Some(vec![node.to_string()]),
            ..offline(&tmp.path().join("s2"))
        })
        .await
        .unwrap();
        // Its bootstrap asks them; their errors come back while nothing else arrives.
        tokio::time::sleep(Duration::from_secs(1)).await;

        let lookup = Lookup::default();
        let t = std::time::Instant::now();
        let meta = leecher
            .metadata(
                &Source::Magnet(format!("magnet:?xt=urn:btih:{hash}")),
                Duration::from_secs(20),
                Some(&lookup),
            )
            .await
            .unwrap();
        let took = t.elapsed();
        assert_eq!(meta.info_hash, hash);
        assert_eq!(meta.total(), 150_000);
        let s = lookup.status();
        assert!(s.dht && s.trackers == 0, "{s:?}");
        assert!(s.from_dht >= 1 && s.peers >= 1 && s.connected >= 1, "{s:?}");
        assert!(s.dht_nodes >= 1, "the node it asked is known: {s:?}");
        // librqbit asks again each second while its DHT knows no node; then it is at once.
        assert!(
            took < Duration::from_secs(10),
            "the file list took {took:?}"
        );
        eprintln!("#92: the file list through the DHT in {took:?}");
        seeder.stop().await;
        leecher.stop().await;
    }

    #[tokio::test]
    async fn junk_is_not_a_torrent() {
        let tmp = tempfile::tempdir().unwrap();
        let t = Torrents::start(Config {
            listen: false,
            ..offline(tmp.path())
        })
        .await
        .unwrap();
        let err = t
            .metadata(
                &Source::File(b"not a torrent".to_vec()),
                Duration::from_secs(5),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, TorrentError::NotATorrent), "{err}");
    }

    /// #517: a renamed single-file torrent writes its bytes to the new name on disk (never
    /// creating the original name), checks against the new file, and resumes at the new name
    /// across a session restart. An unchanged single-file torrent writes nothing to `renames.json`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn renamed_single_file_torrent_writes_checks_and_resumes_at_new_name() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("seed");
        std::fs::create_dir_all(&src).unwrap();
        let orig_file = src.join("original.bin");
        let data: Vec<u8> = (0..180_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&orig_file, &data).unwrap();
        let spawner = librqbit::spawn_utils::BlockingSpawner::new(1);
        let torrent = librqbit::create_torrent(
            &orig_file,
            librqbit::CreateTorrentOptions {
                name: None,
                trackers: Vec::new(),
                piece_length: Some(16 * 1024),
            },
            &spawner,
        )
        .await
        .unwrap()
        .as_bytes()
        .unwrap()
        .to_vec();

        let seeder = Torrents::start(offline(&tmp.path().join("seeder")))
            .await
            .unwrap();
        let hash = seeder
            .add(&torrent, &src, None, Vec::new(), false)
            .await
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while seeder.stats(&hash).unwrap().phase != Phase::Finished {
            assert!(std::time::Instant::now() < deadline, "seeder never checked");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let seed_addr = seeder.listen_addr().expect("seeder listens");

        let leech_state = tmp.path().join("leecher");
        let session_dir = leech_state.join("session");
        // 1. Unchanged name: `sync_renamed_file` does not create `renames.json`.
        rename_plugin::sync_renamed_file(
            &session_dir,
            &hash,
            &torrent,
            std::path::Path::new("original.bin"),
        );
        assert!(
            !session_dir.join("renames.json").exists(),
            "unchanged name does not write renames.json"
        );

        // 2. Changed name: `sync_renamed_file` records the rename in `renames.json`.
        rename_plugin::sync_renamed_file(
            &session_dir,
            &hash,
            &torrent,
            std::path::Path::new("renamed_video.mp4"),
        );
        assert!(
            session_dir.join("renames.json").exists(),
            "changed name writes renames.json"
        );

        // 3. Download to `out`: writes to `renamed_video.mp4`, never `original.bin`.
        let out = tmp.path().join("out");
        let leecher = Torrents::start(offline(&leech_state)).await.unwrap();
        let got_hash = leecher
            .add(&torrent, &out, None, vec![seed_addr], false)
            .await
            .unwrap();
        assert_eq!(got_hash, hash);
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while leecher.stats(&hash).unwrap().phase != Phase::Finished {
            assert!(
                std::time::Instant::now() < deadline,
                "leecher never finished: {:?}",
                leecher.stats(&hash)
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            !out.join("original.bin").exists(),
            "original name is never created"
        );
        assert_eq!(
            std::fs::read(out.join("renamed_video.mp4")).unwrap(),
            data,
            "renamed file holds the downloaded bytes"
        );
        let (files, _, _) = leecher.files(&hash).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, PathBuf::from("renamed_video.mp4"));
        leecher.stop().await;

        // 4. Restart the session with a cold in-memory cache: reloads from `renames.json`,
        // resumes at `renamed_video.mp4`, and rechecks the renamed file on disk.
        rename_plugin::clear_memory_for_test();
        let resumed = Torrents::start(offline(&leech_state)).await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while resumed
            .stats(&hash)
            .map_or(true, |s| s.phase != Phase::Finished)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "resumed session never reached Finished: {:?}",
                resumed.stats(&hash)
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(resumed.stats(&hash).unwrap().done, data.len() as u64);

        // Full recheck (`readd(..., true)`) verifies pieces against `renamed_video.mp4`.
        resumed.readd(&hash, &torrent, true).await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while resumed
            .stats(&hash)
            .map_or(true, |s| s.phase != Phase::Finished)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "recheck never finished: {:?}",
                resumed.stats(&hash)
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(resumed.stats(&hash).unwrap().done, data.len() as u64);
        // The lists read without waiting (the progress window's, Files...) name it the new way
        // too after the restart: the renames are loaded as the session starts.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let seen = loop {
            if let Some((files, _, _)) = resumed.seen_files(&hash) {
                break files;
            }
            assert!(std::time::Instant::now() < deadline, "never seen");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].path, PathBuf::from("renamed_video.mp4"));

        resumed.stop().await;
        seeder.stop().await;
    }
}
