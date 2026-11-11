//! One download: probe, parallel connections driven by the planner, positional writes, progress.
//!
//! The first connection is also the probe: a plain `GET` (no Range, as IDM does — D-007) that
//! keeps streaming from byte 0 while further connections split the file (notes/ENGINE.md §1).
//! Range support is learned from the first range answer: 206 confirms it; 200 means the server
//! ignores ranges, so the file goes back to the probe connection alone.
//! One HTTP/1.1 client per download, shared by its connections: one pool (keep-alive sockets
//! reused between segments, TLS sessions resumed) with at most `connections()` requests at a
//! time, since each worker has one request in flight.

use std::collections::HashSet;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::disk;
use crate::dns;
use crate::html;
use crate::limiter::Limiter;
use crate::naming;
use crate::net;
use crate::planner::{Claim, Planner, WorkerId};
use crate::sites::{self, PageSays, Refusal};
use crate::sparse;
use crate::state::{ResumeState, Validators, state_path};

/// Bytes buffered per connection before claiming and writing them.
const BATCH: usize = 256 * 1024;
/// Received bytes are written at least this often, so a slow connection shows progress and a
/// pause keeps what arrived.
const FLUSH_EVERY: Duration = Duration::from_secs(1);
/// While the resume save flushes the part file to the disk (`Shared::flushing`), the connections
/// keep what they receive in memory, at most this much for the whole download, instead of
/// waiting on the disk (#361): on NTFS a file's writes wait for its flush, whatever their
/// handle (measured: 8 connections' writes held for each whole flush, 3-10 s on a slow disk).
const FLUSH_HOLD_MAX: usize = 256 << 20;

/// Shedding a slow connection (`Options::shed_slow`): after `SHED_GRACE` on a response, a
/// connection under 1/`SHED_RATIO` of the best speed one connection reached, with more than
/// `SHED_LEFT` of work left at its own pace, reconnects. A slow TCP path (or a slow server
/// slot) rarely survives a new request, and otherwise every free connection can end up stuck
/// on one (measured: 14 s instead of ~2 s for 32 MiB when every third request was slow).
const SHED_GRACE: Duration = Duration::from_secs(2);
const SHED_RATIO: u64 = 4;
const SHED_LEFT: Duration = Duration::from_secs(2);
/// A download starts with one connection per this many bytes (at most `max_connections`).
const INITIAL_PIECE: u64 = 64 * 1024;
/// A pending connection is taken over after this many first-byte times of the probe (and never
/// sooner than PREEMPT_MIN).
const PREEMPT_TTFB: u32 = 2;
const PREEMPT_MIN: Duration = Duration::from_millis(20);
/// The most connections a download offers to use (IDM's ceiling too).
pub const MAX_CONNECTIONS: u32 = 32;
/// The most connections once the user allows more than `MAX_CONNECTIONS` (issue #12); the
/// engine never opens more, whatever it is given.
pub const MAX_CONNECTIONS_UNLOCKED: u32 = 128;

#[derive(Clone, Debug)]
pub struct Options {
    /// Max simultaneous connections. IDM's default is 8 (notes/ENGINE.md §4). Read through
    /// [`Options::connections`], which keeps it in `1..=MAX_CONNECTIONS_UNLOCKED`.
    pub max_connections: u32,
    /// Smallest half a split may leave. IDM split pieces down to ≤ 12.6 KB (lab S4, D-007).
    pub min_split: u64,
    pub connect_timeout: Duration,
    /// Max silence on a connection before it is considered dead.
    pub read_timeout: Duration,
    /// Consecutive failures after which a connection gives up (it retires if others are
    /// still working, otherwise the download fails).
    pub max_retries: u32,
    /// The first retry is immediate (as IDM, lab S5); then this delay, doubling, capped at 30 s.
    pub retry_delay: Duration,
    /// The download's User-Agent: by default the automatic one (`agent::automatic`, the default
    /// browser's), else the user's or the one a browser handed over.
    pub user_agent: String,
    /// `user_agent` is MDM's automatic one, not one the user set or a browser handed over: a
    /// site that wants an agent of its own (Google Drive's hosts, `sites::user_agent`) gets it.
    /// [`Options::speak_as`] sets another agent and clears this.
    pub automatic_agent: bool,
    /// Asked with instead of `user_agent` when the server refuses the first request with 403
    /// (#372): a browser's User-Agent on a connection that isn't a browser's trips some sites'
    /// bot checks (Cloudflare, seen on a public PDF), while MDM's own passes. Set when
    /// `user_agent` is the browser's. A later 403 is left to the download.
    pub agent_fallback: Option<String>,
    /// From the browser: the address its own download of this file ended at, its redirects
    /// followed. When the address asked for is refused before any byte (`link_refused`: a
    /// one-time link the browser already spent, a token bound to its session, a site that
    /// turns away whatever isn't that browser), the download asks there once instead (#630),
    /// the address's cookies and logins going only if it is the same site (`scoped_headers`).
    /// Never for a form's POST (its body goes where it was given) or through a service account.
    pub browser_final: Option<Url>,
    /// How often resume state is saved (IDM: "several times per minute").
    pub save_interval: Duration,
    /// Speed limit for this download, bytes/s (IDM's limiter is per file). None = unlimited.
    pub speed_limit: Option<u64>,
    /// Before continuing a segment that already has data on disk, re-fetch up to this many of
    /// its last written bytes and compare them (IDM's resume overlap check; catches changed
    /// files on servers without validators). 0 turns it off.
    pub verify_overlap: u64,
    /// Fixed addresses for host names, like hosts-file entries (tests).
    pub hosts: Vec<(String, Vec<std::net::IpAddr>)>,
    /// Options → Connection → DNS: who answers the names (#36).
    pub dns: crate::dns::DnsChoice,
    /// Options → Proxy / Socks → Network adapter (#35): the adapter every connection goes
    /// through, or none at all.
    pub route: crate::net::Route,
    /// A service account that unlocks this download's link first (#44, `crate::unlock`): asked
    /// before the first request and when the direct link dies. Not in the resume state (like
    /// the headers): whoever resumes gives it again.
    pub unlocker: Option<Arc<dyn crate::unlock::Unlocker>>,
    /// Options → Proxy / Socks.
    pub proxy: Proxy,
    /// A limit shared by every download using the same limiter (a global speed cap), on top
    /// of the download's own.
    pub global: Option<Arc<Limiter>>,
    /// The finished file must have this hash (else the server's announced one, if any).
    pub checksum: Option<crate::verify::Hash>,
    /// Reconnect a connection far slower than the others (see `SHED_*`). Beyond IDM's rule.
    pub shed_slow: bool,
    /// Connection caps learned per host, shared by the downloads that use the same table.
    pub host_limits: Option<Arc<crate::hosts::HostLimits>>,
    /// Resume across a changed modification time (IDM: "Ignore file modification time changes
    /// when resuming a download"): servers that stamp every answer with a new date would
    /// otherwise make each resume start over. Only an ETag then tells a changed file.
    pub ignore_mtime: bool,
    /// Give the finished file the server's date (IDM: "Set file creation date as provided by
    /// the server"): created and modified times from `Last-Modified`.
    pub server_time: bool,
    /// FTP data connections: passive (the client connects; IDM's "Use passive mode for FTP",
    /// on by default), else active (the server connects back).
    pub ftp_passive: bool,
    /// Certificates (DER) trusted besides the system's for FTPS and HTTPS (tests' own CA).
    pub extra_roots: Vec<Vec<u8>>,
    /// Send every request as POST with this body, the Content-Type coming from the headers (a
    /// browser form's download). Redirects go as in browsers: 301, 302 and 303 turn it into a
    /// GET without the body (and without its Content-Type), 307 and 308 keep both. A POST may
    /// have side effects, so further connections repeat it only once an answer has shown that
    /// the server takes ranges (206 or `Accept-Ranges: bytes`); until then the first
    /// connection takes the whole file alone. Not stored in the resume state (like the
    /// headers): whoever resumes gives it again. HTTP only.
    pub post: Option<Bytes>,
    /// Called with the part file's path right after the engine creates it, before any data
    /// is written: once per new download (`Download::start`, `fetch`), never by a resume, which
    /// writes into the part file it finds. The part file is renamed (never copied) to the final
    /// name, so whatever the hook attaches to it — Windows' Mark of the Web stream, macOS'
    /// quarantine attribute — stays with the finished file.
    pub on_file_created: Option<FileHook>,
    /// From the browser: the MIME type its own download had. A web page where it had another
    /// type is the server refusing MDM (a firewall's "The requested URL was rejected"), an error
    /// and never saved as the file (#533, `expects_other_file`).
    pub expected_type: Option<String>,
    /// Tests: a disk whose flush takes this long and holds every write to the file meanwhile
    /// (as NTFS does, #361). None: the real disk.
    #[doc(hidden)]
    pub slow_flush: Option<SlowFlush>,
}

/// A disk whose flush takes `takes` and, as NTFS's does, makes every write to the file wait for
/// it: the gate is held exclusively for the flush and shared for each write (`Options::slow_flush`).
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct SlowFlush {
    pub takes: Duration,
    pub gate: Arc<std::sync::RwLock<()>>,
}

/// A callback given the path of a file the engine created (`Options::on_file_created`).
#[derive(Clone)]
pub struct FileHook(pub Arc<dyn Fn(&Path) + Send + Sync>);

impl FileHook {
    pub fn new(f: impl Fn(&Path) + Send + Sync + 'static) -> Self {
        FileHook(Arc::new(f))
    }
}

impl std::fmt::Debug for FileHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileHook")
    }
}

/// Which proxy downloads go through (Options → Proxy / Socks).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Proxy {
    /// "No proxy/socks" (IDM's default): direct connections.
    #[default]
    None,
    /// "Use system settings": Windows' proxy (and the environment's).
    System,
    /// "Manual proxy/socks configuration": `http://host:port` (or `socks5://…`), with an
    /// optional login, for the listed schemes.
    Manual {
        url: String,
        login: Option<(String, String)>,
        http: bool,
        https: bool,
    },
    /// "Use automatic configuration script": the script decides per address (`pac`); one
    /// that doesn't run leaves it to the system's settings.
    Pac { script: Arc<str> },
}

/// Whether the engine can download `url`: http, https, ftp, ftps (implicit TLS) and ftpes
/// (explicit TLS). Magnet links are the torrent engine's.
pub fn supported(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https") || crate::ftp::is_ftp(url)
}

impl Default for Options {
    fn default() -> Self {
        Options {
            max_connections: 8,
            min_split: 12 * 1024,
            connect_timeout: Duration::from_secs(30),
            read_timeout: Duration::from_secs(30),
            max_retries: 5,
            retry_delay: Duration::from_secs(1),
            user_agent: crate::agent::automatic(),
            automatic_agent: true,
            agent_fallback: None,
            browser_final: None,
            save_interval: Duration::from_secs(2),
            speed_limit: None,
            verify_overlap: 64,
            hosts: Vec::new(),
            dns: Default::default(),
            route: Default::default(),
            unlocker: None,
            proxy: Proxy::None,
            global: None,
            checksum: None,
            shed_slow: true,
            host_limits: None,
            ignore_mtime: false,
            server_time: false,
            ftp_passive: true,
            extra_roots: Vec::new(),
            post: None,
            on_file_created: None,
            expected_type: None,
            slow_flush: None,
        }
    }
}

impl Options {
    /// Speak as `agent` (the user's, a browser's) instead of the automatic User-Agent.
    pub fn speak_as(&mut self, agent: String) {
        self.user_agent = agent;
        self.automatic_agent = false;
    }

    /// A site's own User-Agent may replace this download's (`request_agent`): the agent is the
    /// automatic one, or MDM's former one (#372's fallback, D-007), as before #668.
    fn site_agent(&self) -> bool {
        self.automatic_agent || self.user_agent == crate::agent::IE11_USER_AGENT
    }

    /// The connections this download may open: `max_connections` in
    /// `1..=MAX_CONNECTIONS_UNLOCKED` (a hand-edited setting can hold anything).
    pub fn connections(&self) -> u32 {
        self.max_connections.clamp(1, MAX_CONNECTIONS_UNLOCKED)
    }
}

#[derive(Clone, Debug)]
pub enum Destination {
    /// Save into this folder under the name the server/URL suggests (numbered if taken).
    Dir(PathBuf),
    /// Save exactly here.
    File(PathBuf),
    /// Save here, or under the next free numbered name in its folder (`naming::unique_path`)
    /// when this one is taken as the download starts: a name given before its folder was looked
    /// at (#743: a window that waits for a sleeping drive freezes), decided here, beside it.
    Free(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Request {
    pub url: Url,
    /// Extra headers sent on every request (Referer, Cookie, Authorization …).
    pub headers: HeaderMap,
    pub dest: Destination,
}

/// Cloneable (the network and disk errors it wraps are shared), so a snapshot can carry the
/// failure itself for an interface to word in its own language (`Snapshot::failure`).
#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("{}", describe_http(.0))]
    Http(Arc<reqwest::Error>),
    #[error("server answered {0}")]
    Status(StatusCode),
    /// The site's bot check refused the request: it lets browsers through, not programs (#376).
    /// Cloudflare says so in its answer (`cf-mitigated: challenge`).
    #[error("the site's bot check refused the download")]
    BotCheck,
    /// 429 / 503: the server is busy; retry after its `Retry-After` when it gives one.
    #[error("server busy ({0})")]
    Throttled(StatusCode, Option<Duration>),
    #[error("server sent an unexpected range: {0}")]
    BadRange(String),
    #[error("connection closed before the segment was complete")]
    ShortBody,
    #[error("the file changed on the server")]
    FileChanged,
    /// The server ignores ranges and the probe connection can't take the file back alone.
    #[error("the server does not support ranges")]
    NoRanges,
    #[error("cannot resume: {0}")]
    BadState(String),
    #[error("disk: {0}")]
    Io(Arc<io::Error>),
    /// Checked before starting: the file and a safety margin don't fit on the drive.
    #[error("not enough disk space: {} needed, {} free", disk::human(*needed), disk::human(*free))]
    NoSpace { needed: u64, free: u64 },
    /// The drive filled up while downloading (another program took the space).
    #[error("the disk is full")]
    DiskFull,
    #[error("{} is too big for this {file_system} drive (files up to 4 GB)", disk::human(*size))]
    TooBigForDrive { size: u64, file_system: String },
    #[error("cancelled")]
    Cancelled,
    /// The chosen network adapter is down, or can't reach this server (#35).
    #[error(transparent)]
    Adapter(#[from] crate::net::NetError),
    /// Another download in this process is writing to the same file.
    #[error("already being downloaded: {0}")]
    InUse(PathBuf),
    /// The save folder can't be reached or created (a removed drive, an offline share).
    #[error("the folder {} is not available (drive removed or network share offline?)", .0.display())]
    DestinationUnavailable(PathBuf),
    /// The drive or network share went away while downloading.
    #[error("the drive or network share is no longer available")]
    DriveGone,
    /// Longer than Windows programs can open (MAX_PATH).
    #[error("the path is too long for Windows: {}", .0.display())]
    PathTooLong(PathBuf),
    /// A web page (a sign-in wall, an expired link's notice) came instead of the file.
    #[error("the server sent a web page instead of the file (sign-in needed or the link expired?)")]
    GotWebPage,
    /// A full answer to a range request for the same, unchanged file: the server (or one CDN
    /// edge) didn't serve the range this time. Retried; the file did not change.
    #[error("the server did not send the requested part")]
    RangesUnavailable,
    /// The redirected address stopped working and the original address no longer leads to this
    /// file: the user has to give a fresh address (Refresh download address).
    #[error("the download link has expired (refresh the download address)")]
    LinkExpired,
    /// This connection was far slower than the others: it reconnects for the rest of its
    /// segment (not a failure).
    #[error("much slower than the other connections; reconnecting")]
    SlowConnection,
    /// The finished file doesn't have the expected hash; it stays as the part file.
    #[error("the file's {0} checksum doesn't match (the data is kept; redownload it)")]
    ChecksumMismatch(&'static str),
    /// An FTP server's refusal: its reply code and text.
    #[error("the FTP server answered {code} {text}")]
    FtpReply { code: u32, text: String },
    /// A network failure outside HTTP (FTP's sessions): `cause` as the system words it.
    #[error("{}", describe_net(*.kind, .cause))]
    Net { kind: NetKind, cause: String },
    /// A site's own refusal (Google Drive's quota page, its sign-in wall, a folder link, #34):
    /// retrying now won't help, the user has to do something else.
    #[error("{0}")]
    Refused(Refusal),
    /// A secure connection (HTTPS, FTPS) on a system without trusted certificates to check
    /// the server with (a minimal Linux without `ca-certificates`, #264).
    #[error("{}", crate::tls::NO_ROOTS)]
    NoCertificates,
}

/// What failed on the network (`Error::Net`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetKind {
    Connect,
    /// The server went quiet.
    Timeout,
    /// The connection broke while receiving.
    Lost,
    /// The secure connection failed (an untrusted certificate, a handshake the server refused).
    Tls,
}

impl Error {
    /// The address no longer serves this file (or refuses us): retrying won't help.
    fn link_dead(&self) -> bool {
        matches!(self, Error::Status(s) if matches!(s.as_u16(), 401 | 403 | 404 | 410 | 416))
            || matches!(self, Error::GotWebPage | Error::LinkExpired | Error::BotCheck)
            // A site's refusal; a service's, unless waiting helps (#44).
            || matches!(self, Error::Refused(r) if !matches!(r, Refusal::Service { fault, .. } if fault.retry_wait().is_some()))
            // Login refused; no such file (or no permission); a name the server won't take.
            || matches!(self, Error::FtpReply { code: 530 | 532 | 550 | 553, .. })
    }

    /// Errors that stop the whole download instead of being retried by a connection.
    fn is_fatal(&self) -> bool {
        matches!(
            self,
            Error::FileChanged
                | Error::NoRanges
                | Error::BadState(_)
                | Error::Io(_)
                | Error::NoSpace { .. }
                | Error::DiskFull
                | Error::TooBigForDrive { .. }
                | Error::DestinationUnavailable(_)
                | Error::DriveGone
                | Error::PathTooLong(_)
                | Error::Adapter(_)
                | Error::ChecksumMismatch(_)
                | Error::Cancelled
                | Error::InUse(_)
                | Error::NoCertificates
        )
    }

    /// A failure of the first request that a later try may not have: a busy server (429, 503),
    /// a server error (500, 502, 504), a network that failed to connect or went quiet.
    fn transient(&self) -> bool {
        match self {
            Error::Throttled(..) => true,
            Error::Status(s) => matches!(s.as_u16(), 500 | 502 | 504),
            Error::Http(h) => h.is_connect() || h.is_timeout() || h.is_request() || h.is_body(),
            Error::ShortBody => true,
            // Too many sessions; no data connection; transfer aborted; file busy; local error.
            Error::FtpReply { code, .. } => matches!(code, 421 | 425 | 426 | 450 | 451),
            Error::Net { kind, .. } => *kind != NetKind::Tls,
            Error::Refused(Refusal::Service { fault, .. }) => fault.retry_wait().is_some(),
            _ => false,
        }
    }

    /// A server turning this connection away while others work: its connection limit. FTP
    /// servers say it at login, with 421 or (vsftpd, ProFTPD) 530.
    fn refused(&self) -> bool {
        match self {
            Error::Throttled(..) => true,
            Error::Status(s) => s.as_u16() == 403,
            Error::Http(h) => h.is_connect(),
            Error::FtpReply { code, .. } => matches!(code, 421 | 530),
            Error::Net { kind, .. } => *kind == NetKind::Connect,
            _ => false,
        }
    }
}

/// How long to wait before try number `failures + 1` after `e`: the first retry at once (IDM
/// reconnects within ~12 ms, lab S5), then `retry_delay` doubling up to 30 s; a busy server's
/// Retry-After wins (capped at 5 minutes). One policy for the first request and every connection.
fn retry_wait(opts: &Options, failures: u32, e: &Error) -> Option<Duration> {
    match e {
        Error::Throttled(_, Some(after)) => Some((*after).min(Duration::from_secs(300))),
        _ if failures > 1 => {
            Some((opts.retry_delay * 2u32.pow((failures - 2).min(10))).min(Duration::from_secs(30)))
        }
        _ => None,
    }
}

/// A network error in words a user can act on, with its deepest cause ("connection refused",
/// "dns error: …") but not the address, which the dialog already shows.
fn describe_http(e: &reqwest::Error) -> String {
    let what = if e.is_timeout() {
        "the server did not answer in time"
    } else if e.is_connect() {
        "cannot connect to the server"
    } else if e.is_body() || e.is_decode() {
        "the connection was lost while receiving data"
    } else if e.is_redirect() {
        "too many redirects"
    } else {
        "network error"
    };
    let mut cause = None;
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        cause = Some(s.to_string());
        source = s.source();
    }
    match cause.map(|c| strip_os_error_code(&c)) {
        Some(c) if !c.is_empty() && !c.contains("://") => format!("{what} ({c})"),
        _ => what.to_owned(),
    }
}

/// A network failure outside HTTP in words a user can act on, with its cause.
fn describe_net(kind: NetKind, cause: &str) -> String {
    let what = match kind {
        NetKind::Connect => "cannot connect to the server",
        NetKind::Timeout => "the server did not answer in time",
        NetKind::Lost => "the connection was lost while receiving data",
        NetKind::Tls => "the secure connection failed",
    };
    if cause.is_empty() {
        what.to_owned()
    } else {
        format!("{what} ({cause})")
    }
}

/// "… refused it. (os error 10061)" → "… refused it".
pub(crate) fn strip_os_error_code(s: &str) -> String {
    let s = match s.rfind(" (os error ") {
        Some(i) if s.ends_with(')') => &s[..i],
        _ => s,
    };
    s.trim().trim_end_matches('.').to_owned()
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        // NOT_READY, BAD_NETPATH, DEV_NOT_EXIST, NETNAME_DELETED, BAD_NET_NAME,
        // DEVICE_NOT_CONNECTED: the drive or share is gone, not a disk fault.
        #[cfg(windows)]
        if matches!(e.raw_os_error(), Some(21 | 53 | 55 | 64 | 67 | 1167)) {
            return Error::DriveGone;
        }
        // ENXIO, ENODEV (both systems); ENOMEDIUM (Linux): the device is gone.
        #[cfg(unix)]
        if matches!(e.raw_os_error(), Some(6 | 19))
            || (cfg!(target_os = "linux") && e.raw_os_error() == Some(123))
        {
            return Error::DriveGone;
        }
        match e.kind() {
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => Error::DiskFull,
            _ => Error::Io(Arc::new(e)),
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        if crate::tls::is_no_roots(&e) {
            return Error::NoCertificates;
        }
        Error::Http(Arc::new(e))
    }
}

impl From<disk::Refusal> for Error {
    fn from(r: disk::Refusal) -> Self {
        match r {
            disk::Refusal::NoSpace { needed, free } => Error::NoSpace { needed, free },
            disk::Refusal::TooBigForFileSystem { size, file_system } => {
                Error::TooBigForDrive { size, file_system }
            }
        }
    }
}

/// The request headers to send to `to` when they were given for `from`: credentials
/// (cookies, authorization) never follow a redirect to another host or from https down to
/// http — as reqwest does while it follows the redirect itself — except between one site's own
/// hosts (`sites::same_party`: a Drive link's cookies go to Drive's file host).
pub(crate) fn scoped_headers(headers: &HeaderMap, from: &Url, to: &Url) -> HeaderMap {
    let mut h = headers.clone();
    let same_host = (from.host_str() == to.host_str()
        && from.port_or_known_default() == to.port_or_known_default())
        || sites::same_party(from, to);
    let downgrade = from.scheme() == "https" && to.scheme() != "https";
    if !same_host || downgrade {
        for name in [
            header::COOKIE,
            header::AUTHORIZATION,
            header::PROXY_AUTHORIZATION,
        ] {
            h.remove(name);
        }
    }
    h
}

/// The addresses a POST download's redirects turned into GETs (301, 302, 303, as browsers
/// do), recorded by its client's redirect policy: requests to them afterwards are GETs too.
#[derive(Clone, Default)]
struct AsGet(Arc<Mutex<HashSet<Url>>>);

impl AsGet {
    fn contains(&self, url: &Url) -> bool {
        self.0.lock().unwrap().contains(url)
    }

    /// Following up to `MAX_REDIRECTS`, noting the method each hop leaves with.
    fn policy(&self) -> reqwest::redirect::Policy {
        let seen = self.0.clone();
        reqwest::redirect::Policy::custom(move |attempt| {
            let mut seen = seen.lock().unwrap();
            // `previous` ends with the address that redirected; the first is the one asked.
            let came_as_get = attempt.previous().len() > 1
                && attempt.previous().last().is_some_and(|u| seen.contains(u));
            if came_as_get || matches!(attempt.status().as_u16(), 301..=303) {
                seen.insert(attempt.url().clone());
            } else {
                seen.remove(attempt.url());
            }
            drop(seen);
            reqwest::redirect::Policy::limited(MAX_REDIRECTS).redirect(attempt)
        })
    }
}

/// Most HTTP redirects (301, 302, 303, 307, 308) a request follows, as browsers do (20; reqwest
/// stops at 10): download links often go through a tracker, a mirror picker and a CDN.
const MAX_REDIRECTS: usize = 20;

/// A GET's redirects, followed up to `MAX_REDIRECTS`, except one between two of a site's own
/// hosts (`sites::same_party`: Drive's `open?id=` to Docs): reqwest would drop the user's cookies
/// there, so it is left to `get`, which follows it with them.
fn get_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let party_hop = attempt.previous().last().is_some_and(|from| {
            from.host_str() != attempt.url().host_str() && sites::same_party(from, attempt.url())
        });
        if party_hop {
            attempt.stop()
        } else {
            reqwest::redirect::Policy::limited(MAX_REDIRECTS).redirect(attempt)
        }
    })
}

/// The body a POST download sends to an address reached with or without a redirect to a GET
/// (`as_get`); for a GET, the body's headers go too, as browsers drop them. None: a GET.
fn post_for(opts: &Options, as_get: bool, headers: &mut HeaderMap) -> Option<Bytes> {
    let body = opts.post.clone()?;
    if !as_get {
        return Some(body);
    }
    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_ENCODING,
    ] {
        headers.remove(name);
    }
    None
}

/// A request for `url` with `headers`: a POST with `post` as its body, else a GET. The client
/// sends the download's User-Agent (`Options::user_agent`); a site that wants a browser's gets
/// it instead when `site_agent` (`Options::site_agent`: the agent is MDM's automatic one).
fn build(
    client: &Client,
    url: Url,
    post: Option<&Bytes>,
    headers: &HeaderMap,
    site_agent: bool,
) -> RequestBuilder {
    let mut headers = headers.clone();
    if let Some(site) = request_agent(site_agent, &url)
        && !headers.contains_key(header::USER_AGENT)
    {
        headers.insert(header::USER_AGENT, HeaderValue::from_static(site));
    }
    let req = match post {
        Some(body) => client.post(url).body(body.clone()),
        None => client.get(url),
    };
    req.headers(headers)
}

/// The User-Agent a request to `url` carries instead of the download's own: a site's browser
/// agent (`sites::user_agent`: Google Drive's hosts), only when `site_agent` (the download's is
/// MDM's automatic one, `Options::site_agent`). A browser's agent handed over with the download,
/// or one the user set, always wins.
fn request_agent(site_agent: bool, url: &Url) -> Option<&'static str> {
    site_agent.then(|| sites::user_agent(url)).flatten()
}

/// A header's text: UTF-8, else ISO-8859-1 (servers send file names either way, while
/// `HeaderValue::to_str` takes ASCII only).
fn header_text(h: &HeaderMap, name: header::HeaderName) -> Option<String> {
    let raw = h.get(name)?.as_bytes();
    Some(match std::str::from_utf8(raw) {
        Ok(s) => s.to_owned(),
        Err(_) => raw.iter().map(|&b| char::from(b)).collect(),
    })
}

/// What the server's range answers have shown so far.
const RANGES_UNKNOWN: u8 = 0;
const RANGES_YES: u8 = 1;
const RANGES_NO: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Probing,
    Downloading,
    /// Checking the finished file against its checksum.
    Verifying,
    Completing,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub enum ConnStatus {
    Connecting,
    Receiving,
    /// Waiting before a retry, with the last error.
    Retrying(Error),
    Finished,
    Failed(Error),
    /// Closed for good: `host` allows `cap` connections and the others fill them (learned).
    Retired {
        host: String,
        cap: u32,
    },
}

#[derive(Clone, Debug)]
pub struct ConnView {
    pub id: WorkerId,
    pub downloaded: u64,
    pub status: ConnStatus,
    /// Bytes/s over the current response (0 while not receiving).
    pub speed: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentView {
    pub start: u64,
    pub cursor: u64,
    pub end: u64,
}

/// A point-in-time view for progress displays.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub state: State,
    pub url: Url,
    pub file: Option<PathBuf>,
    pub size: Option<u64>,
    pub downloaded: u64,
    pub resumable: Option<bool>,
    pub segments: Vec<SegmentView>,
    pub connections: Vec<ConnView>,
    /// Why the download failed (state `Failed`), in English (the command line, logs).
    pub error: Option<String>,
    /// The same failure as data, for an interface that words it in its own language.
    pub failure: Option<Error>,
    /// What happened, oldest first (the last `EVENTS`): the download's event log. Never
    /// cookies or credentials.
    pub events: Vec<Event>,
    /// While a service account prepares the file's link (#44): the service, and how far it is
    /// (0 to 1) when it says. The list shows it instead of "Connecting...".
    pub at_service: Option<(String, Option<f32>)>,
    /// Where its server is, as its connections saw it (#140).
    pub server: Option<crate::Server>,
}

/// One entry of a download's event log; `seq` counts up within one `Download`.
#[derive(Clone, Debug)]
pub struct Event {
    pub seq: u64,
    pub at: std::time::SystemTime,
    pub text: String,
}

/// Events kept per download.
pub const EVENTS: usize = 50;

/// What a quick look at a URL tells before downloading (for "Download File Info").
#[derive(Clone, Debug)]
pub struct ProbeInfo {
    pub final_url: Url,
    pub size: Option<u64>,
    /// Suggested local name (Content-Disposition, else URL).
    pub file_name: String,
    /// Where the server is, as the look saw it (#140).
    pub server: Option<crate::Server>,
}

/// One plain `GET` (or `Options::post`'s POST) to learn the final URL, size and name, through
/// the redirects `get` follows (a waiting page's look reads up to `PAGE_PEEK` of it and keeps its
/// wait); the file's own body is not read.
pub async fn probe(url: Url, headers: HeaderMap, opts: &Options) -> Result<ProbeInfo, Error> {
    probe_kept(url, headers, opts).await.map(|(info, _)| info)
}

/// The open answer of a look (`probe_kept`), for the download that follows it to read (#28): a
/// one-time link, or a host that allows one download at a time, refuses the download asking
/// again. Dropping it closes the connection.
pub struct KeptAnswer {
    /// The address asked: only a download of the same address takes it.
    url: Url,
    client: Client,
    as_get: AsGet,
    head: Head,
    body: Body,
    /// The browser-like connection it came through, when only that one got past the site's
    /// bot check (#668): the download goes on with it.
    browser: Option<Arc<crate::browserlike::Browser>>,
    headers: HeaderMap,
    took: Duration,
    at: std::time::Instant,
    /// The service's direct link it was asked at (#44), with its connection cap and name.
    unlocked: Option<crate::unlock::Unlocked>,
    /// What its client learnt of the server (#140): the download goes on with it.
    seen: Arc<crate::server::ServerSeen>,
}

impl std::fmt::Debug for KeptAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeptAnswer")
            .field("url", &self.url.as_str())
            .field("status", &self.head.status)
            .field("browser_like", &self.browser.is_some())
            .finish_non_exhaustive()
    }
}

/// How long a kept answer is worth taking: a server ends an idle connection after a while,
/// and the download then asks again (as when it isn't kept).
const KEEP_ANSWER_FOR: Duration = Duration::from_secs(15 * 60);

/// `probe`, keeping its answer open when its body is the file from byte 0 (200, or 206 from
/// 0), for `Download::start_with`. Not for FTP, whose look is a separate command.
pub async fn probe_kept(
    url: Url,
    headers: HeaderMap,
    opts: &Options,
) -> Result<(ProbeInfo, Option<KeptAnswer>), Error> {
    if crate::ftp::is_ftp(&url) {
        let site = crate::ftp::Site::new(&url, &headers, opts)?;
        let facts = site.check().await?;
        return Ok((
            ProbeInfo {
                file_name: naming::file_name(None, &url),
                final_url: url,
                size: facts.size,
                server: None,
            },
            None,
        ));
    }
    let as_get = AsGet::default();
    // Through a service account (#44): its direct link is looked at instead, without the host's
    // cookies or logins.
    let unlocked = unlock_origin(opts, &url, None, &CancellationToken::new()).await?;
    let (target, headers) = match &unlocked {
        Some(u) => (u.url.clone(), HeaderMap::new()),
        None => (url.clone(), headers),
    };
    let seen = Arc::new(crate::server::ServerSeen::default());
    let client = client(opts, &target, &as_get, &seen)?;
    let expects = expects_other_file(opts.expected_type.as_deref(), None, &url);
    // Where the look asked, and with what: asked again there if the site refuses (#668).
    let (asked_at, asked_with) = (target.clone(), headers.clone());
    // A browser's User-Agent refused (#372): asked once more as MDM.
    let (client, asked) = match get(
        &client,
        target.clone(),
        headers.clone(),
        opts.post.as_ref(),
        opts.site_agent(),
        expects,
        &CancellationToken::new(),
    )
    .await
    {
        Err(e) if agent_refused(&e) && opts.agent_fallback.is_some() => {
            let mut plain = opts.clone();
            plain.user_agent = opts.agent_fallback.clone().unwrap_or_default();
            let client = self::client(&plain, &target, &as_get, &seen)?;
            let asked = get(
                &client,
                target,
                headers,
                opts.post.as_ref(),
                plain.site_agent(),
                expects,
                &CancellationToken::new(),
            )
            .await;
            (client, asked)
        }
        asked => (client, asked),
    };
    let (head, body, browser, headers, took) = match asked {
        Ok((resp, headers, took)) => {
            seen.answered(&resp);
            (Head::of(&resp), Body::Http(resp), None, headers, took)
        }
        // #668: the site refused MDM's connection as a program's (`refused_as_a_program`):
        // looked at once more the way a browser connects, its answer kept for the download
        // (which then asks no more).
        Err(e) if refused_as_a_program(&e) && unlocked.is_none() && opts.post.is_none() => {
            let cancel = CancellationToken::new();
            let (b, (head, body, headers, took)) =
                browser_ask(opts, &asked_at, &asked_with, expects, &cancel)
                    .await
                    .map_err(|other| match other {
                        Error::Cancelled | Error::Adapter(_) => other,
                        _ => e,
                    })?;
            (head, body, Some(b), headers, took)
        }
        Err(e) => return Err(e),
    };
    let final_url = head.url.clone();
    let (size, from_zero) = match head.status {
        StatusCode::PARTIAL_CONTENT => {
            let (start, _, total) = parse_content_range(&head.headers)?;
            (total, start == 0)
        }
        StatusCode::OK => (head.len, true),
        _ => (head.len, false),
    };
    let cd = header_text(&head.headers, header::CONTENT_DISPOSITION);
    let file_name = name_for(cd.as_deref(), &final_url, unlocked.as_ref());
    let info = ProbeInfo {
        final_url,
        size: size.or(unlocked.as_ref().and_then(|u| u.size)),
        file_name,
        server: seen.get(),
    };
    let kept = from_zero.then(|| KeptAnswer {
        url,
        client,
        seen,
        as_get,
        head,
        body,
        browser,
        headers,
        took,
        at: std::time::Instant::now(),
        unlocked,
    });
    Ok((info, kept))
}

/// A client for requests to `url`'s server that aren't downloads (a debrid service's API, #44):
/// the same proxy, DNS and network adapter as `opts`' downloads.
pub fn http_client(opts: &Options, url: &Url) -> Result<Client, Error> {
    client(opts, url, &AsGet::default(), &Default::default())
}

/// The file's name: the server's (Content-Disposition), else the service's (#44), else the
/// address's.
fn name_for(cd: Option<&str>, url: &Url, unlocked: Option<&crate::unlock::Unlocked>) -> String {
    match (cd, unlocked.and_then(|u| u.file_name.as_deref())) {
        (None, Some(name)) if !name.trim().is_empty() => naming::sanitize(name),
        _ => naming::file_name(cd, url),
    }
}

/// `opts.unlocker`'s direct link for `url` (#44), noted in the event log (never the link
/// itself): None without an unlocker, for a form's POST, or when the service declines the host.
async fn unlock_origin(
    opts: &Options,
    url: &Url,
    shared: Option<&Shared>,
    cancel: &CancellationToken,
) -> Result<Option<crate::unlock::Unlocked>, Error> {
    let Some(u) = &opts.unlocker else {
        return Ok(None);
    };
    if opts.post.is_some() || !matches!(url.scheme(), "http" | "https") {
        return Ok(None);
    }
    unlock_noted(u, url, shared, cancel).await.map_err(|fault| {
        Error::Refused(Refusal::Service {
            service: u.name().to_owned(),
            fault,
        })
    })
}

/// `u`'s link for `url`, its waits told in the event log and the list's status ("40% at
/// TorBox") meanwhile, and the link it gave noted (never the link itself).
async fn unlock_noted(
    u: &Arc<dyn crate::unlock::Unlocker>,
    url: &Url,
    shared: Option<&Shared>,
    cancel: &CancellationToken,
) -> Result<Option<crate::unlock::Unlocked>, crate::unlock::ServiceFault> {
    let note = |s: String, progress: Option<f32>| {
        if let Some(sh) = shared {
            sh.event(s);
            sh.view.lock().unwrap().at_service = Some((u.name().to_owned(), progress));
        }
    };
    let out = u.unlock(url, &note, cancel).await;
    if let Some(sh) = shared {
        sh.view.lock().unwrap().at_service = None;
        if let Ok(Some(x)) = &out {
            sh.event(crate::unlock::note_unlocked(u.name(), x));
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub path: PathBuf,
    pub size: u64,
}

/// A running download.
pub struct Download {
    shared: Arc<Shared>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<Result<Outcome, Error>>,
}

impl Download {
    /// Start downloading in the background. Must be called inside a Tokio runtime.
    pub fn start(req: Request, opts: Options) -> Download {
        Download::start_with(req, opts, None)
    }

    /// `start`, connection 1 reading `kept` (a look's open answer, `probe_kept`) instead of
    /// asking again, when it is the same address's and recent (`KEEP_ANSWER_FOR`).
    pub fn start_with(req: Request, opts: Options, kept: Option<KeptAnswer>) -> Download {
        Download::spawn(req.url.clone(), move |shared, cancel| {
            let sh = shared.clone();
            let body = async move {
                if crate::ftp::is_ftp(&req.url) {
                    run_fresh_ftp(req, opts, sh, cancel).await
                } else {
                    run_fresh(req, opts, sh, cancel, kept).await
                }
            };
            run(body, shared)
        })
    }

    /// Continue a paused, cancelled or crashed download from its state file
    /// (`state::state_path(dest)`). `headers` are sent on every request (cookies etc. are not
    /// stored in the state file).
    pub fn resume(
        state_file: PathBuf,
        headers: HeaderMap,
        opts: Options,
    ) -> Result<Download, Error> {
        // A removed drive or offline share: say so (the data is still there for later).
        if let Some(dir) = state_file.parent().filter(|d| !d.as_os_str().is_empty())
            && !dir.is_dir()
        {
            return Err(Error::DestinationUnavailable(dir.to_owned()));
        }
        let saved = ResumeState::load(&state_file).map_err(Error::BadState)?;
        Ok(Download::spawn(saved.url.clone(), move |shared, cancel| {
            run(
                run_resume(saved, state_file, headers, opts, shared.clone(), cancel),
                shared,
            )
        }))
    }

    fn spawn<F, Fut>(url: Url, f: F) -> Download
    where
        F: FnOnce(Arc<Shared>, CancellationToken) -> Fut,
        Fut: Future<Output = Result<Outcome, Error>> + Send + 'static,
    {
        let cancel = CancellationToken::new();
        let shared = Arc::new(Shared {
            view: Mutex::new(Snapshot {
                state: State::Probing,
                url,
                file: None,
                size: None,
                downloaded: 0,
                resumable: None,
                segments: Vec::new(),
                connections: Vec::new(),
                error: None,
                failure: None,
                events: Vec::new(),
                server: None,
                at_service: None,
            }),
            seen: Default::default(),
            planner: Mutex::new(None),
            limiter: Limiter::new(None),
            best_speed: Default::default(),
            flushing: Default::default(),
            held: Default::default(),
        });
        let task = tokio::spawn(f(shared.clone(), cancel.clone()));
        Download {
            shared,
            cancel,
            task,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Change this download's speed limit (bytes/s, None = unlimited) while it runs.
    pub fn set_speed_limit(&self, rate: Option<u64>) {
        self.shared.limiter.set(rate);
    }

    pub fn speed_limit(&self) -> Option<u64> {
        self.shared.limiter.get()
    }

    pub async fn wait(self) -> Result<Outcome, Error> {
        match self.task.await {
            Ok(r) => r,
            Err(e) if e.is_cancelled() => Err(Error::Cancelled),
            Err(e) => std::panic::resume_unwind(e.into_panic()),
        }
    }
}

/// Download `url` to `path` and wait for it: MDM's own downloads that aren't in the list (its
/// helpers, its updates, a crash report's symbols) use the engine like every download. An
/// earlier attempt cut off at `path` resumes; `progress(done, total)` while it runs.
pub async fn fetch(
    url: Url,
    path: &Path,
    opts: Options,
    progress: impl Fn(u64, Option<u64>),
) -> Result<Outcome, Error> {
    let state = crate::state::state_path(path);
    let fresh = || {
        Download::start(
            Request {
                url: url.clone(),
                headers: HeaderMap::new(),
                dest: Destination::File(path.to_owned()),
            },
            opts.clone(),
        )
    };
    // A saved state for another address (an earlier release's file) is not resumed.
    let dl = match crate::state::ResumeState::load(&state) {
        Ok(saved) if saved.url == url => {
            Download::resume(state, HeaderMap::new(), opts.clone()).unwrap_or_else(|_| fresh())
        }
        _ => fresh(),
    };
    loop {
        let s = dl.snapshot();
        progress(s.downloaded, s.size);
        if matches!(s.state, State::Done | State::Failed | State::Cancelled) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    dl.wait().await
}

struct Shared {
    view: Mutex<Snapshot>,
    /// What the download's client learnt of its server (#140); the look's when it went on
    /// from the look's answer.
    seen: std::sync::OnceLock<Arc<crate::server::ServerSeen>>,
    planner: Mutex<Option<Planner>>,
    limiter: Limiter,
    /// The best speed one connection of this download sustained (≥ 1 s), bytes/s: what a good
    /// connection to this server does, the yardstick for shedding.
    best_speed: std::sync::atomic::AtomicU64,
    /// The resume save is flushing the part file to the disk right now (`save_state`): the
    /// connections hold what they receive (`hold_cap`).
    flushing: std::sync::atomic::AtomicBool,
    /// Bytes received and claimed but not yet written, in the connections' buffers: shown as
    /// downloaded (they are), but never saved as such.
    held: std::sync::atomic::AtomicU64,
}

impl Shared {
    fn snapshot(&self) -> Snapshot {
        let mut s = self.view.lock().unwrap().clone();
        s.server = self.seen.get().and_then(|seen| seen.get());
        if let Some(p) = self.planner.lock().unwrap().as_ref() {
            s.downloaded = p.downloaded() + self.held.load(Ordering::Relaxed);
            s.segments = p
                .segments()
                .iter()
                .map(|g| SegmentView {
                    start: g.start,
                    cursor: g.cursor,
                    end: g.end,
                })
                .collect();
        }
        s
    }

    fn set_state(&self, state: State) {
        self.view.lock().unwrap().state = state;
    }

    /// Add to the event log.
    fn event(&self, text: impl Into<String>) {
        let mut v = self.view.lock().unwrap();
        let seq = v.events.last().map_or(1, |e| e.seq + 1);
        if v.events.len() >= EVENTS {
            v.events.remove(0);
        }
        v.events.push(Event {
            seq,
            at: std::time::SystemTime::now(),
            text: text.into(),
        });
    }

    fn conn(&self, id: WorkerId, f: impl FnOnce(&mut ConnView)) {
        let mut v = self.view.lock().unwrap();
        let i = match v.connections.iter().position(|c| c.id == id) {
            Some(i) => i,
            None => {
                v.connections.push(ConnView {
                    id,
                    downloaded: 0,
                    status: ConnStatus::Connecting,
                    speed: 0,
                });
                v.connections.len() - 1
            }
        };
        f(&mut v.connections[i]);
    }

    fn with_planner<T>(&self, f: impl FnOnce(&mut Planner) -> T) -> T {
        f(self
            .planner
            .lock()
            .unwrap()
            .as_mut()
            .expect("planner exists while workers run"))
    }
}

/// The address ranges are fetched from, with the headers for it. `generation` counts switches.
#[derive(Clone)]
struct Target {
    url: Url,
    headers: HeaderMap,
    /// A POST download's body for this address (None: GET; see `post_for`).
    post: Option<Bytes>,
    generation: u64,
}

/// After asking the original address didn't help, how long until it may be asked again.
const REFRESH_EVERY: Duration = Duration::from_secs(120);

/// What every connection needs to fetch a segment.
struct Ctx {
    /// The download's client: every connection's requests go through its pool.
    client: Client,
    /// The file's version; each range request's `If-Range` is chosen from them.
    validators: Mutex<Validators>,
    /// Where ranges come from now (after redirects); switched when it expires.
    target: Mutex<Target>,
    /// The address the user gave and its headers, when it redirected: asked again when the
    /// target expires.
    origin: Option<(Url, HeaderMap)>,
    /// When asking the origin last failed to help (one refresh at a time; after a failure,
    /// none for `REFRESH_EVERY`).
    refreshed: tokio::sync::Mutex<Option<std::time::Instant>>,
    size: u64,
    file: Arc<File>,
    opts: Options,
    shared: Arc<Shared>,
    cancel: CancellationToken,
    /// `RANGES_*`: learned from the first range answer.
    ranges: AtomicU8,
    /// The first answer advertised ranges (`Accept-Ranges: bytes`, or a 206): a full answer to
    /// a range request with the same ETag is then an edge that missed the range this time, not
    /// a server without ranges (#136).
    advertised: bool,
    /// Further connections wait for a range answer (`range_answered`): a POST whose server
    /// hasn't shown yet that it takes ranges (`Options::post`).
    wait_for_ranges: bool,
    /// Signalled by every 206 answer.
    range_answered: tokio::sync::Notify,
    /// Where a POST download's redirects turned into GETs (refreshing the target).
    as_get: AsGet,
    /// The file itself is a web page; otherwise an HTML answer is a sign-in wall or an error.
    web_page_ok: bool,
    /// An FTP download's server and its idle sessions (else HTTP through `client`).
    ftp: Option<Arc<crate::ftp::Site>>,
    /// The browser-like connection every request goes through instead of `client`, when only it
    /// got past the site's bot check (#668).
    browser: Option<Arc<crate::browserlike::Browser>>,
}

/// An answer's body coming in: an HTTP response, or an FTP transfer.
pub(crate) enum Body {
    Http(Response),
    Ftp(crate::ftp::Transfer),
    /// An answer to the browser-like connection (#668, `crate::browserlike`).
    #[cfg_attr(not(feature = "browser-connection"), allow(dead_code))]
    Browser(crate::browserlike::Stream),
}

impl Body {
    /// The next bytes; None at the end.
    async fn chunk(&mut self) -> Result<Option<bytes::Bytes>, Error> {
        match self {
            Body::Http(r) => Ok(r.chunk().await?),
            Body::Ftp(t) => t.chunk().await,
            Body::Browser(b) => b.chunk().await,
        }
    }

    /// The bytes it will carry, if known.
    fn len(&self) -> Option<u64> {
        match self {
            Body::Http(r) => r.content_length(),
            Body::Ftp(t) => t.expected(),
            Body::Browser(b) => b.len(),
        }
    }

    /// Done with it, at its end or before: its connection may serve the next request.
    async fn done(self, at_end: bool) {
        match self {
            // Read the end of the body so the connection goes back to the pool.
            Body::Http(mut r) => {
                if !at_end {
                    let _ = r.chunk().await;
                }
            }
            Body::Ftp(t) => t.done(at_end).await,
            // Its connection isn't pooled for another request: there is only the one.
            Body::Browser(_) => {}
        }
    }
}

/// What the engine reads of an HTTP answer's head, from either client (reqwest's, or the
/// browser-like one's, #668).
#[derive(Clone, Debug)]
pub(crate) struct Head {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    /// Where the answer came from, its redirects followed.
    pub(crate) url: Url,
    pub(crate) len: Option<u64>,
}

impl Head {
    fn of(resp: &Response) -> Head {
        Head {
            status: resp.status(),
            headers: resp.headers().clone(),
            url: resp.url().clone(),
            len: resp.content_length(),
        }
    }
}

impl Ctx {
    fn target(&self) -> Target {
        self.target.lock().unwrap().clone()
    }

    /// After `generation`'s address answered as a dead link: ask the original address for
    /// today's target (once for all connections, at most every `REFRESH_EVERY`) and switch to
    /// it if it is the same file. True when there is a newer target to retry with.
    async fn refresh_target(&self, generation: u64) -> bool {
        let Some((origin, headers)) = &self.origin else {
            return false;
        };
        let mut failed_at = self.refreshed.lock().await;
        if self.target.lock().unwrap().generation != generation {
            return true; // another connection already switched
        }
        // Signed links may live minutes while the download takes hours: refresh as often as
        // that helps, but after an unhelpful refresh wait before asking again.
        if failed_at.is_some_and(|t| t.elapsed() < REFRESH_EVERY) {
            return false;
        }
        let Some((url, mut headers)) = refresh_final_url(
            &self.client,
            origin,
            headers,
            self.opts.post.as_ref(),
            self.opts.site_agent(),
            self.size,
            &self.cancel,
            self.opts.unlocker.as_ref(),
            &self.shared,
        )
        .await
        .filter(|(u, _)| *u != self.target.lock().unwrap().url) else {
            *failed_at = Some(std::time::Instant::now());
            return false;
        };
        *failed_at = None;
        self.shared.event(format!(
            "The address stopped working; the original address now leads to {}",
            url.host_str().unwrap_or("?")
        ));
        let post = post_for(&self.opts, self.as_get.contains(&url), &mut headers);
        let mut t = self.target.lock().unwrap();
        t.headers = headers;
        t.post = post;
        t.url = url;
        t.generation += 1;
        true
    }

    fn learn_ranges(&self, yes: bool) {
        let v = if yes { RANGES_YES } else { RANGES_NO };
        if self
            .ranges
            .compare_exchange(RANGES_UNKNOWN, v, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.shared.view.lock().unwrap().resumable = Some(yes);
        }
    }
}

/// How long an idle keep-alive socket is kept: shorter than most servers' keep-alive, so a
/// paused download doesn't come back to sockets the server already closed.
const POOL_IDLE: Duration = Duration::from_secs(15);

/// The client for a download of `url`; a POST download's redirects are noted in `as_get`.
fn client(
    opts: &Options,
    url: &Url,
    as_get: &AsGet,
    seen: &Arc<crate::server::ServerSeen>,
) -> Result<Client, Error> {
    let port = url.port_or_known_default().unwrap_or(443);
    let route = &opts.route;
    // An IP address in the URL never reaches the resolver, which drops what the chosen adapter
    // can't reach (#35): check it here.
    net::check_url(route, url)?;
    let mut builder = Client::builder();
    if !opts.extra_roots.is_empty() {
        builder = builder.tls_certs_merge(
            opts.extra_roots
                .iter()
                .filter_map(|der| reqwest::Certificate::from_der(der).ok()),
        );
    }
    // With an adapter chosen, the system's proxy is looked up now, so that it can be checked
    // (and a proxy on this computer recognised) like the others.
    let system = match (&opts.proxy, route.is_any()) {
        (Proxy::System, false) => Some(net::system_proxy(url)),
        _ => None,
    };
    // The proxy that carries every request of this client, when it's on this computer: such a
    // connection isn't bound; where the proxy sends the traffic is up to it.
    let mut through_local_proxy = false;
    let mut check_proxy = |proxy: &str, covers_all: bool| -> Result<(), Error> {
        let host = Url::parse(proxy)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        if net::is_this_computer(&host) {
            through_local_proxy |= covers_all;
        } else if let Ok(ip) = host.trim_matches(['[', ']']).parse() {
            route.check(ip)?;
        }
        Ok(())
    };
    // Whether a proxy carries this download (#140: the address that answers is then the
    // proxy's): known here for each kind of setting.
    let mut proxied = false;
    let builder = match (&opts.proxy, system) {
        (Proxy::None, _) => builder.no_proxy(),
        (Proxy::System, Some(Some(proxy))) => {
            check_proxy(&proxy, true)?;
            proxied = true;
            builder.no_proxy().proxy(reqwest::Proxy::all(&proxy)?)
        }
        (Proxy::System, Some(None)) => builder.no_proxy(),
        // reqwest reads the system's proxy by default.
        (Proxy::System, None) => {
            proxied = net::system_proxy(url).is_some();
            builder
        }
        (
            Proxy::Manual {
                url: proxy,
                login,
                http,
                https,
            },
            _,
        ) => {
            let with_login = |p: reqwest::Proxy| match login {
                Some((user, pass)) => p.basic_auth(user, pass),
                None => p,
            };
            if *http || *https {
                check_proxy(proxy, *http && *https)?;
            }
            proxied = if url.scheme() == "https" {
                *https
            } else {
                *http
            };
            let mut b = builder.no_proxy();
            if *http {
                b = b.proxy(with_login(reqwest::Proxy::http(proxy.as_str())?));
            }
            if *https {
                b = b.proxy(with_login(reqwest::Proxy::https(proxy.as_str())?));
            }
            b
        }
        // The script's first route MDM can use: a proxy, or straight to the server.
        (Proxy::Pac { script }, _) => {
            let first = crate::pac::routes(script, url).ok().and_then(|routes| {
                routes.into_iter().find_map(|r| match r {
                    crate::pac::Route::Direct => Some(None),
                    crate::pac::Route::Proxy(p) => reqwest::Proxy::all(&p).ok().map(|_| Some(p)),
                })
            });
            // No usable answer: the system's proxy, as reqwest reads it (with an adapter
            // chosen, looked up now to be checked).
            let first = match first {
                None if !route.is_any() => Some(net::system_proxy(url)),
                other => other,
            };
            match first {
                Some(Some(p)) => {
                    check_proxy(&p, true)?;
                    proxied = true;
                    builder.no_proxy().proxy(reqwest::Proxy::all(&p)?)
                }
                Some(None) => builder.no_proxy(),
                None => {
                    proxied = net::system_proxy(url).is_some();
                    builder
                }
            }
        }
    };
    // A connection to this computer isn't bound (#35); its redirects may then only lead back
    // to this computer, or they would leave unbound.
    let to_this_computer = through_local_proxy
        || (net::is_this_computer(url.host_str().unwrap_or_default())
            && !matches!(opts.proxy, Proxy::Manual { .. } | Proxy::Pac { .. }));
    let builder = net::bind_client(builder, route, to_this_computer)?;
    let inner = match opts.post {
        Some(_) => as_get.policy(),
        None => get_policy(),
    };
    let builder = if route.is_any() {
        builder.redirect(inner)
    } else {
        let route = route.clone();
        let stay_here = to_this_computer && !through_local_proxy;
        builder.redirect(reqwest::redirect::Policy::custom(move |attempt| {
            let refused = net::check_url(&route, attempt.url()).err().or_else(|| {
                (stay_here && !net::is_this_computer(attempt.url().host_str().unwrap_or(""))).then(
                    || net::NetError::Unbound {
                        label: route.label().unwrap_or_default().to_owned(),
                    },
                )
            });
            match refused {
                Some(e) => attempt.error(e),
                None => inner.redirect(attempt),
            }
        }))
    };
    seen.set_proxied(proxied);
    Ok(builder
        // Each connection's opening timed (#140): the download's own, no extra probe.
        .connector_layer(crate::server::Timed(seen.clone()))
        .dns_resolver(Arc::new(dns::Resolver::new(
            &opts.hosts,
            port,
            &opts.dns,
            &opts.extra_roots,
            route,
        )))
        .http1_only()
        .user_agent(&opts.user_agent)
        .connect_timeout(opts.connect_timeout)
        .read_timeout(opts.read_timeout)
        .pool_max_idle_per_host(opts.connections() as usize)
        .pool_idle_timeout(POOL_IDLE)
        .tcp_nodelay(true)
        .build()?)
}

/// Runs a download body and records its final state.
async fn run(
    body: impl Future<Output = Result<Outcome, Error>>,
    shared: Arc<Shared>,
) -> Result<Outcome, Error> {
    let result = body.await;
    shared.event(match &result {
        Ok(o) => format!("Complete: {}", o.path.display()),
        Err(Error::Cancelled) => "Stopped".to_owned(),
        Err(e) => format!("Failed: {e}"),
    });
    let mut v = shared.view.lock().unwrap();
    v.state = match &result {
        Ok(o) => {
            // Where it really is: `finish` numbers the name when another file took it meanwhile,
            // and whoever reads the view (the app's row) goes by this.
            v.file = Some(o.path.clone());
            State::Done
        }
        Err(Error::Cancelled) => State::Cancelled,
        Err(e) => {
            v.error = Some(e.to_string());
            v.failure = Some(e.clone());
            State::Failed
        }
    };
    drop(v);
    result
}

async fn run_fresh(
    req: Request,
    opts: Options,
    shared: Arc<Shared>,
    cancel: CancellationToken,
    kept: Option<KeptAnswer>,
) -> Result<Outcome, Error> {
    let mut opts = opts;
    // The first request had to give up the browser's User-Agent (#372).
    let mut asked_as_mdm = false;
    shared.limiter.set(opts.speed_limit);
    let (shared, cancel) = (&shared, &cancel);
    shared.conn(0, |_| {});
    // Probe on connection 0: a plain GET (or the form's POST) whose body becomes the segment
    // starting at 0; or the look's answer kept for it (#28), not asked for again.
    // A web page where the download expects another kind of file is an error (#533): the look's
    // answer, asked before the file's name was settled, is asked for again then (`get` says so).
    let expects = expects_other_file(
        opts.expected_type.as_deref(),
        dest_name(&req.dest),
        &req.url,
    );
    let kept = kept.filter(|k| {
        k.url == req.url
            && k.at.elapsed() < KEEP_ANSWER_FOR
            && !(expects && is_web_page(&k.head.headers) && !named(&k.head.headers))
    });
    let mut unlocked = kept.as_ref().and_then(|k| k.unlocked.clone());
    // The browser-like connection the download goes on with, when only it got through (#668).
    let mut browser: Option<Arc<crate::browserlike::Browser>> = None;
    let (as_get, client0, (head, body), headers, ttfb) = match kept {
        Some(k) => {
            shared.event(
                "Connection 1: continuing from the answer the file's information window got",
            );
            let _ = shared.seen.set(k.seen);
            browser = k.browser;
            (k.as_get, k.client, (k.head, k.body), k.headers, k.took)
        }
        None => {
            // Through a service account (#44): its direct link, without the host's cookies
            // or logins. The host's link stays the download's origin, asked again (through the
            // service) when the direct link dies.
            unlocked = unlock_origin(&opts, &req.url, Some(shared), cancel).await?;
            let (target, asked) = match &unlocked {
                Some(u) => (u.url.clone(), HeaderMap::new()),
                None => (req.url.clone(), req.headers.clone()),
            };
            let as_get = AsGet::default();
            let seen = shared.seen.get_or_init(Default::default).clone();
            let as_given = opts.clone();
            // The download's own User-Agent, which the browser-like connection carries (#668),
            // even when MDM's connection had to give it up (#372).
            let given_agent = opts.user_agent.clone();
            let ask = FirstAsk {
                as_get: &as_get,
                seen: &seen,
                expects,
                shared,
                cancel,
            };
            let (mut client0, mut first, as_mdm) = ask.at(&mut opts, &target, &asked).await?;
            asked_as_mdm = as_mdm;
            // #630: the link refused, where the browser's own download of it ended is asked
            // once, as the link was (the browser's User-Agent first). The link's refusal is
            // the one told if that fails too.
            if let Err(e) = &first
                && link_refused(e)
                && unlocked.is_none()
                && opts.post.is_none()
                && let Some(other) = opts.browser_final.clone().filter(|o| *o != target)
            {
                shared.event(format!(
                    "{}: {e}; asking where the browser's download ended ({})",
                    target.host_str().unwrap_or("?"),
                    other.host_str().unwrap_or("?"),
                ));
                let mut again = as_given;
                let headers = scoped_headers(&asked, &target, &other);
                match ask.at(&mut again, &other, &headers).await {
                    Ok((client1, Ok(answer), as_mdm)) => {
                        (opts, client0, first, asked_as_mdm) = (again, client1, Ok(answer), as_mdm);
                    }
                    Ok((_, Err(Error::Cancelled), _)) | Err(Error::Cancelled) => {
                        return Err(Error::Cancelled);
                    }
                    Ok((_, Err(e), _)) | Err(e) => {
                        shared.event(format!("{}: {e}", other.host_str().unwrap_or("?")));
                    }
                }
            }
            // #668: the site refused MDM's connection as a program's (its bot check, or a 412):
            // asked once more, the way a browser connects, at the same address with the same
            // headers. Its refusal stays the one told if that fails too.
            match first {
                Ok(((resp, headers), ttfb)) => {
                    seen.answered(&resp);
                    let head = Head::of(&resp);
                    (as_get, client0, (head, Body::Http(resp)), headers, ttfb)
                }
                Err(e) if refused_as_a_program(&e) && unlocked.is_none() && opts.post.is_none() => {
                    if asked_as_mdm {
                        opts.user_agent = given_agent;
                        asked_as_mdm = false;
                    }
                    let (b, head, body, headers, ttfb) =
                        browser_first(&opts, &target, &asked, e, expects, shared, cancel).await?;
                    browser = Some(b);
                    (as_get, client0, (head, body), headers, ttfb)
                }
                Err(e) => return Err(e),
            }
        }
    };
    // One connection, as a browser downloads: the site let this one through.
    if browser.is_some() {
        opts.max_connections = 1;
    }
    // The service's own cap on connections to its link (Real-Debrid's "chunks").
    if let Some(cap) = unlocked
        .as_ref()
        .and_then(|u| u.max_connections)
        .filter(|c| *c >= 1)
    {
        opts.max_connections = opts.max_connections.min(cap);
    }
    let preempt_after = (ttfb * PREEMPT_TTFB).max(PREEMPT_MIN);
    let size = match head.status {
        StatusCode::OK => head.len,
        // Some servers answer partially even without Range; accept it if it starts at 0.
        StatusCode::PARTIAL_CONTENT => match parse_content_range(&head.headers)? {
            (0, _, total) => total,
            (start, ..) => return Err(Error::BadRange(format!("probe started at {start}"))),
        },
        s => return Err(Error::Status(s)),
    };
    let ranges = if head.status == StatusCode::PARTIAL_CONTENT {
        RANGES_YES
    } else {
        RANGES_UNKNOWN
    };
    // The first answer says the server takes ranges (`Accept-Ranges: bytes`).
    let advertised = ranges == RANGES_YES
        || header_str(&head.headers, header::ACCEPT_RANGES)
            .is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("bytes")));
    let final_url = head.url.clone();
    shared.event(format!(
        "Connected to {}{}: {}, size {}",
        final_url.host_str().unwrap_or("?"),
        if final_url != req.url {
            " (redirected)"
        } else {
            ""
        },
        head.status,
        size.map_or("unknown".to_owned(), disk::human),
    ));
    // Credentials stay with the host they were given for, with the cookies pages set on the way
    // (`get`).
    let mut headers = headers;
    // A POST stays one if its redirects kept it (307, 308); it is sent again only once the
    // server has answered a range (206): `Accept-Ranges` alone isn't proof, and a form may do
    // something each time it is posted (an export, an order).
    let post = post_for(&opts, as_get.contains(&final_url), &mut headers);
    let wait_for_ranges = post.is_some() && ranges != RANGES_YES;
    let h = &head.headers;
    // The expected hash: the user's, else what a full (200) answer announces for its content.
    let checksum = opts.checksum.clone().or_else(|| {
        (head.status == StatusCode::OK)
            .then(|| crate::verify::Hash::from_headers(h))
            .flatten()
    });
    let validators = Validators::from_response(
        header_str(h, header::ETAG),
        header_str(h, header::LAST_MODIFIED),
        header_str(h, header::DATE),
        final_url.host_str(),
    );
    // The server's date for the finished file (Options::server_time): its Last-Modified as
    // sent, whether or not it is strong enough to resume by.
    let stamp = opts
        .server_time
        .then(|| header_str(h, header::LAST_MODIFIED))
        .flatten()
        .and_then(|lm| httpdate::parse_http_date(lm).ok());

    let content_type = head
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let (dest, _reservation, file) = open_destination(&req.dest, size, &opts, || {
        let cd = header_text(&head.headers, header::CONTENT_DISPOSITION);
        name_for(cd.as_deref(), &final_url, unlocked.as_ref())
    })
    .await?;
    downloading(shared, &dest, size, ranges);
    let file = Arc::new(file);

    let size = if let Some(size) = size {
        let mut planner = Planner::new(size, opts.min_split);
        planner.set_preempt_after(preempt_after);
        *shared.planner.lock().unwrap() = Some(planner);
        let mut saved = ResumeState::new(
            req.url.clone(),
            final_url.clone(),
            dest.clone(),
            size,
            validators.clone(),
        );
        saved.content_type = content_type.clone();
        saved.checksum = checksum.clone();
        saved.final_get = opts.post.is_some() && post.is_none();
        saved.asked_as_mdm = asked_as_mdm;
        saved.browser_like = browser.is_some();
        let ctx = Arc::new(Ctx {
            client: client0.clone(),
            validators: Mutex::new(validators),
            target: Mutex::new(Target {
                url: final_url.clone(),
                headers: headers.clone(),
                post: post.clone(),
                generation: 0,
            }),
            // Asked again through MDM's own connection, which the site's bot check refuses: a
            // browser-like download keeps the address it has (#668).
            origin: (req.url != final_url && browser.is_none())
                .then(|| (req.url.clone(), req.headers.clone())),
            refreshed: Default::default(),
            size,
            file: file.clone(),
            opts: opts.clone(),
            shared: shared.clone(),
            // A child token: the supervisor stops its own connections without cancelling the
            // download (which may still fall back to one connection).
            cancel: cancel.child_token(),
            ranges: AtomicU8::new(ranges),
            advertised,
            wait_for_ranges,
            range_answered: Default::default(),
            as_get,
            web_page_ok: expects_web_page(&dest, content_type.as_deref()),
            ftp: None,
            browser: browser.clone(),
        });
        match segmented(ctx, saved, Some(body)).await {
            Ok(()) => size,
            // The server ignores ranges and the probe's data can't be kept: start over on
            // one connection.
            Err(Error::NoRanges) => {
                *shared.planner.lock().unwrap() = None;
                shared.view.lock().unwrap().resumable = Some(false);
                let body = match &browser {
                    Some(b) => {
                        let (head, body, _) = b.get(final_url, headers, false, cancel).await?;
                        crate::browserlike::check_first(&head, expects)?;
                        body
                    }
                    None => {
                        let agent = opts.site_agent();
                        let (resp, ..) = get(
                            &client0,
                            final_url,
                            headers,
                            post.as_ref(),
                            agent,
                            expects,
                            cancel,
                        )
                        .await?;
                        Body::Http(resp)
                    }
                };
                single_stream(body, &file, shared, opts.global.as_deref(), cancel).await?
            }
            Err(e) => return Err(e),
        }
    } else {
        single_stream(body, &file, shared, opts.global.as_deref(), cancel).await?
    };
    verify_part(shared, &dest, checksum.as_ref()).await?;
    finish(shared, file, &dest, size, stamp)
}

/// Prepare the destination of a new download of `size` bytes (if known), named `name()` in a
/// chosen folder: the folder exists, the file fits on the drive, the name is reserved in this
/// process, and the part file is created (exclusively, for a chosen folder: another program
/// may have taken the name meanwhile) at its full size, and shown to `opts.on_file_created`.
async fn open_destination(
    dest: &Destination,
    size: Option<u64>,
    opts: &Options,
    name: impl FnOnce() -> String,
) -> Result<(PathBuf, naming::Reservation, File), Error> {
    // The folder must exist: we create it, but a removed drive or offline share can't be.
    let folder = match dest {
        Destination::File(p) | Destination::Free(p) => {
            p.parent().unwrap_or(Path::new("")).to_owned()
        }
        Destination::Dir(dir) => dir.clone(),
    };
    std::fs::create_dir_all(&folder).map_err(|_| Error::DestinationUnavailable(folder.clone()))?;
    if let Destination::File(p) | Destination::Free(p) = dest
        && !naming::path_fits(p)
    {
        return Err(Error::PathTooLong(p.clone()));
    }
    // Will it fit? Refuse now rather than fail half-way (a drive we can't identify is skipped).
    if let Some(size) = size {
        // Off the async threads, and bounded: a stale network drive can hang the volume probe.
        if let Some(volume) = disk::probe(folder).await {
            disk::check(&volume, size)?;
        }
    }
    let (dest, reservation, file) = match dest {
        Destination::File(p) => {
            let r = naming::reserve(p).ok_or_else(|| Error::InUse(p.clone()))?;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(naming::part_path(p))?;
            (p.clone(), r, file)
        }
        Destination::Dir(dir) => {
            let name = name();
            // Shortened to fit Windows' path limit; a folder too deep for any name is an error.
            let name =
                naming::fit_name(dir, &name).ok_or_else(|| Error::PathTooLong(dir.join(&name)))?;
            claim_free(dir, &name)?
        }
        Destination::Free(p) => {
            let dir = p.parent().unwrap_or(Path::new(""));
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            claim_free(dir, &name)?
        }
    };
    // Before any data: a mark the hook sets (Mark of the Web, quarantine) is on the file from
    // its first byte, and the final rename keeps it.
    if let Some(hook) = &opts.on_file_created {
        (hook.0)(&naming::part_path(&dest));
    }
    if let Some(size) = size {
        // Sparse first: writes far into a merely extended file make NTFS zero-fill up to them.
        if size >= sparse::MIN_SIZE {
            sparse::set(&file, true);
        }
        file.set_len(size)?;
    }
    Ok((dest, reservation, file))
}

/// `name` in `dir`, or the next free numbered name (`naming::unique_path`): reserved in this
/// process, its part file created exclusively (another program may take the name meanwhile).
fn claim_free(dir: &Path, name: &str) -> Result<(PathBuf, naming::Reservation, File), Error> {
    let mut attempt = 0;
    loop {
        let dest = naming::unique_path(dir, name);
        if let Some(r) = naming::reserve(&dest) {
            // Read too: reconnecting connections re-read their last bytes (overlap check).
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(naming::part_path(&dest))
            {
                Ok(file) => return Ok((dest, r, file)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {}
                Err(e) => return Err(e.into()),
            }
        }
        attempt += 1;
    }
}

/// The download has its file and is receiving it.
fn downloading(shared: &Shared, dest: &Path, size: Option<u64>, ranges: u8) {
    let mut v = shared.view.lock().unwrap();
    v.state = State::Downloading;
    v.file = Some(dest.to_owned());
    v.size = size;
    v.resumable = (ranges == RANGES_YES)
        .then_some(true)
        .or(size.is_none().then_some(false));
}

/// A new FTP download: as `run_fresh`, with the server's `SIZE` and `MDTM` for the answer's
/// headers and `REST` for ranges.
async fn run_fresh_ftp(
    req: Request,
    opts: Options,
    shared: Arc<Shared>,
    cancel: CancellationToken,
) -> Result<Outcome, Error> {
    shared.limiter.set(opts.speed_limit);
    let (shared, cancel) = (&shared, &cancel);
    let site = Arc::new(crate::ftp::Site::new(&req.url, &req.headers, &opts)?);
    // Connection 0 asks about the file and receives it from byte 0 (the probe).
    shared.conn(0, |_| {});
    let ((facts, first), ttfb) = first_try(&opts, shared, cancel, || site.start()).await?;
    let preempt_after = (ttfb * PREEMPT_TTFB).max(PREEMPT_MIN);
    let size = facts.size;
    shared.event(format!(
        "Connected to {} ({}), size {}",
        req.url.host_str().unwrap_or("?"),
        match req.url.scheme() {
            "ftp" => "FTP",
            _ => "FTPS",
        },
        size.map_or("unknown".to_owned(), disk::human),
    ));
    let ranges = if facts.rest {
        RANGES_YES
    } else {
        RANGES_UNKNOWN
    };
    // The modification time stands in for Last-Modified: resume checks it.
    let validators = Validators {
        last_modified: facts.modified.map(httpdate::fmt_http_date),
        ..Validators::default()
    };
    let stamp = opts.server_time.then_some(facts.modified).flatten();
    let (dest, _reservation, file) =
        open_destination(&req.dest, size, &opts, || naming::file_name(None, &req.url)).await?;
    downloading(shared, &dest, size, ranges);
    let file = Arc::new(file);
    let first = Body::Ftp(first);
    let size = if let Some(size) = size {
        let mut planner = Planner::new(size, opts.min_split);
        planner.set_preempt_after(preempt_after);
        *shared.planner.lock().unwrap() = Some(planner);
        // No password on disk: whoever resumes gives the login again.
        let url = crate::ftp::without_password(&req.url);
        let mut saved = ResumeState::new(
            url.clone(),
            url.clone(),
            dest.clone(),
            size,
            validators.clone(),
        );
        saved.checksum = opts.checksum.clone();
        let ctx = Arc::new(Ctx {
            // FTP's connections are its own (`ftp`): this client's are none of the server's.
            client: client(&opts, &req.url, &AsGet::default(), &Default::default())?,
            validators: Mutex::new(validators),
            target: Mutex::new(Target {
                url,
                headers: HeaderMap::new(),
                post: None,
                generation: 0,
            }),
            origin: None,
            refreshed: Default::default(),
            size,
            file: file.clone(),
            opts: opts.clone(),
            shared: shared.clone(),
            cancel: cancel.child_token(),
            ranges: AtomicU8::new(ranges),
            // FTP's REST is asked for, not advertised.
            advertised: true,
            wait_for_ranges: false,
            range_answered: Default::default(),
            as_get: AsGet::default(),
            web_page_ok: true,
            ftp: Some(site.clone()),
            browser: None,
        });
        match segmented(ctx, saved, Some(first)).await {
            Ok(()) => size,
            // REST refused: the probe's data can't be kept; the whole file on one connection.
            Err(Error::NoRanges) => {
                *shared.planner.lock().unwrap() = None;
                shared.view.lock().unwrap().resumable = Some(false);
                let t = cancellable(cancel, site.open_at(0, Some(size))).await?;
                single_stream(Body::Ftp(t), &file, shared, opts.global.as_deref(), cancel).await?
            }
            Err(e) => return Err(e),
        }
    } else {
        single_stream(first, &file, shared, opts.global.as_deref(), cancel).await?
    };
    verify_part(shared, &dest, opts.checksum.as_ref()).await?;
    finish(shared, file, &dest, size, stamp)
}

async fn run_resume(
    saved: ResumeState,
    state_file: PathBuf,
    mut headers: HeaderMap,
    opts: Options,
    shared: Arc<Shared>,
    cancel: CancellationToken,
) -> Result<Outcome, Error> {
    let mut opts = opts;
    if saved.asked_as_mdm
        && let Some(agent) = opts.agent_fallback.take()
    {
        opts.user_agent = agent;
    }
    shared.limiter.set(opts.speed_limit);
    let bad = |what: &str| Error::BadState(format!("{}: {what}", state_file.display()));
    if state_file != state_path(&saved.dest) {
        return Err(bad("does not belong to the file it names"));
    }
    let _reservation =
        naming::reserve(&saved.dest).ok_or_else(|| Error::InUse(saved.dest.clone()))?;
    let part = naming::part_path(&saved.dest);
    // A crash between the final rename and removing the state file: the download is complete.
    if !part.exists() && std::fs::metadata(&saved.dest).is_ok_and(|m| m.len() == saved.size) {
        let _ = std::fs::remove_file(&state_file);
        shared.set_state(State::Completing);
        return Ok(Outcome {
            path: saved.dest.clone(),
            size: saved.size,
        });
    }
    // Signed links from a redirect expire: ask the original address again for today's target
    // (it must still be the same file), else keep the stored one.
    let as_get = AsGet::default();
    let seen = shared.seen.get_or_init(Default::default).clone();
    let client = client(&opts, &saved.url, &as_get, &seen)?;
    // Only the browser-like connection got past the site's bot check (#668): it goes on with
    // it, on one connection, at the address it had (MDM's own connection would be refused).
    let browser = match saved.browser_like && crate::browserlike::AVAILABLE {
        true => Some(Arc::new(crate::browserlike::Browser::new(
            &opts,
            &saved.final_url,
            &headers,
        )?)),
        false => None,
    };
    if browser.is_some() {
        opts.max_connections = 1;
        shared.event("Going on the way a browser connects, as the site's bot check asked");
    }
    // Through a service account (#44), the service is asked for today's link even when the
    // state holds none of its links (the download went directly until now).
    let refreshed =
        if browser.is_none() && (saved.url != saved.final_url || opts.unlocker.is_some()) {
            let post = opts.post.as_ref();
            let agent = opts.site_agent();
            refresh_final_url(
                &client,
                &saved.url,
                &headers,
                post,
                agent,
                saved.size,
                &cancel,
                opts.unlocker.as_ref(),
                &shared,
            )
            .await
        } else {
            None
        };
    // A POST download's target is asked with GET when its redirects turned it into one.
    let (url, final_get, refreshed_headers) = match refreshed {
        Some((url, h)) => (url.clone(), as_get.contains(&url), Some(h)),
        None => (saved.final_url.clone(), saved.final_get, None),
    };
    let origin =
        (saved.url != url && browser.is_none()).then(|| (saved.url.clone(), headers.clone()));
    // Credentials stay with the host they were given for (the refresh's own way there scoped
    // them as it went).
    headers = refreshed_headers.unwrap_or_else(|| scoped_headers(&headers, &saved.url, &url));
    let post = post_for(&opts, final_get, &mut headers);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&part)
        .map_err(|e| bad(&format!("part file: {e}")))?;
    if file.metadata()?.len() != saved.size {
        return Err(bad("part file has the wrong size"));
    }
    // FTP: the server's size and modification time must still be those of the data on disk.
    let ftp = if crate::ftp::is_ftp(&url) {
        let site = Arc::new(crate::ftp::Site::new(&url, &headers, &opts)?);
        match cancellable(&cancel, site.check()).await {
            Ok(facts) => {
                let modified = facts.modified.map(httpdate::fmt_http_date);
                let time_changed = !opts.ignore_mtime
                    && modified.is_some()
                    && saved.validators.last_modified.is_some()
                    && modified != saved.validators.last_modified;
                if time_changed || facts.size.is_some_and(|n| n != saved.size) {
                    return Err(Error::FileChanged);
                }
            }
            // Not answering now: the connections retry, and each checks its overlap.
            Err(e) if e.transient() => {}
            Err(e) => return Err(e),
        }
        Some(site)
    } else {
        None
    };
    shared.event(format!(
        "Resuming: {} of {} on disk, from {}",
        disk::human(saved.downloaded()),
        disk::human(saved.size),
        url.host_str().unwrap_or("?"),
    ));
    let mut planner = Planner::resume(saved.size, opts.min_split, &saved.segments)
        .map_err(|_| bad("segments do not cover the file"))?;
    planner.set_preempt_after(PREEMPT_MIN);
    *shared.planner.lock().unwrap() = Some(planner);
    {
        let mut v = shared.view.lock().unwrap();
        v.state = State::Downloading;
        v.file = Some(saved.dest.clone());
        v.size = Some(saved.size);
        v.resumable = Some(true);
    }
    let file = Arc::new(file);
    let (dest, size) = (saved.dest.clone(), saved.size);
    let checksum = opts.checksum.clone().or_else(|| saved.checksum.clone());
    let web_page_ok = expects_web_page(&dest, saved.content_type.as_deref());
    let ctx = Arc::new(Ctx {
        client,
        validators: Mutex::new({
            let mut v = saved.validators.clone();
            if opts.ignore_mtime {
                v.last_modified = None;
            }
            v
        }),
        // A POST's ranges may never have been tried (it came on one connection): one
        // connection asks first, the others follow its 206.
        wait_for_ranges: post.is_some() && ftp.is_none(),
        target: Mutex::new(Target {
            url,
            headers,
            post,
            generation: 0,
        }),
        origin,
        refreshed: Default::default(),
        size,
        file: file.clone(),
        opts,
        shared: shared.clone(),
        cancel: cancel.child_token(),
        // Ranges worked before: a 200 now is a changed file or a range-less edge.
        ranges: AtomicU8::new(RANGES_YES),
        advertised: true,
        range_answered: Default::default(),
        as_get,
        web_page_ok,
        ftp,
        browser,
    });
    let stamp = ctx
        .opts
        .server_time
        .then(|| ctx.validators.lock().unwrap().last_modified.clone())
        .flatten()
        .and_then(|lm| httpdate::parse_http_date(&lm).ok());
    segmented(ctx, saved, None).await?;
    verify_part(&shared, &dest, checksum.as_ref()).await?;
    finish(&shared, file, &dest, size, stamp)
}

/// Follow the original address again (with its own headers, and a POST download's body) as the
/// first request did (`get`: redirects, waiting pages, a site's own steps) and return where it
/// leads now, if that is still the same file: a ranged answer with the saved size; with the
/// headers to go on with there. None keeps the stored address (no redirect any more, a different
/// file, or the site is unreachable).
#[allow(clippy::too_many_arguments)]
async fn refresh_final_url(
    client: &Client,
    origin: &Url,
    headers: &HeaderMap,
    post: Option<&Bytes>,
    site_agent: bool,
    size: u64,
    cancel: &CancellationToken,
    unlocker: Option<&Arc<dyn crate::unlock::Unlocker>>,
    shared: &Shared,
) -> Option<(Url, HeaderMap)> {
    // Through a service account (#44): the service gives today's direct link (the host's link
    // itself leads to its web page); the host's cookies and logins stay with the host.
    let unlocked = match unlocker.filter(|_| post.is_none()) {
        Some(u) => unlock_noted(u, origin, Some(shared), cancel)
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let (target, mut asked) = match &unlocked {
        Some(u) => (u.url.clone(), HeaderMap::new()),
        None => (origin.clone(), headers.clone()),
    };
    asked.insert(header::RANGE, HeaderValue::from_static("bytes=0-0"));
    let (resp, mut headers, _) = get(client, target, asked, post, site_agent, false, cancel)
        .await
        .ok()?;
    headers.remove(header::RANGE);
    if resp.status() != StatusCode::PARTIAL_CONTENT {
        return None;
    }
    let (_, _, total) = parse_content_range(resp.headers()).ok()?;
    (total == Some(size)).then(|| (resp.url().clone(), headers))
}

/// Check the finished part file against `hash` (at most two files at once: it reads them
/// whole). No hash, nothing to do.
async fn verify_part(
    shared: &Shared,
    dest: &Path,
    hash: Option<&crate::verify::Hash>,
) -> Result<(), Error> {
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let Some(hash) = hash.cloned() else {
        return Ok(());
    };
    shared.set_state(State::Verifying);
    let _slot = SLOTS.acquire().await.map_err(|_| Error::Cancelled)?;
    let part = naming::part_path(dest);
    let algo = hash.algo;
    let ok = tokio::task::spawn_blocking(move || crate::verify::matches(&part, &hash))
        .await
        .expect("verifier panicked")?;
    if ok {
        shared.event(format!("Checksum verified ({})", algo.name()));
        Ok(())
    } else {
        Err(Error::ChecksumMismatch(algo.name()))
    }
}

/// Flush and rename the part file to its final name.
fn finish(
    shared: &Shared,
    file: Arc<File>,
    dest: &Path,
    size: u64,
    stamp: Option<std::time::SystemTime>,
) -> Result<Outcome, Error> {
    shared.set_state(State::Completing);
    let file = Arc::try_unwrap(file).map_err(|_| io::Error::other("part file still in use"))?;
    file.set_len(size)?;
    // Every byte is written: a normal file again (nothing left to allocate).
    if size >= sparse::MIN_SIZE {
        sparse::set(&file, false);
    }
    file.sync_all()?;
    // The server's date (Options::server_time), set before the rename moves the file.
    if let Some(t) = stamp {
        let times = std::fs::FileTimes::new().set_modified(t).set_accessed(t);
        #[cfg(windows)]
        let times = {
            use std::os::windows::fs::FileTimesExt;
            times.set_created(t)
        };
        let _ = file.set_times(times);
    }
    drop(file);
    let part = naming::part_path(dest);
    // Never replace a file that appeared at the destination meanwhile (rename replaces on
    // Windows): take the next free numbered name instead.
    let original = dest;
    let dest = if dest.exists() {
        let dir = dest.parent().unwrap_or(Path::new("."));
        let name = dest
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download");
        naming::unique_path(dir, name)
    } else {
        dest.to_owned()
    };
    // Antivirus scanners and indexers briefly lock new files on Windows: retry the rename.
    craft_atomic::retry_rename(craft_atomic::RenameRetry::platform(), || {
        std::fs::rename(&part, &dest)
    })?;
    // Only now is the resume state obsolete.
    let _ = std::fs::remove_file(state_path(original));
    Ok(Outcome { path: dest, size })
}

/// Run the connections with the resume state saved every `save_interval`, and once more when
/// they stop. The state file is removed when the download completes.
async fn segmented(
    ctx: Arc<Ctx>,
    mut saved: ResumeState,
    first: Option<Body>,
) -> Result<(), Error> {
    let path = state_path(&saved.dest);
    let mut synced = Synced {
        at: std::time::Instant::now(),
        bytes: 0,
        took: Duration::ZERO,
    };
    save_state(&ctx, &mut saved, &path, &mut synced, true).await?;
    let interval = ctx.opts.save_interval;
    let result = {
        // Saves until told to stop or a save fails (resolving to that error). It stops only
        // between saves: a save's blocking write can't be cancelled, so dropping the saver
        // mid-save would leave that write running after the download has returned, writing a
        // stale state file next to a finished or already resumed download (#106).
        let stop = CancellationToken::new();
        let saver = async {
            loop {
                tokio::select! {
                    () = tokio::time::sleep(interval) => {}
                    () = stop.cancelled() => return None,
                }
                if let Err(e) = save_state(&ctx, &mut saved, &path, &mut synced, false).await {
                    return Some(e);
                }
            }
        };
        let supervisor = supervise(ctx.clone(), first);
        tokio::pin!(saver, supervisor);
        tokio::select! {
            r = &mut supervisor => {
                // The connections stopped: let a save under way finish. Its error, if any, is
                // the final save's to report (it writes the same file, forced, just below).
                stop.cancel();
                let _ = saver.await;
                r
            }
            e = &mut saver => Err(e.expect("the saver only stops when told to")),
        }
    };
    match &result {
        // Not resumable on one connection: the state is useless.
        Err(Error::NoRanges) => {
            let _ = std::fs::remove_file(&path);
        }
        // Complete: the final state stays until `finish` has renamed the part file, so a
        // crash in between still resumes (to an immediate rename).
        Ok(()) | Err(_) => save_state(&ctx, &mut saved, &path, &mut synced, true).await?,
    }
    result
}

/// While downloading, the part file is flushed (and the state rewritten) only after 1/16 of
/// the file (1-64 MiB) has arrived or this long, at most once per save tick, and always when
/// the connections stop. Flushing every tick stalled HDDs and churned antivirus; IDM saves
/// "several times per minute" [IDM-doc].
///
/// A flush holds every write to the file for as long as it runs (NTFS, measured, #361), so on a
/// disk where it takes seconds, flushing as often as the file grows left the download frozen
/// most of the time. The next flush therefore waits nine times as long as the last flush during
/// the transfer took (`forced_gap`), at most `SYNC_GAP_MAX`: at most a tenth of the time goes
/// to flushing, and on a fast disk (a flush of a few milliseconds) nothing changes. What it
/// gives up: after a power cut or a crash the bytes since the last flush are downloaded again,
/// so on a slow disk up to `SYNC_GAP_MAX` of them (at 1 Gbit/s about 7 GB) instead of about 2 s
/// of them. The saved positions still never claim bytes that weren't flushed.
const SYNC_BYTES_MAX: u64 = 64 << 20;
const SYNC_BYTES_MIN: u64 = 1 << 20;
const SYNC_EVERY: Duration = Duration::from_secs(30);
const SYNC_GAP_MAX: Duration = Duration::from_secs(60);

fn sync_bytes(size: u64) -> u64 {
    (size / 16).clamp(SYNC_BYTES_MIN, SYNC_BYTES_MAX)
}

/// How long to wait after a flush that took `took` before the next.
fn flush_gap(took: Duration) -> Duration {
    took.saturating_mul(9).min(SYNC_GAP_MAX)
}

/// When the part file was last flushed, how many bytes were on disk then, and how long it took.
struct Synced {
    at: std::time::Instant,
    bytes: u64,
    took: Duration,
}

impl Synced {
    /// Is a flush due with `bytes` on disk now, `elapsed` since the last one ended?
    fn due(&self, bytes: u64, size: u64, elapsed: Duration) -> bool {
        elapsed >= flush_gap(self.took)
            && (bytes >= self.bytes.saturating_add(sync_bytes(size)) || elapsed >= SYNC_EVERY)
    }
}

/// Save the resume state if it is due (`force`: always). `saved` is the state last written; it
/// never claims bytes that weren't flushed (snapshot -> flush -> write).
async fn save_state(
    ctx: &Ctx,
    saved: &mut ResumeState,
    path: &Path,
    synced: &mut Synced,
    force: bool,
) -> Result<(), Error> {
    let segments = ctx.shared.with_planner(|p| p.save());
    let validators = ctx.validators.lock().unwrap().clone();
    let target = ctx.target();
    let (final_url, final_get) = (target.url, ctx.opts.post.is_some() && target.post.is_none());
    let meta_changed = validators != saved.validators
        || final_url != saved.final_url
        || final_get != saved.final_get;
    let bytes: u64 = segments.iter().map(|s| s.cursor - s.start).sum();
    let due = force || synced.due(bytes, ctx.size, synced.at.elapsed());
    let file = ctx.file.clone();
    let path = path.to_owned();
    if !due {
        // Between flushes only a new validator or address is written (with segments on disk).
        if meta_changed {
            saved.validators = validators;
            saved.final_url = final_url;
            saved.final_get = final_get;
            let s = saved.clone();
            tokio::task::spawn_blocking(move || s.write(&path))
                .await
                .expect("state writer panicked")?;
        }
        return Ok(());
    }
    if segments == saved.segments && !meta_changed && path.exists() {
        // Nothing new since the last write.
        *synced = Synced {
            at: std::time::Instant::now(),
            bytes,
            took: Duration::ZERO,
        };
        return Ok(());
    }
    let mut s = saved.clone();
    s.validators = validators;
    s.final_url = final_url;
    s.final_get = final_get;
    // Tests' slow disk (not for the forced saves at the start and the end).
    let slow = if force {
        None
    } else {
        ctx.opts.slow_flush.clone()
    };
    ctx.shared.flushing.store(true, Ordering::Release);
    let began = std::time::Instant::now();
    let done = tokio::task::spawn_blocking(move || {
        if let Some(slow) = slow {
            let _writes_wait = slow.gate.write().unwrap();
            std::thread::sleep(slow.takes);
        }
        s.save(&path, segments, &file).map(|()| s)
    })
    .await
    .expect("state writer panicked");
    ctx.shared.flushing.store(false, Ordering::Release);
    *saved = done?;
    *synced = Synced {
        at: std::time::Instant::now(),
        bytes,
        took: forced_gap(force, began.elapsed()),
    };
    Ok(())
}

/// The flush time that spaces out the next flush. A forced save runs while no connection
/// writes (before they start, after they stop), so it holds no transfer up and doesn't push the
/// next save back: the first one at the start, which also pays for creating the part file, once
/// delayed the first save of progress past the end of a short download (#518).
fn forced_gap(force: bool, took: Duration) -> Duration {
    if force { Duration::ZERO } else { took }
}

/// A refusal of the address asked for that the browser's own download got past (#630): 401
/// and 403 (a token bound to the browser's session), 404 and 410 (a one-time link the browser
/// already spent, which some servers then forget, others call gone), 412 (FileHippo's answer
/// to whatever isn't a browser). A site's bot check stays the site's call (#376).
fn link_refused(e: &Error) -> bool {
    matches!(e, Error::Status(s) if matches!(s.as_u16(), 401 | 403 | 404 | 410 | 412))
}

/// What a download's first request needs besides its address, headers and options.
struct FirstAsk<'a> {
    as_get: &'a AsGet,
    seen: &'a Arc<crate::server::ServerSeen>,
    expects: bool,
    shared: &'a Shared,
    cancel: &'a CancellationToken,
}

/// A first request's client, its answer (or why not) and whether it went as MDM (#372).
type FirstAnswer = (
    Client,
    Result<((Response, HeaderMap), Duration), Error>,
    bool,
);

impl FirstAsk<'_> {
    /// `first_get` at `url` with its client; asked once more as MDM when the browser's
    /// User-Agent is refused (#372, `opts` then goes on as MDM: the third value says so).
    async fn at(
        &self,
        opts: &mut Options,
        url: &Url,
        headers: &HeaderMap,
    ) -> Result<FirstAnswer, Error> {
        let (shared, cancel, expects) = (self.shared, self.cancel, self.expects);
        let client0 = client(opts, url, self.as_get, self.seen)?;
        match first_get(&client0, url, headers, opts, expects, shared, cancel).await {
            Err(e) if agent_refused(&e) && opts.agent_fallback.is_some() => {
                // #372: the rest of the download goes as MDM too.
                opts.user_agent = opts.agent_fallback.take().unwrap_or_default();
                shared.event(
                    "The server refused the browser's User-Agent; asking as Mister Download Manager",
                );
                let client1 = client(opts, url, self.as_get, self.seen)?;
                let first = first_get(&client1, url, headers, opts, expects, shared, cancel).await;
                Ok((client1, first, true))
            }
            first => Ok((client0, first, false)),
        }
    }
}

/// A plain 403, which a browser's User-Agent may have brought on (#372). A site's bot check
/// isn't asked again as MDM: it gets one try the way a browser connects (`browser_first`).
fn agent_refused(e: &Error) -> bool {
    matches!(e, Error::Status(s) if s.as_u16() == 403)
}

/// A first answer that refuses MDM's connection for not being a browser's (#668): the site's bot
/// check (`Error::BotCheck`, Cloudflare's), or 412 Precondition Failed, which MDM's first
/// request, sent without any precondition (no `If-…` header), can only get as such a refusal
/// (FileHippo's, #630).
fn refused_as_a_program(e: &Error) -> bool {
    matches!(e, Error::BotCheck) || matches!(e, Error::Status(s) if s.as_u16() == 412)
}

/// #668: the first request once more, after the site refused MDM's own connection as a
/// program's (`refused_as_a_program`, its error `refused`): through the browser-like connection
/// (`crate::browserlike`), at the same address with the same headers, once, its answer checked
/// as the first one is. When that fails too, `refused` is what is told (the error users know,
/// with its way out: the browser), the reason in the event log. With the answer, the connection
/// the download goes on with.
async fn browser_first(
    opts: &Options,
    url: &Url,
    headers: &HeaderMap,
    refused: Error,
    expects: bool,
    shared: &Shared,
    cancel: &CancellationToken,
) -> Result<
    (
        Arc<crate::browserlike::Browser>,
        Head,
        Body,
        HeaderMap,
        Duration,
    ),
    Error,
> {
    if !crate::browserlike::AVAILABLE {
        return Err(refused);
    }
    let host = url.host_str().unwrap_or("?");
    shared.event(format!(
        "{host}: {refused}; asking once more the way a browser connects"
    ));
    let (b, asked) = match browser_ask(opts, url, headers, expects, cancel).await {
        Ok(got) => got,
        Err(e @ (Error::Cancelled | Error::Adapter(_))) => return Err(e),
        Err(e) => {
            shared.event(format!("{host}, asked the way a browser connects: {e}"));
            return Err(refused);
        }
    };
    let (head, body, headers, took) = asked;
    shared.event(format!(
        "{host}: the browser-like connection got through; downloading on it, one connection"
    ));
    Ok((b, head, body, headers, took))
}

/// The browser-like connection for `url`, and its answer to the first request (`browser_first`,
/// `probe_kept`): the file's, or why not.
async fn browser_ask(
    opts: &Options,
    url: &Url,
    headers: &HeaderMap,
    expects: bool,
    cancel: &CancellationToken,
) -> Result<
    (
        Arc<crate::browserlike::Browser>,
        (Head, Body, HeaderMap, Duration),
    ),
    Error,
> {
    let b = Arc::new(crate::browserlike::Browser::new(opts, url, headers)?);
    let asked = std::time::Instant::now();
    let (head, body, headers) = b.get(url.clone(), headers.clone(), false, cancel).await?;
    let took = asked.elapsed();
    crate::browserlike::check_first(&head, expects)?;
    Ok((b, (head, body, headers, took)))
}

/// A plain `GET` (no Range), or a POST of `post`, that must answer 200 or 206. A GET follows
/// the HTTP redirects (the client's policy) and then the redirects a page makes itself (#28):
/// a download site's "your download starts in a moment" page, sent with a `Refresh` header or a
/// `<meta http-equiv="refresh">`, leads on to the file as it does in a browser (`refresh_target`,
/// `meta_refresh`). Its wait, up to `PAGE_WAIT`, is kept, and the cookies it sets go on to its
/// own host. A page that leads on only with a script isn't followed: it stays the answer.
///
/// A site whose links aren't the file's address (`sites`, #34: Google Drive) is asked its own
/// way: its link goes to the file's address first, its pages say where to go next or why it
/// refuses (`Error::Refused`), and a page of its where the file was expected is an error, never
/// the file. So is any web page when the download `expects` another kind of file (#533,
/// `expects_other_file`): a server's refusal, a sign-in wall. A small answer without a type is
/// looked into then, and asked for again if it isn't a page.
///
/// With the answer, `headers` as the download goes on with them at the address it ended at:
/// credentials scoped to it (`scoped_headers`), the pages' cookies added; and how long the last
/// request took to answer (a page's wait and its own request not counted: the time to first byte).
#[allow(clippy::too_many_arguments)]
async fn get(
    client: &Client,
    url: Url,
    headers: HeaderMap,
    post: Option<&Bytes>,
    site_agent: bool,
    expects: bool,
    cancel: &CancellationToken,
) -> Result<(Response, HeaderMap, Duration), Error> {
    let (mut url, mut headers) = (url, headers);
    // A site's link is asked at its file's address (a Drive link: Drive's file host), the same
    // party, so the user's cookies go too. A form's POST goes where it was given.
    if post.is_none() {
        if let Some(r) = sites::not_a_file(&url) {
            return Err(Error::Refused(r));
        }
        if let Some(next) = sites::rewrite(&url) {
            headers = scoped_headers(&headers, &url, &next);
            url = next;
        }
    }
    // After a page's redirect, a page read but leading nowhere is asked for again, as is.
    let mut hops = 0;
    // A site's refusal while it is looked at once more (`sites::second_look`), with the address
    // that refused; and whether that look was taken.
    let mut held: Option<(Refusal, Url)> = None;
    let mut looked = false;
    // Redirects between a site's own hosts followed here (`get_policy`).
    let mut party_hops = 0;
    loop {
        let req = build(client, url.clone(), post, &headers, site_agent);
        let asked = std::time::Instant::now();
        let resp = cancellable(cancel, async { Ok(req.send().await?) }).await?;
        let took = asked.elapsed();
        let from = resp.url().clone();
        // A redirect between a site's own hosts, which the client leaves (`get_policy`): followed
        // here, the user's credentials and the cookies it sets going on.
        if post.is_none()
            && resp.status().is_redirection()
            && party_hops < MAX_REDIRECTS
            && let Some(next) = header_str(resp.headers(), header::LOCATION)
                .and_then(|l| from.join(l).ok())
                .filter(|next| sites::same_party(&from, next))
        {
            headers = scoped_headers(&scoped_headers(&headers, &url, &from), &from, &next);
            if !(from.scheme() == "https" && next.scheme() != "https") {
                add_cookies(&mut headers, &set_cookies(resp.headers()));
            }
            url = next;
            party_hops += 1;
            continue;
        }
        // The second look stands only if it leads to the file another way (a Google Doc's
        // editor, whose export follows below); otherwise the refusal was the answer.
        if let Some((refusal, refused_at)) = held.take() {
            let other_way = resp.status() == StatusCode::OK
                && is_web_page(resp.headers())
                && sites::rewrite(&from).is_some_and(|n| n != refused_at);
            if !other_way {
                return Err(Error::Refused(refusal));
            }
        }
        if let Some(r) = sites::refused(&url, &from, resp.status().as_u16()) {
            return Err(Error::Refused(r));
        }
        match resp.status() {
            StatusCode::OK | StatusCode::PARTIAL_CONTENT => {}
            s if s == StatusCode::TOO_MANY_REQUESTS || s == StatusCode::SERVICE_UNAVAILABLE => {
                return Err(Error::Throttled(s, retry_after(resp.headers())));
            }
            s if is_bot_check(s, resp.headers()) => return Err(Error::BotCheck),
            s => return Err(Error::Status(s)),
        }
        // Where the HTTP redirects ended: the headers' credentials follow as reqwest's did.
        headers = scoped_headers(&headers, &url, &from);
        // A site's page, without a file name, where its file was expected: the site talking
        // (a page it changed, a wall), never the file.
        let site_page = post.is_none()
            && resp.status() == StatusCode::OK
            && is_web_page(resp.headers())
            && !named(resp.headers())
            && (sites::expects_file(&from) || expects);
        // No type said where another kind of file is expected (#533): small, it is looked into.
        let untyped = post.is_none()
            && expects
            && hops < MAX_PAGE_REDIRECTS
            && resp.status() == StatusCode::OK
            && !resp.headers().contains_key(header::CONTENT_TYPE)
            && !named(resp.headers())
            && resp.content_length().is_none_or(|n| n <= PAGE_PEEK as u64);
        // A form is never posted again, and a page's own redirects end somewhere.
        if post.is_some() || resp.status() != StatusCode::OK || hops >= MAX_PAGE_REDIRECTS {
            if site_page {
                return Err(Error::GotWebPage);
            }
            return Ok((resp, headers, took));
        }
        let cookies = set_cookies(resp.headers());
        let refresh =
            header_text(resp.headers(), header::REFRESH).and_then(|v| refresh_target(&v, &from));
        // A site's link reached through redirects (a short link to a Drive file; a Google Doc's
        // editor after Drive's `open?id=`): its file's address.
        let site_link = is_web_page(resp.headers())
            .then(|| sites::rewrite(&from))
            .flatten();
        let (wait, next) = if let Some(next) = refresh {
            next
        } else if let Some(next) = site_link {
            (Duration::ZERO, next)
        } else if (is_web_page(resp.headers()) || untyped)
            // A web page small enough to look into (larger ones aren't waiting pages).
            && resp.content_length().is_none_or(|n| n <= PAGE_PEEK as u64)
        {
            let page = read_up_to(resp, PAGE_PEEK, cancel).await?;
            // Without a type and not a page: the file, asked for again as is.
            let site_page = site_page || (untyped && sniffs_as_html(&page));
            if untyped && !site_page {
                add_cookies(&mut headers, &cookies);
                url = from;
                hops = MAX_PAGE_REDIRECTS;
                continue;
            }
            match sites::read_page(&from, &page) {
                PageSays::Next(next) => (Duration::ZERO, next),
                PageSays::Refused(r) => match sites::second_look(&from).filter(|_| !looked) {
                    Some(look) => {
                        looked = true;
                        held = Some((r, from.clone()));
                        (Duration::ZERO, look)
                    }
                    None => return Err(Error::Refused(r)),
                },
                PageSays::Nothing => match meta_refresh(&page, &from) {
                    Some(next) => next,
                    None if site_page => return Err(Error::GotWebPage),
                    // Just a page: asked for again, as its body went into looking.
                    None => {
                        add_cookies(&mut headers, &cookies);
                        url = from;
                        hops = MAX_PAGE_REDIRECTS;
                        continue;
                    }
                },
            }
        } else if site_page {
            return Err(Error::GotWebPage);
        } else {
            return Ok((resp, headers, took));
        };
        if !wait.is_zero() {
            cancellable(cancel, async {
                tokio::time::sleep(wait.min(PAGE_WAIT)).await;
                Ok(())
            })
            .await?;
        }
        headers = scoped_headers(&headers, &from, &next);
        // Its own host's (or its site's, `sites::same_party`), and never from https down to http
        // (as the user's own cookies).
        if (next.host_str() == from.host_str() || sites::same_party(&from, &next))
            && !(from.scheme() == "https" && next.scheme() != "https")
        {
            add_cookies(&mut headers, &cookies);
        }
        url = next;
        hops += 1;
    }
}

/// Most redirects in a row that pages make themselves (`get`).
const MAX_PAGE_REDIRECTS: usize = 5;
/// How much of a web page is read to find its redirect.
const PAGE_PEEK: usize = 256 << 10;
/// The longest wait a page asks for before it leads on that is kept.
const PAGE_WAIT: Duration = Duration::from_secs(10);

/// The first `max` bytes of `resp`'s body (all of it when shorter).
async fn read_up_to(
    mut resp: Response,
    max: usize,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    while body.len() < max {
        let chunk = cancellable(cancel, async { Ok(resp.chunk().await?) }).await?;
        let Some(chunk) = chunk else { break };
        body.extend_from_slice(&chunk);
    }
    body.truncate(max);
    Ok(body)
}

/// Where a page leads on to and after how long, from its `Refresh` value or a meta refresh's
/// `content` (`5; url=next.html`, `0;URL='https://…'`, `3, https://…`), relative to `base`.
/// None unless it leads to another http(s) address: a page that only reloads itself is the
/// answer.
fn refresh_target(value: &str, base: &Url) -> Option<(Duration, Url)> {
    let (delay, rest) = value.split_once([';', ','])?;
    let delay = delay
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|d| d.is_finite() && *d >= 0.0)?;
    let rest = rest.trim();
    // `url=` is optional; the address may be quoted.
    let target = match rest.get(..3) {
        Some(u) if u.eq_ignore_ascii_case("url") && rest[3..].trim_start().starts_with('=') => {
            rest[3..].trim_start()[1..].trim()
        }
        _ => rest,
    };
    let target = target
        .strip_prefix(['"', '\''])
        .map_or(target, |t| t.trim_end_matches(['"', '\'']));
    // A meta tag's value is HTML: `&amp;` and the like.
    let target = html::decode(target);
    let next = base.join(target.trim()).ok()?;
    let mut same = next.clone();
    same.set_fragment(base.fragment());
    (matches!(next.scheme(), "http" | "https") && same != *base).then(|| {
        (
            Duration::from_secs_f64(delay.min(PAGE_WAIT.as_secs_f64())),
            next,
        )
    })
}

/// The redirect a page's `<meta http-equiv="refresh" content="…">` makes (`refresh_target`).
fn meta_refresh(page: &[u8], base: &Url) -> Option<(Duration, Url)> {
    let page = String::from_utf8_lossy(page);
    html::tags(&page, "meta").find_map(|(_, tag)| {
        html::attribute(tag, "http-equiv")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("refresh"))
            .then(|| html::attribute(tag, "content"))
            .flatten()
            .and_then(|c| refresh_target(&c, base))
    })
}

/// The `name=value` pairs a response's `Set-Cookie` headers set.
pub(crate) fn set_cookies(h: &HeaderMap) -> Vec<String> {
    h.get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next())
        .map(str::trim)
        .filter(|c| c.contains('='))
        .map(str::to_owned)
        .collect()
}

/// `cookies` added to the request's `Cookie` header, each replacing one of the same name.
pub(crate) fn add_cookies(headers: &mut HeaderMap, cookies: &[String]) {
    if cookies.is_empty() {
        return;
    }
    let name = |c: &str| c.split('=').next().unwrap_or_default().trim().to_owned();
    let new: HashSet<String> = cookies.iter().map(|c| name(c)).collect();
    let mut all: Vec<String> = header_str(headers, header::COOKIE)
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|c| !c.is_empty() && !new.contains(&name(c)))
        .map(str::to_owned)
        .collect();
    all.extend(cookies.iter().cloned());
    if let Ok(v) = HeaderValue::from_str(&all.join("; ")) {
        headers.insert(header::COOKIE, v);
    }
}

/// The first request of a download, retried as a connection is (`retry_wait`) when it fails
/// in a way a later try may not: a server busy at the moment the download starts no longer
/// fails the whole download. With the answer (and the headers to go on with, `get`), how long its
/// last request waited for it: a waiting page's delay isn't the server's time to first byte.
async fn first_get(
    client: &Client,
    url: &Url,
    headers: &HeaderMap,
    opts: &Options,
    expects: bool,
    shared: &Shared,
    cancel: &CancellationToken,
) -> Result<((Response, HeaderMap), Duration), Error> {
    let ((resp, headers, took), _) = first_try(opts, shared, cancel, || {
        get(
            client,
            url.clone(),
            headers.clone(),
            opts.post.as_ref(),
            opts.site_agent(),
            expects,
            cancel,
        )
    })
    .await?;
    Ok(((resp, headers), took))
}

/// `attempt` retried as `first_get` does; with its result, how long the try that got it took.
async fn first_try<T, F, Fut>(
    opts: &Options,
    shared: &Shared,
    cancel: &CancellationToken,
    mut attempt: F,
) -> Result<(T, Duration), Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Error>>,
{
    let mut failures = 0;
    loop {
        let asked = std::time::Instant::now();
        match cancellable(cancel, attempt()).await {
            Ok(got) => return Ok((got, asked.elapsed())),
            Err(e) if e.transient() && failures < opts.max_retries => {
                failures += 1;
                shared.event(format!("Connection 1: {e}; retrying"));
                shared.conn(0, |c| c.status = ConnStatus::Retrying(e.clone()));
                if let Some(delay) = retry_wait(opts, failures, &e) {
                    cancellable(cancel, async {
                        tokio::time::sleep(delay).await;
                        Ok(())
                    })
                    .await?;
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// Run connections until the planner is complete. Respawns a connection when work is left
/// and none is running (e.g. the others retired).
async fn supervise(ctx: Arc<Ctx>, first: Option<Body>) -> Result<(), Error> {
    let mut set = JoinSet::new();
    let mut next_id: WorkerId = 0;
    if let Some(body) = first {
        // The probe's body is byte 0 onward: reserve that segment for connection 0 before any
        // other connection can split or take it.
        ctx.shared.with_planner(|p| p.assign(0, None));
        set.spawn(worker(ctx.clone(), 0, Some(body)));
        next_id = 1;
    }
    // No more connections than the host is known to allow.
    let host = ctx.target().url.host_str().unwrap_or("").to_owned();
    let learned = ctx.opts.host_limits.as_ref().and_then(|h| h.cap(&host));
    let wanted = ctx.opts.connections();
    if let Some(cap) = learned.filter(|c| *c < wanted) {
        ctx.shared
            .event(format!("{host} allows {cap} connection(s) (learned)"));
    }
    let max = learned.map_or(wanted, |c| c.min(wanted));
    // A small file starts with fewer connections (one per INITIAL_PIECE): opened all at once
    // they would cut it into slivers and fetch some twice (benchmark 2026-10-30: 256 KiB took
    // 13-19 requests; IDM, which ramps up in waves, took 4). Later splits still go down to
    // `min_split` as connections free up.
    let size = ctx.shared.with_planner(|p| p.size());
    let by_size = u32::try_from(size.div_ceil(INITIAL_PIECE)).unwrap_or(u32::MAX);
    let max = max.min(by_size.max(1)).max(1);
    // A POST the server hasn't shown to take ranges: one connection, until a 206 says the
    // others may follow (a second POST may do the form's work twice).
    let mut held = ctx.wait_for_ranges;
    while next_id < if held { 1 } else { max } {
        set.spawn(worker(ctx.clone(), next_id, None));
        next_id += 1;
    }
    let mut last_err = None;
    loop {
        let joined = tokio::select! {
            joined = set.join_next() => match joined {
                Some(joined) => joined,
                None => break,
            },
            () = ctx.range_answered.notified(), if held => {
                held = false;
                while next_id < max {
                    set.spawn(worker(ctx.clone(), next_id, None));
                    next_id += 1;
                }
                continue;
            }
        };
        match joined.expect("worker panicked") {
            Err(e) if e.is_fatal() => {
                stop_connections(&ctx, &mut set).await;
                return Err(e);
            }
            Err(e) => last_err = Some(e),
            Ok(()) => {}
        }
        let complete = ctx.shared.with_planner(|p| p.is_complete());
        // Every byte is in: the connections still asking (a retry under way, an answer late on
        // a busy machine) have nothing left to fetch. Stopped now, a late answer can't start
        // the download over (#136: a server without ranges sent the whole file twice).
        if complete {
            stop_connections(&ctx, &mut set).await;
            break;
        }
        if set.is_empty() {
            if let Some(e) = last_err.take() {
                return Err(e); // the last connection exhausted its retries
            }
            set.spawn(worker(ctx.clone(), next_id, None));
            next_id += 1;
        }
    }
    Ok(())
}

/// Stop the connections still running, and wait until each has. They are told to stop, not
/// aborted: a connection aborted in a blocking read or write of the part file (the check of
/// its overlap, a write) leaves that call running, with the file open, after the download has
/// ended. A finished download then failed with "part file still in use", the rename's handle
/// still taken (#201: a connection taken over in its overlap check, on a busy machine), and a
/// stopped one went on writing after it had returned. Every wait of a connection ends on its
/// token, and a blocking call only takes as long as one read or write.
async fn stop_connections(ctx: &Ctx, set: &mut JoinSet<Result<(), Error>>) {
    ctx.cancel.cancel();
    while set.join_next().await.is_some() {}
}

/// One connection: take an assignment, fetch it, repeat until there is nothing left to do.
async fn worker(ctx: Arc<Ctx>, id: WorkerId, mut first: Option<Body>) -> Result<(), Error> {
    let mut after = None;
    let mut failures = 0u32;
    // Bytes this connection had received at its last failure: progress since resets the budget.
    let mut at_last_failure = 0u64;
    loop {
        if ctx.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let Some(a) = ctx.shared.with_planner(|p| p.assign(id, after.take())) else {
            ctx.shared.conn(id, |c| c.status = ConnStatus::Finished);
            return Ok(());
        };
        ctx.shared.conn(id, |c| {
            c.status = ConnStatus::Connecting;
            c.speed = 0;
        });
        let target = ctx.target();

        // The server ignores ranges: only the probe's plain GET can bring data. While it
        // still streams it takes this segment over as it gets there; once it has stopped,
        // the download starts over on one connection.
        if first.is_none() && ctx.ranges.load(Ordering::SeqCst) == RANGES_NO {
            ctx.shared.with_planner(|p| p.release(id));
            return if ctx.shared.with_planner(|p| p.holds(0)) {
                ctx.shared.conn(id, |c| c.status = ConnStatus::Finished);
                Ok(())
            } else {
                Err(Error::NoRanges)
            };
        }
        let whole = first.is_some();
        // Re-fetch the last few bytes already on disk and compare them before going on.
        let overlap = if first.is_some() {
            0
        } else {
            a.written.min(ctx.opts.verify_overlap)
        };
        let result = match first.take() {
            Some(body) if a.from == 0 => Ok(body),
            Some(_) => Err(Error::BadRange(
                "probe segment no longer starts at 0".into(),
            )),
            None => request(&ctx, &target, a.from - overlap, a.to).await,
        };
        let result = match result {
            Ok(body) => receive(&ctx, id, body, a.from, a.to, overlap, whole).await,
            Err(e) => Err(e),
        };
        match result {
            Ok(Some(end)) => {
                failures = 0;
                after = Some(end);
            }
            Ok(None) => {
                // Preempted (another connection took the segment over): ask again.
                ctx.shared.with_planner(|p| p.release(id));
            }
            Err(Error::NoRanges) => {
                // The server ignores ranges: connection 0 (the probe) keeps the whole file if it
                // can, every other connection stops. If it can't have it all at once (it was
                // split), it still takes each segment over as it reaches it, while it streams.
                ctx.shared.with_planner(|p| p.release(id));
                return if ctx.shared.with_planner(|p| p.make_single(0) || p.holds(0)) {
                    ctx.shared.conn(id, |c| c.status = ConnStatus::Finished);
                    Ok(())
                } else {
                    Err(Error::NoRanges)
                };
            }
            Err(e) => {
                ctx.shared.with_planner(|p| p.release(id));
                if e.is_fatal() {
                    return Err(e);
                }
                // Shed: reconnect at once for the rest of the segment (not a failure).
                if matches!(e, Error::SlowConnection) {
                    ctx.shared.event(format!("Connection {}: {e}", id + 1));
                    continue;
                }
                let (working, receiving) = ctx
                    .shared
                    .with_planner(|p| (p.active_workers(), p.receiving_workers()));
                let others = working > 0;
                // An extra connection refused while others work: the host's connection limit.
                // Once learned (twice seen), this connection retires instead of retrying — if the
                // others fill the cap. Below it the server most likely still counts a connection
                // of ours that just closed (a shed or a split reconnects at once): retry as usual
                // (benchmark 2026-10-29: retiring here left 50 MiB on one connection for 12 s).
                if e.refused()
                    && others
                    && let Some(limits) = &ctx.opts.host_limits
                {
                    let host = ctx.target().url.host_str().unwrap_or("").to_owned();
                    if let Some(cap) = limits.refused(&host, receiving as u32)
                        && working as u32 >= cap
                    {
                        ctx.shared.event(format!(
                            "Connection {}: {host} allows {cap} connection(s) (learned)",
                            id + 1
                        ));
                        ctx.shared
                            .conn(id, |c| c.status = ConnStatus::Retired { host, cap });
                        return Ok(());
                    }
                }
                // A dead or forbidden link is not retried. While other connections work it
                // means this extra one is refused (a per-client connection limit): retire it.
                if e.link_dead() {
                    // A redirected address that expired (signed CDN links): go where the
                    // original address leads now, if that is still this file.
                    if !matches!(e, Error::LinkExpired)
                        && ctx.refresh_target(target.generation).await
                    {
                        ctx.shared.conn(id, |c| c.status = ConnStatus::Connecting);
                        continue;
                    }
                    let e = match e {
                        Error::Status(_) if ctx.origin.is_some() => Error::LinkExpired,
                        e => e,
                    };
                    ctx.shared
                        .conn(id, |c| c.status = ConnStatus::Failed(e.clone()));
                    return if others { Ok(()) } else { Err(e) };
                }
                let mut received = 0;
                ctx.shared.conn(id, |c| received = c.downloaded);
                if received > at_last_failure {
                    failures = 0;
                }
                at_last_failure = received;
                failures += 1;
                if failures > ctx.opts.max_retries {
                    let others = ctx.shared.with_planner(|p| p.active_workers()) > 0;
                    ctx.shared
                        .conn(id, |c| c.status = ConnStatus::Failed(e.clone()));
                    return if others { Ok(()) } else { Err(e) };
                }
                ctx.shared
                    .event(format!("Connection {}: {e}; retrying", id + 1));
                ctx.shared
                    .conn(id, |c| c.status = ConnStatus::Retrying(e.clone()));
                if let Some(delay) = retry_wait(&ctx.opts, failures, &e) {
                    cancellable(&ctx.cancel, async {
                        tokio::time::sleep(delay).await;
                        Ok(())
                    })
                    .await?;
                }
            }
        }
    }
}

/// `GET` bytes `[from, to)`; checks the server returned exactly that range of the same file.
/// FTP: `REST from` and `RETR`.
async fn request(ctx: &Ctx, target: &Target, from: u64, to: u64) -> Result<Body, Error> {
    if let Some(site) = &ctx.ftp {
        return match cancellable(&ctx.cancel, site.open_at(from, Some(ctx.size))).await {
            Ok(t) => {
                if from > 0 {
                    ctx.learn_ranges(true);
                }
                Ok(Body::Ftp(t))
            }
            // REST refused ("not implemented", "syntax error") where it wasn't known to work.
            Err(Error::FtpReply {
                code: 500 | 501 | 502 | 504,
                ..
            }) if from > 0 && ctx.ranges.load(Ordering::SeqCst) != RANGES_YES => {
                ctx.learn_ranges(false);
                Err(Error::NoRanges)
            }
            Err(e) => Err(e),
        };
    }
    let if_range = ctx
        .validators
        .lock()
        .unwrap()
        .if_range(target.url.host_str())
        .and_then(|v| HeaderValue::from_str(v).ok());
    let range = format!("bytes={from}-{}", to - 1);
    let (head, body) = match &ctx.browser {
        // The browser-like connection's own (#668): its redirects followed as `get` does.
        Some(b) => {
            let mut headers = target.headers.clone();
            headers.insert(header::RANGE, HeaderValue::from_str(&range).expect("ascii"));
            if let Some(v) = &if_range {
                headers.insert(header::IF_RANGE, v.clone());
            }
            let (head, body, _) = b
                .get(target.url.clone(), headers, true, &ctx.cancel)
                .await?;
            (head, body)
        }
        None => {
            let mut req = build(
                &ctx.client,
                target.url.clone(),
                target.post.as_ref(),
                &target.headers,
                ctx.opts.site_agent(),
            )
            .header(header::RANGE, range);
            if let Some(v) = &if_range {
                req = req.header(header::IF_RANGE, v);
            }
            let resp = cancellable(&ctx.cancel, async { Ok(req.send().await?) }).await?;
            (Head::of(&resp), Body::Http(resp))
        }
    };
    if head.status.is_success() && !ctx.web_page_ok && is_web_page(&head.headers) {
        return Err(Error::GotWebPage);
    }
    match head.status {
        StatusCode::PARTIAL_CONTENT => {
            ctx.learn_ranges(true);
            ctx.range_answered.notify_one();
        }
        // The same file under another ETag (each CDN node has its own): go on with the date.
        StatusCode::OK if per_node_etag(ctx, if_range.as_ref(), &head) => {
            return Err(Error::RangesUnavailable);
        }
        // A full body for a ranged request once ranges are known to work: If-Range failed (the
        // file changed), unless it is still the same file and this answer just lacks the range.
        StatusCode::OK if ctx.ranges.load(Ordering::SeqCst) == RANGES_YES => {
            return Err(if same_file(ctx, &head) {
                Error::RangesUnavailable
            } else {
                Error::FileChanged
            });
        }
        StatusCode::OK => {
            ctx.learn_ranges(false);
            return Err(Error::NoRanges);
        }
        s if s == StatusCode::TOO_MANY_REQUESTS || s == StatusCode::SERVICE_UNAVAILABLE => {
            return Err(Error::Throttled(s, retry_after(&head.headers)));
        }
        s => return Err(Error::Status(s)),
    }
    let (start, _, total) = parse_content_range(&head.headers)?;
    if start != from || total.is_some_and(|t| t != ctx.size) {
        return Err(Error::BadRange(format!(
            "asked {from}-{}, got {start}..",
            to - 1
        )));
    }
    Ok(body)
}

/// A full answer that still describes the file being downloaded: the same length and, when the
/// download has validators, the same one it would send.
fn same_file(ctx: &Ctx, resp: &Head) -> bool {
    let host = resp.url.host_str();
    let v = ctx.validators.lock().unwrap();
    let h = &resp.headers;
    let theirs = match v.if_range(host) {
        None => return resp.len == Some(ctx.size),
        Some(ours) if Some(ours) == v.etag.as_deref() => header_str(h, header::ETAG),
        Some(_) => header_str(h, header::LAST_MODIFIED),
    };
    resp.len == Some(ctx.size) && theirs.is_some() && theirs == v.if_range(host)
}

/// A full answer to an ETag `If-Range` that still matches by length and Last-Modified: the
/// ETag differs per node (CDNs, mirrors), not the file. The ETag is dropped for the rest of the
/// download, so the next request validates with the date (and the overlap check).
fn per_node_etag(ctx: &Ctx, sent: Option<&HeaderValue>, resp: &Head) -> bool {
    let mut v = ctx.validators.lock().unwrap();
    // Sent with an ETag (dates never start with a quote) — possibly one another connection has
    // just dropped, so don't compare it with the current one.
    let sent_etag = sent.is_some_and(|s| s.as_bytes().starts_with(b"\""));
    // Another node's ETag differs from the one sent. The same ETag is that too only from a
    // server that advertised ranges: one that didn't, answering in full, has no ranges (#136),
    // and is told so at once rather than after a round of retries.
    let theirs = header_str(&resp.headers, header::ETAG);
    let other_node = ctx.advertised
        || (theirs.is_some() && theirs.map(str::as_bytes) != sent.map(HeaderValue::as_bytes));
    let same = sent_etag
        && other_node
        && resp.len == Some(ctx.size)
        && v.last_modified.is_some()
        && header_str(&resp.headers, header::LAST_MODIFIED) == v.last_modified.as_deref();
    if same {
        v.etag = None;
        v.etag_host = None;
    }
    same
}

fn header_str(h: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

/// A site's bot check turning the request away (#376), as Cloudflare marks it.
pub(crate) fn is_bot_check(status: StatusCode, h: &HeaderMap) -> bool {
    status == StatusCode::FORBIDDEN
        && h.get("cf-mitigated")
            .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"challenge"))
}

pub(crate) fn is_web_page(h: &HeaderMap) -> bool {
    h.get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(is_web_page_type)
}

fn is_web_page_type(content_type: &str) -> bool {
    let mime = content_type.split(';').next().unwrap_or("").trim();
    mime.eq_ignore_ascii_case("text/html") || mime.eq_ignore_ascii_case("application/xhtml+xml")
}

/// The server names what it sends (`Content-Disposition`): its own file, page or not.
pub(crate) fn named(h: &HeaderMap) -> bool {
    h.contains_key(header::CONTENT_DISPOSITION)
}

/// Where a download is saved under a name of its own (the browser's, the user's), that name.
fn dest_name(dest: &Destination) -> Option<&str> {
    match dest {
        Destination::File(p) | Destination::Free(p) => p.file_name().and_then(|n| n.to_str()),
        Destination::Dir(_) => None,
    }
}

/// Types of file a web page never is, by extension: documents, archives, programs, disk images,
/// audio, video and pictures. A name without one of these (a page's `.php`, no extension at all)
/// says nothing.
const FILE_EXTENSIONS: &[&str] = &[
    "doc",
    "docx",
    "docm",
    "dot",
    "dotx",
    "xls",
    "xlsx",
    "xlsm",
    "xlsb",
    "ppt",
    "pptx",
    "pps",
    "ppsx",
    "odt",
    "ods",
    "odp",
    "rtf",
    "pdf",
    "epub",
    "mobi",
    "djvu",
    "zip",
    "rar",
    "7z",
    "gz",
    "tgz",
    "bz2",
    "xz",
    "zst",
    "tar",
    "cab",
    "arj",
    "lz",
    "lzh",
    "exe",
    "msi",
    "msix",
    "msixbundle",
    "appx",
    "apk",
    "deb",
    "rpm",
    "appimage",
    "dmg",
    "pkg",
    "jar",
    "iso",
    "img",
    "vhd",
    "vhdx",
    "mp3",
    "m4a",
    "aac",
    "flac",
    "wav",
    "wma",
    "ogg",
    "opus",
    "mp4",
    "m4v",
    "mkv",
    "avi",
    "mov",
    "wmv",
    "flv",
    "webm",
    "mpg",
    "mpeg",
    "jpg",
    "jpeg",
    "png",
    "gif",
    "bmp",
    "webp",
    "tif",
    "tiff",
    "psd",
    "torrent",
];

/// The download expects a kind of file a web page isn't (#533): the browser's own download had
/// another type (`expected_type`), or its name (`name`, else the address's) is a file's. A web
/// page in its place, without a name of the server's, is an error (`Error::GotWebPage`). A page
/// the browser saved as one (`text/html`), a `.html` link, or a server's own type for the file
/// (`application/octet-stream`) are downloads like any other.
fn expects_other_file(expected_type: Option<&str>, name: Option<&str>, url: &Url) -> bool {
    if let Some(t) = expected_type.map(str::trim).filter(|t| !t.is_empty()) {
        return !is_web_page_type(t);
    }
    let name = name.map_or_else(|| naming::file_name(None, url), str::to_owned);
    Path::new(&name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| FILE_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Bytes that start as an HTML document (after a byte order mark and white space).
fn sniffs_as_html(body: &[u8]) -> bool {
    let body = body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body);
    let start = body
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(body.len());
    let head: Vec<u8> = body[start..]
        .iter()
        .take(16)
        .map(u8::to_ascii_lowercase)
        .collect();
    ["<!doctype html", "<html", "<head", "<body", "<title"]
        .iter()
        .any(|t| head.starts_with(t.as_bytes()))
}

/// Whether the download is itself a web page (it was served as one, or is named like one).
fn expects_web_page(dest: &Path, content_type: Option<&str>) -> bool {
    content_type.is_some_and(is_web_page_type)
        || dest.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            ["htm", "html", "xhtml", "shtml"]
                .iter()
                .any(|x| e.eq_ignore_ascii_case(x))
        })
}

/// Stream a response into the file at `from`. `Some(end)` when the segment is complete,
/// `None` when the segment was taken over by another connection.
///
/// Every chunk is claimed from the planner as soon as it arrives (not when the buffer is
/// flushed), so a split can never fall inside data this connection has already received.
async fn receive(
    ctx: &Ctx,
    id: WorkerId,
    mut body: Body,
    from: u64,
    to: u64,
    mut verify: u64,
    whole: bool,
) -> Result<Option<u64>, Error> {
    ctx.shared.conn(id, |c| c.status = ConnStatus::Receiving);
    // Written and committed up to `pos`; `buf` holds claimed bytes not yet written.
    let mut pos = from;
    let mut buf = Vec::with_capacity(BATCH);
    let mut flushed_at = std::time::Instant::now();
    let started = std::time::Instant::now();
    // Time spent waiting for the disk (`flush`): a connection isn't slow for that (#361), it
    // wasn't reading the network then.
    let mut on_disk = Duration::ZERO;
    let mut received = 0u64;
    let mut checked = started;
    loop {
        let next = tokio::select! {
            _ = ctx.cancel.cancelled() => {
                // Paused or stopped: keep what arrived (it is claimed, so it is ours to write).
                flush(ctx, id, &mut pos, &mut buf).await?;
                return Err(Error::Cancelled);
            }
            n = body.chunk() => n,
        };
        let (ended, err) = match next {
            Ok(Some(mut chunk)) => {
                // The response starts `verify` bytes before `from`: they must equal the disk.
                if verify > 0 {
                    let k = verify.min(chunk.len() as u64);
                    let (file, at) = (ctx.file.clone(), from - verify);
                    let disk = tokio::task::spawn_blocking(move || read_at(&file, at, k as usize))
                        .await
                        .expect("reader panicked")?;
                    if disk[..] != chunk[..k as usize] {
                        return Err(Error::FileChanged);
                    }
                    verify -= k;
                    chunk = chunk.slice(k as usize..);
                    if chunk.is_empty() {
                        continue;
                    }
                }
                let want = chunk.len() as u64;
                // Pay first: the wait is when other connections may split this segment, and
                // the claim below then grants only what still belongs to it. Stop ends the
                // wait (under a low limit it is seconds long; Stop used to wait it out).
                let paid = async {
                    ctx.shared.limiter.acquire(want).await;
                    if let Some(g) = &ctx.opts.global {
                        g.acquire(want).await;
                    }
                };
                tokio::select! {
                    _ = ctx.cancel.cancelled() => {
                        // Not claimed yet: this chunk isn't ours to keep; what is, is written.
                        flush(ctx, id, &mut pos, &mut buf).await?;
                        return Err(Error::Cancelled);
                    }
                    _ = paid => {}
                }
                let at = pos + buf.len() as u64;
                // An answer that runs to the end of the file (`whole`: the probe's plain GET),
                // while ranges aren't known to work: at its segment's end it takes over the
                // split-off segments nothing has arrived for yet, rather than dropping bytes it
                // is already receiving for a range request the server may ignore.
                let keep_reading = whole && ctx.ranges.load(Ordering::SeqCst) != RANGES_YES;
                let claimed = ctx.shared.with_planner(|p| {
                    let Claim::Granted(mut n) = p.claim(id, at, want) else {
                        return None;
                    };
                    while keep_reading && n < want && p.absorb_next(id) {
                        match p.claim(id, at + n, want - n) {
                            Claim::Granted(m) => n += m,
                            Claim::Lost => break,
                        }
                    }
                    // Ending exactly at the boundary: take the next segment over too.
                    let full =
                        p.room(id, at + n) == Some(0) && !(keep_reading && p.absorb_next(id));
                    Some((n, full))
                });
                // Claimed bytes can't be taken over, so a lost segment has an empty buffer.
                let refund = |n: u64| {
                    ctx.shared.limiter.refund(n);
                    if let Some(g) = &ctx.opts.global {
                        g.refund(n);
                    }
                };
                let Some((n, full)) = claimed else {
                    refund(want);
                    return Ok(None);
                };
                if n < want {
                    refund(want - n);
                }
                if received == 0
                    && n > 0
                    && let Some(limits) = &ctx.opts.host_limits
                {
                    // This connection's first data: tell the host's learned cap how many are
                    // receiving now.
                    let now = ctx.shared.with_planner(|p| p.receiving_workers()) as u32;
                    limits.working(ctx.target().url.host_str().unwrap_or(""), now);
                }
                buf.extend_from_slice(&chunk[..n as usize]);
                ctx.shared.held.fetch_add(n, Ordering::Relaxed);
                received += n;
                if checked.elapsed() >= Duration::from_millis(500) {
                    checked = std::time::Instant::now();
                    let on_net = started.elapsed().saturating_sub(on_disk);
                    let speed = (received as f64 / on_net.as_secs_f64()) as u64;
                    ctx.shared.conn(id, |c| c.speed = speed);
                    note_best(ctx, received, on_net);
                    if should_shed(ctx, id, speed, on_net, pos + buf.len() as u64) {
                        flush(ctx, id, &mut pos, &mut buf).await?;
                        return Err(Error::SlowConnection);
                    }
                }
                if !full && buf.len() < hold_cap(ctx) && ctx.shared.flushing.load(Ordering::Acquire)
                {
                    continue; // the disk is busy flushing: this waits in memory (#361)
                }
                if buf.len() < BATCH && !full && flushed_at.elapsed() < FLUSH_EVERY {
                    continue;
                }
                (false, None)
            }
            Err(e) => (true, Some(e)),
            Ok(None) => (true, None),
        };
        let waiting = std::time::Instant::now();
        let flushed = flush(ctx, id, &mut pos, &mut buf).await?;
        flushed_at = std::time::Instant::now();
        on_disk += flushed_at - waiting;
        match flushed {
            Some(end) => {
                note_best(ctx, received, started.elapsed().saturating_sub(on_disk));
                // A segment another connection shortened leaves an HTTP body far from its end
                // (dropped); an FTP session is kept either way.
                if err.is_none() && (pos >= to || matches!(body, Body::Ftp(_))) {
                    // Not past a stop (`stop_connections` waits for this connection).
                    tokio::select! {
                        _ = ctx.cancel.cancelled() => {}
                        () = body.done(ended) => {}
                    }
                }
                return Ok(Some(end));
            }
            None if ended => return Err(err.unwrap_or(Error::ShortBody)),
            None => {}
        }
    }
}

/// How much a connection may hold in memory while the part file is flushed (`FLUSH_HOLD_MAX`,
/// shared by the connections this download may open).
fn hold_cap(ctx: &Ctx) -> usize {
    FLUSH_HOLD_MAX / ctx.opts.connections().max(1) as usize
}

/// Remember the speed of a response that carried enough (256 KiB) to say what this server can
/// do over one connection (`on_net`: the time it took, waits for the disk not counted).
fn note_best(ctx: &Ctx, received: u64, on_net: Duration) {
    let secs = on_net.as_secs_f64();
    if received >= 256 * 1024 && secs > 0.0 {
        ctx.shared
            .best_speed
            .fetch_max((received as f64 / secs) as u64, Ordering::Relaxed);
    }
}

/// Whether connection `id` (at `speed` for `elapsed` on this response, next byte `at`) should
/// reconnect: far slower than the best a connection of this download sustained (comparing
/// with the others fails once they are all stuck on slow paths), with a lot left at its pace.
/// Never while a speed limit makes connections slow on purpose.
fn should_shed(ctx: &Ctx, id: WorkerId, speed: u64, elapsed: Duration, at: u64) -> bool {
    let limited = ctx.shared.limiter.get().is_some()
        || ctx.opts.global.as_ref().is_some_and(|g| g.get().is_some());
    if !ctx.opts.shed_slow || limited || elapsed < SHED_GRACE {
        return false;
    }
    let best = ctx.shared.best_speed.load(Ordering::Relaxed);
    let left = ctx.shared.with_planner(|p| p.room(id, at)).unwrap_or(0);
    speed.saturating_mul(SHED_RATIO) < best
        && left as f64 / speed.max(1) as f64 > SHED_LEFT.as_secs_f64()
}

/// Write the claimed bytes in `buf` at `pos` and commit them. `Some(end)` when the segment is
/// now complete.
async fn flush(
    ctx: &Ctx,
    id: WorkerId,
    pos: &mut u64,
    buf: &mut Vec<u8>,
) -> Result<Option<u64>, Error> {
    let data = std::mem::replace(buf, Vec::with_capacity(BATCH));
    let n = data.len() as u64;
    if n > 0 {
        let file = ctx.file.clone();
        let at = *pos;
        let slow = ctx.opts.slow_flush.clone();
        let written = tokio::task::spawn_blocking(move || {
            let _waits_for_flush = slow.as_ref().map(|s| s.gate.read().unwrap());
            write_all_at(&file, at, &data)
        })
        .await
        .expect("writer panicked");
        ctx.shared.held.fetch_sub(n, Ordering::Relaxed);
        written?;
    }
    *pos += n;
    ctx.shared.conn(id, |c| c.downloaded += n);
    Ok(ctx.shared.with_planner(|p| p.commit(id, *pos)))
}

/// Servers without range support: one connection, written in order. Returns the size.
async fn single_stream(
    mut body: Body,
    file: &Arc<File>,
    shared: &Arc<Shared>,
    global: Option<&Limiter>,
    cancel: &CancellationToken,
) -> Result<u64, Error> {
    shared.conn(0, |c| c.status = ConnStatus::Receiving);
    let expected = body.len();
    let mut pos = 0u64;
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            n = body.chunk() => n,
        };
        let Some(chunk) = next? else { break };
        // Stop ends a limiter's wait too (under a low limit it is seconds long).
        let paid = async {
            shared.limiter.acquire(chunk.len() as u64).await;
            if let Some(g) = global {
                g.acquire(chunk.len() as u64).await;
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            _ = paid => {}
        }
        let (f, at, n) = (file.clone(), pos, chunk.len() as u64);
        tokio::task::spawn_blocking(move || write_all_at(&f, at, &chunk))
            .await
            .expect("writer panicked")?;
        pos += n;
        let mut v = shared.view.lock().unwrap();
        v.downloaded = pos;
        if let Some(c) = v.connections.first_mut() {
            c.downloaded = pos;
        }
    }
    if expected.is_some_and(|e| e != pos) {
        return Err(Error::ShortBody);
    }
    shared.conn(0, |c| c.status = ConnStatus::Finished);
    Ok(pos)
}

fn read_at(file: &File, mut at: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let mut got = 0;
    while got < len {
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(file, &mut buf[got..], at)?;
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(file, &mut buf[got..], at)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        got += n;
        at += n as u64;
    }
    Ok(buf)
}

fn write_all_at(file: &File, mut at: u64, mut data: &[u8]) -> io::Result<()> {
    while !data.is_empty() {
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_write(file, data, at)?;
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::write_at(file, data, at)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        at += n as u64;
        data = &data[n..];
    }
    Ok(())
}

/// `Content-Range: bytes a-b/total` → `(a, b, total)`; `total` is `None` for `*`.
fn parse_content_range(h: &HeaderMap) -> Result<(u64, u64, Option<u64>), Error> {
    let raw = h
        .get(header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let bad = || Error::BadRange(raw.to_owned());
    let rest = raw.strip_prefix("bytes ").ok_or_else(bad)?;
    let (range, total) = rest.split_once('/').ok_or_else(bad)?;
    let (a, b) = range.split_once('-').ok_or_else(bad)?;
    let a = a.trim().parse().map_err(|_| bad())?;
    let b = b.trim().parse().map_err(|_| bad())?;
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse().map_err(|_| bad())?),
    };
    Ok((a, b, total))
}

/// `Retry-After`: delay in seconds, or an HTTP date.
pub(crate) fn retry_after(h: &HeaderMap) -> Option<Duration> {
    let v = h.get(header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let when = httpdate::parse_http_date(v).ok()?;
    Some(
        when.duration_since(std::time::SystemTime::now())
            .unwrap_or_default(),
    )
}

pub(crate) async fn cancellable<T>(
    cancel: &CancellationToken,
    fut: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    tokio::select! {
        _ = cancel.cancelled() => Err(Error::Cancelled),
        r = fut => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_lead_on_as_browsers_read_them() {
        let base = Url::parse("https://site.test/download/abc").unwrap();
        let at = |v: &str| refresh_target(v, &base).map(|(d, u)| (d.as_secs(), u.to_string()));
        assert_eq!(
            at("0; url=https://cdn.test/f.zip"),
            Some((0, "https://cdn.test/f.zip".into()))
        );
        assert_eq!(
            at("5;URL='/get/f.zip'"),
            Some((5, "https://site.test/get/f.zip".into()))
        );
        assert_eq!(
            at("3, next?a=1&amp;b=2"),
            Some((3, "https://site.test/download/next?a=1&b=2".into()))
        );
        // The wait is capped; a page that reloads itself, or leads off the web, is the answer.
        assert_eq!(at("600; url=/x").map(|a| a.0), Some(PAGE_WAIT.as_secs()));
        assert_eq!(at("30"), None);
        assert_eq!(at("0; url=abc#top"), None);
        assert_eq!(at("0; url=javascript:alert(1)"), None);
        assert_eq!(at("soon; url=/x"), None);

        let page = |html: &str| meta_refresh(html.as_bytes(), &base).map(|(_, u)| u.to_string());
        assert_eq!(
            page(r#"<html><META HTTP-EQUIV="Refresh" CONTENT="0;URL=/file.zip"></html>"#),
            Some("https://site.test/file.zip".into())
        );
        assert_eq!(
            page("<meta content='2; url=https://cdn.test/a.bin' http-equiv=refresh>"),
            Some("https://cdn.test/a.bin".into())
        );
        // Other meta tags, and attribute names inside others, don't count.
        assert_eq!(
            page(
                r#"<meta name="description" content="0; url=/x"><meta data-http-equiv="refresh" content="0; url=/y">"#
            ),
            None
        );
        assert_eq!(page("<p>window.location = '/x'</p>"), None);
    }

    #[test]
    fn a_pages_cookies_go_on_with_it() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; session=old"));
        let mut set = HeaderMap::new();
        set.append(
            header::SET_COOKIE,
            HeaderValue::from_static("session=new; Path=/; HttpOnly"),
        );
        set.append(
            header::SET_COOKIE,
            HeaderValue::from_static("xsrf=t; Secure"),
        );
        add_cookies(&mut h, &set_cookies(&set));
        assert_eq!(
            header_str(&h, header::COOKIE),
            Some("a=1; session=new; xsrf=t")
        );
    }

    /// #361: a flush that took a while pushes the next one back, on a fast disk nothing changes.
    #[test]
    fn a_slow_flush_makes_the_next_one_wait() {
        let s = Duration::from_secs;
        let at = |took| Synced {
            at: std::time::Instant::now(),
            bytes: 0,
            took,
        };
        let gb = 1u64 << 30;
        // 3 s for the last flush: not again before 27 s, however many bytes came.
        assert!(!at(s(3)).due(gb, 45 * gb, s(20)));
        assert!(at(s(3)).due(gb, 45 * gb, s(27)));
        // At most SYNC_GAP_MAX, whatever the flush took.
        assert!(at(s(30)).due(gb, 45 * gb, s(61)));
        // A flush of a few milliseconds: the old rules (each tick once 64 MiB have arrived).
        assert!(at(Duration::from_millis(8)).due(128 << 20, 45 * gb, s(2)));
        assert!(!at(Duration::from_millis(8)).due(1 << 20, 45 * gb, s(2)));
        // Few bytes on a fast disk: still the long wait.
        assert!(at(Duration::from_millis(8)).due(1 << 20, 45 * gb, s(30)));
    }

    /// #518: when the forced save at the start (no connection writing yet) takes 0.5 s on a busy
    /// disk, the first save of progress still comes at the first tick, not 4.5 s later.
    #[test]
    fn a_forced_save_does_not_push_the_next_back() {
        let at = |took| Synced {
            at: std::time::Instant::now(),
            bytes: 0,
            took,
        };
        let (slow, tick) = (Duration::from_millis(500), Duration::from_secs(2));
        assert!(at(forced_gap(true, slow)).due(15 << 20, 24 << 20, tick));
        // A save during the transfer still does.
        assert!(!at(forced_gap(false, slow)).due(15 << 20, 24 << 20, tick));
    }

    #[test]
    fn flush_step_scales_with_the_file() {
        assert_eq!(sync_bytes(24 << 20), 3 << 19);
        assert_eq!(sync_bytes(100 << 10), 1 << 20);
        assert_eq!(sync_bytes(10 << 30), 64 << 20);
    }

    #[test]
    fn connections_stay_in_range() {
        let with = |n| Options {
            max_connections: n,
            ..Options::default()
        };
        assert_eq!(with(0).connections(), 1);
        assert_eq!(with(100).connections(), 100);
        assert_eq!(with(5000).connections(), MAX_CONNECTIONS_UNLOCKED);
    }

    #[tokio::test]
    async fn network_errors_read_plainly() {
        // Nothing listens on port 1.
        let e = reqwest::get("http://127.0.0.1:1/").await.unwrap_err();
        let msg = Error::from(e).to_string();
        assert!(msg.starts_with("cannot connect to the server"), "{msg}");
        assert!(
            !msg.contains("127.0.0.1") && !msg.contains("os error"),
            "{msg}"
        );
    }

    #[test]
    fn retry_after_seconds_or_date() {
        let mut h = HeaderMap::new();
        h.insert(header::RETRY_AFTER, HeaderValue::from_static("7"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
        let later = std::time::SystemTime::now() + Duration::from_secs(90);
        h.insert(
            header::RETRY_AFTER,
            HeaderValue::from_str(&httpdate::fmt_http_date(later)).unwrap(),
        );
        let d = retry_after(&h).unwrap();
        assert!(
            d > Duration::from_secs(80) && d <= Duration::from_secs(90),
            "{d:?}"
        );
        h.insert(header::RETRY_AFTER, HeaderValue::from_static("soon"));
        assert_eq!(retry_after(&h), None);
    }

    #[test]
    fn dead_links_are_not_retried() {
        for s in [401, 403, 404, 410, 416] {
            assert!(
                Error::Status(StatusCode::from_u16(s).unwrap()).link_dead(),
                "{s}"
            );
        }
        for s in [500, 502] {
            assert!(
                !Error::Status(StatusCode::from_u16(s).unwrap()).link_dead(),
                "{s}"
            );
        }
        assert!(!Error::Throttled(StatusCode::SERVICE_UNAVAILABLE, None).link_dead());
    }

    #[test]
    fn credentials_stay_with_their_host() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("sid=1"));
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic x"));
        h.insert(header::REFERER, HeaderValue::from_static("https://site/"));
        let u = |s: &str| Url::parse(s).unwrap();
        let same = scoped_headers(&h, &u("https://site/a"), &u("https://site/b"));
        assert!(same.contains_key(header::COOKIE) && same.contains_key(header::AUTHORIZATION));
        let cdn = scoped_headers(&h, &u("https://site/a"), &u("https://cdn.example/a"));
        assert!(!cdn.contains_key(header::COOKIE) && !cdn.contains_key(header::AUTHORIZATION));
        assert!(
            cdn.contains_key(header::REFERER),
            "non-credential headers stay"
        );
        let down = scoped_headers(&h, &u("https://site/a"), &u("http://site/a"));
        assert!(
            !down.contains_key(header::COOKIE),
            "no credentials over a https -> http downgrade"
        );
        let port = scoped_headers(&h, &u("http://site/a"), &u("http://site:8080/a"));
        assert!(
            !port.contains_key(header::COOKIE),
            "another port is another origin"
        );
    }

    #[test]
    fn a_drive_links_cookies_go_to_drives_own_hosts_only() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("SID=1"));
        let u = |s: &str| Url::parse(s).unwrap();
        let link = u("https://drive.google.com/file/d/x/view");
        for to in [
            "https://drive.usercontent.google.com/download?id=x",
            "https://docs.google.com/document/d/x/export",
        ] {
            assert!(
                scoped_headers(&h, &link, &u(to)).contains_key(header::COOKIE),
                "{to}"
            );
        }
        for to in [
            "https://accounts.google.com/ServiceLogin",
            "https://doc-0.docs.googleusercontent.com/export/x",
            "http://drive.usercontent.google.com/download?id=x",
        ] {
            assert!(
                !scoped_headers(&h, &link, &u(to)).contains_key(header::COOKIE),
                "{to}"
            );
        }
    }

    #[test]
    fn drive_gets_a_browsers_agent_and_only_drive() {
        let u = |s: &str| Url::parse(s).unwrap();
        let browser = Some(sites::gdrive::USER_AGENT);
        // MDM's automatic agent (the default browser's, #668), by default.
        let auto = Options::default();
        assert!(auto.automatic_agent && auto.site_agent());
        for drive in [
            "https://drive.google.com/file/d/x/view",
            "https://drive.usercontent.google.com/download?id=x",
            "http://docs.google.com:8080/document/d/x/export",
        ] {
            assert_eq!(
                request_agent(auto.site_agent(), &u(drive)),
                browser,
                "{drive}"
            );
        }
        for other in [
            "https://example.com/a.zip",
            "https://accounts.google.com/ServiceLogin",
            "https://doc-0.docs.googleusercontent.com/export/x",
            "https://drive.google.com.evil.test/x",
        ] {
            assert_eq!(request_agent(auto.site_agent(), &u(other)), None, "{other}");
        }
        // The browser's own agent (handed over with the download) or the user's wins, even
        // when it is the same text as the automatic one.
        for own in [
            "Mozilla/5.0 Firefox/140.0".to_owned(),
            crate::agent::automatic(),
        ] {
            let mut opts = Options::default();
            opts.speak_as(own);
            assert!(!opts.site_agent());
            assert_eq!(
                request_agent(opts.site_agent(), &u("https://drive.google.com/x")),
                None
            );
        }
        // MDM's former agent, asked with when a browser's is refused (#372), still gets it.
        let mut fallback = Options::default();
        fallback.speak_as(crate::agent::IE11_USER_AGENT.to_owned());
        assert!(fallback.site_agent());
    }

    #[test]
    fn content_range() {
        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 0-99/1000"),
        );
        assert_eq!(parse_content_range(&h).unwrap(), (0, 99, Some(1000)));
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 5-9/*"),
        );
        assert_eq!(parse_content_range(&h).unwrap(), (5, 9, None));
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("items 0-1/2"),
        );
        assert!(parse_content_range(&h).is_err());
    }
}
