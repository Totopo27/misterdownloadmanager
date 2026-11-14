//! Torrents through the service (#44): a magnet link goes to the user's account, the service
//! downloads the torrent on its own servers, and its files then come over HTTP through MDM's
//! engine (every connection, resume). Nothing connects to peers from this computer. A `.torrent`
//! file is sent as its magnet link (the app makes it: its hash, name and trackers).
//!
//! Each service from its public API documentation (read 2026-11-06):
//! - Real-Debrid: `torrents/addMagnet`, `torrents/info/{id}`, `torrents/selectFiles/{id}`
//!   (`files=all`), `torrents/delete/{id}`; the finished torrent's `links` are its own hoster
//!   links, unrestricted as any (`unrestrict/link`).
//! - AllDebrid: `v4/magnet/upload` (`magnets[]`), `v4.1/magnet/status` (`statusCode` 0–3 working,
//!   4 ready, 5 and up failed), `v4/magnet/files` (`id[]`, a tree of `n`/`s`/`l`/`e`),
//!   `v4/magnet/delete`; the files' links are unlocked as any (`link/unlock`).
//! - Premiumize: `transfer/create` (`src`), `transfer/list`, `folder/list` (walked) or
//!   `item/details` for a single file, `transfer/delete`; its files' links are direct links to
//!   the user's own cloud.
//! - TorBox: `torrents/createtorrent` (`magnet`), `torrents/mylist?id=`, `torrents/requestdl` for
//!   each file (a direct link), `torrents/controltorrent` (`delete`).
//! - Mega-Debrid: not yet (`ServiceKind::takes_magnets`).

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mdm_engine::download::{SegmentView, State};
use mdm_engine::{Destination, Download, ServiceFault, Snapshot};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{Debrid, Fault, ServiceKind, Session, alldebrid, premiumize, realdebrid, torbox};

/// How the service is doing with a torrent.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Still at it: how far (0 to 1, when it says), and its own word for it ("downloading").
    Working {
        progress: Option<f32>,
        what: String,
    },
    /// Done on its servers: the torrent's name and files.
    Ready {
        name: String,
        files: Vec<RemoteFile>,
    },
    Failed(ServiceFault),
}

/// One finished file at the service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFile {
    /// Its path in the torrent, `/`-separated (empty: the service names it when it is unlocked).
    pub path: String,
    /// 0: unknown.
    pub size: u64,
    /// The service's link to it: unlocked through the account (Real-Debrid, AllDebrid) or a direct
    /// link to the user's cloud (Premiumize). Never shown.
    pub link: String,
}

impl Session {
    /// Send `magnet` to the service: its id for the torrent there.
    async fn torrent_add(&self, magnet: &str) -> Result<String, Fault> {
        match self.account.service {
            ServiceKind::RealDebrid => {
                let t = self.with_token(|t| async move {
                    realdebrid::add_magnet(&self.http, &t, magnet).await
                });
                t.await
            }
            ServiceKind::AllDebrid => {
                alldebrid::add_magnet(&self.http, &self.token(None).await?, magnet).await
            }
            // Never asked: `torrent_account` skips it.
            ServiceKind::MegaDebrid => Err(Fault::Unsupported),
            ServiceKind::Premiumize => {
                premiumize::create(&self.http, &self.token(None).await?, magnet).await
            }
            ServiceKind::TorBox => {
                let key = self.token(None).await?;
                let id = torbox::create(&self.http, &key, torbox::Kind::Torrent, "magnet", magnet)
                    .await?;
                Ok(id.to_string())
            }
        }
    }

    async fn torrent_status(&self, id: &str) -> Result<Status, Fault> {
        match self.account.service {
            ServiceKind::RealDebrid => {
                self.with_token(
                    |t| async move { realdebrid::torrent_status(&self.http, &t, id).await },
                )
                .await
            }
            ServiceKind::AllDebrid => {
                alldebrid::magnet_status(&self.http, &self.token(None).await?, id).await
            }
            // Never asked: `torrent_account` skips it.
            ServiceKind::MegaDebrid => Err(Fault::Unsupported),
            ServiceKind::Premiumize => {
                premiumize::transfer_status(&self.http, &self.token(None).await?, id).await
            }
            ServiceKind::TorBox => {
                let (kind, n) = (torbox::Kind::Torrent, torbox::torrent_id(id)?);
                let key = self.token(None).await?;
                let i = torbox::item(&self.http, &key, kind, n).await?;
                match torbox::state(&i) {
                    Some(s) => Ok(s),
                    None => Ok(Status::Ready {
                        files: torbox::files(&self.http, &key, kind, n, &i).await?,
                        name: i.name,
                    }),
                }
            }
        }
    }

    async fn torrent_delete(&self, id: &str) -> Result<(), Fault> {
        match self.account.service {
            ServiceKind::RealDebrid => {
                self.with_token(
                    |t| async move { realdebrid::torrent_delete(&self.http, &t, id).await },
                )
                .await
            }
            ServiceKind::AllDebrid => {
                alldebrid::magnet_delete(&self.http, &self.token(None).await?, id).await
            }
            // Never asked: `torrent_account` skips it.
            ServiceKind::MegaDebrid => Err(Fault::Unsupported),
            ServiceKind::Premiumize => {
                premiumize::transfer_delete(&self.http, &self.token(None).await?, id).await
            }
            ServiceKind::TorBox => {
                let n = torbox::torrent_id(id)?;
                torbox::delete(
                    &self.http,
                    &self.token(None).await?,
                    torbox::Kind::Torrent,
                    n,
                )
                .await
            }
        }
    }
}

impl Debrid {
    /// The account torrents go to: the first one whose service takes magnets.
    pub fn torrent_account(&self) -> Option<(u64, ServiceKind)> {
        self.sessions
            .read()
            .unwrap()
            .iter()
            .find(|s| s.account.service.takes_magnets())
            .map(|s| (s.account.id, s.account.service))
    }

    pub(crate) fn session(&self, id: u64) -> Option<Arc<Session>> {
        self.sessions
            .read()
            .unwrap()
            .iter()
            .find(|s| s.account.id == id)
            .cloned()
    }
}

/// A path from the service, made safe to write under the torrent's folder: each part a file
/// name MDM accepts, no `..`, no root, nothing empty. None: nothing usable is left.
pub fn safe_path(path: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.split(['/', '\\']) {
        let part = part.trim();
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        let clean = mdm_engine::naming::sanitize(part);
        if clean.is_empty()
            || Path::new(&clean)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return None;
        }
        out.push(clean);
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// What the app gives a torrent through the service.
#[derive(Clone, Debug)]
pub struct Order {
    /// The account it goes to.
    pub account: u64,
    /// The magnet link (also the list's address).
    pub magnet: Url,
    /// Its id at the service, when it was sent already (a resumed download).
    pub transfer: Option<String>,
    /// Its folder: the files go under it, in their paths.
    pub folder: PathBuf,
    /// The downloads' options (connections, proxy, DNS, adapter, speed limit); the unlocker is
    /// set here.
    pub net: mdm_engine::Options,
    /// How often the service is asked while it works.
    pub poll: Duration,
}

#[derive(Default)]
struct Shared {
    state: Option<State>,
    transfer: Option<String>,
    /// The event log: its own notes and the finished files' events, in order (stable, so the
    /// list copies each once); the running file's follow.
    events: Vec<(std::time::SystemTime, String)>,
    error: Option<String>,
    failure: Option<ServiceFault>,
    /// The service's name, for the failure.
    service: &'static str,
    /// The service at work on it: how far, when it says (`at_service`).
    working: Option<Option<f32>>,
    /// Bytes of the files done, and of all of them.
    done: u64,
    total: Option<u64>,
    current: Option<Download>,
    folder: Option<PathBuf>,
    saved_files: Vec<PathBuf>,
    preexisting_folder: bool,
}

/// A torrent going through the service, then its files downloading: what the list shows as one
/// download.
#[derive(Clone)]
pub struct RemoteTorrent {
    shared: Arc<Mutex<Shared>>,
    cancel: CancellationToken,
    url: Url,
}

impl std::fmt::Debug for RemoteTorrent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoteTorrent")
    }
}

impl RemoteTorrent {
    /// Start (or continue) `order` through `debrid`'s account. Runs on the current tokio runtime.
    pub fn start(debrid: &Debrid, order: Order) -> RemoteTorrent {
        let shared = Arc::new(Mutex::new(Shared {
            state: Some(State::Probing),
            transfer: order.transfer.clone(),
            ..Default::default()
        }));
        let me = RemoteTorrent {
            shared,
            cancel: CancellationToken::new(),
            url: order.magnet.clone(),
        };
        let session = debrid.session(order.account);
        if let Some(s) = &session {
            me.shared.lock().unwrap().service = s.account.service.name();
        }
        let run = me.clone();
        tokio::spawn(async move {
            let out = match session {
                Some(s) => run.run(s, order).await,
                None => Err(Fault::Service(ServiceFault::SignIn)),
            };
            let mut sh = run.shared.lock().unwrap();
            sh.current = None;
            match out {
                Ok(()) => sh.state = Some(State::Done),
                Err(_) if run.cancel.is_cancelled() => sh.state = Some(State::Cancelled),
                Err(f) => {
                    let f = f.service();
                    sh.error = Some(f.to_string());
                    sh.failure = Some(f);
                    sh.state = Some(State::Failed);
                }
            }
        });
        me
    }

    fn note(&self, text: String) {
        self.shared
            .lock()
            .unwrap()
            .events
            .push((std::time::SystemTime::now(), text));
    }

    async fn run(&self, s: Arc<Session>, order: Order) -> Result<(), Fault> {
        let service = s.account.service.name();
        let cancelled = || Fault::Service(ServiceFault::Other("cancelled".into()));
        let known = self.shared.lock().unwrap().transfer.clone();
        let id = match known {
            Some(id) => id,
            None => {
                let id = s.torrent_add(order.magnet.as_str()).await?;
                self.note(format!("Sent to {service}"));
                self.shared.lock().unwrap().transfer = Some(id.clone());
                id
            }
        };
        // The service at work: asked every `poll`, the event log told when its word or the
        // percentage (by tens) changes.
        let mut said = String::new();
        let (name, files) = loop {
            match s.torrent_status(&id).await? {
                Status::Ready { name, files } => break (name, files),
                Status::Failed(f) => return Err(Fault::Service(f)),
                Status::Working { progress, what } => {
                    self.shared.lock().unwrap().working = Some(progress);
                    let line = match progress {
                        Some(p) => format!(
                            "{service} is downloading the torrent: {what}, {}%",
                            ((p.clamp(0.0, 1.0) * 10.0).floor() * 10.0) as u32
                        ),
                        None => format!("{service} is downloading the torrent: {what}"),
                    };
                    if line != said {
                        self.note(line.clone());
                        said = line;
                    }
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(order.poll) => {}
                _ = self.cancel.cancelled() => return Err(cancelled()),
            }
        };
        self.shared.lock().unwrap().working = None;
        self.note(format!(
            "{service} has the torrent: {} file(s), downloading them",
            files.len()
        ));
        let total: u64 = files.iter().map(|f| f.size).sum();
        self.shared.lock().unwrap().total = (total > 0).then_some(total);
        // Real-Debrid's and AllDebrid's links are their hoster links: unlocked through the
        // account, as any. Premiumize's and TorBox's are direct.
        let unlocker = match s.account.service {
            ServiceKind::Premiumize | ServiceKind::TorBox => None,
            _ => Some(Arc::new(crate::AccountUnlocker { session: s.clone() })
                as Arc<dyn mdm_engine::Unlocker>),
        };
        // As torrent programs do: one file goes into the folder itself, several into a folder
        // named after the torrent (a path that already starts with that name isn't doubled).
        let folder = if files.len() == 1 {
            order.folder.clone()
        } else {
            order
                .folder
                .join(safe_path(&name).unwrap_or_else(|| PathBuf::from("torrent")))
        };
        let within = |path: &str| {
            let t = path.trim_start_matches(['/', '\\']);
            // The one file by its name, whatever folder the service keeps it in (TorBox's
            // `<torrent>/<file>`).
            if files.len() == 1 {
                return t.rsplit(['/', '\\']).next().unwrap_or(t).to_owned();
            }
            match t.split_once(['/', '\\']) {
                Some((first, rest)) if files.len() > 1 && first.trim() == name.trim() => {
                    rest.to_owned()
                }
                _ => t.to_owned(),
            }
        };
        if files.len() > 1
            && order.transfer.is_none()
            && std::fs::read_dir(&folder).is_ok_and(|mut rd| rd.next().is_some())
        {
            self.shared.lock().unwrap().preexisting_folder = true;
        }
        std::fs::create_dir_all(&folder)
            .map_err(|e| Fault::Service(ServiceFault::Other(e.to_string())))?;
        for f in &files {
            let link = Url::parse(&f.link).map_err(|_| {
                Fault::Service(ServiceFault::Other(
                    "the service gave a link MDM can't read".into(),
                ))
            })?;
            let dest = match safe_path(&within(&f.path)) {
                Some(rel) => {
                    let path = folder.join(rel);
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir)
                            .map_err(|e| Fault::Service(ServiceFault::Other(e.to_string())))?;
                    }
                    path
                }
                // No usable path: the service's own name for it, in the folder.
                None if f.path.trim().is_empty() => PathBuf::new(),
                None => return Err(Fault::Service(ServiceFault::UnsafeName(f.path.clone()))),
            };
            if !dest.as_os_str().is_empty()
                && dest.is_file()
                && !mdm_engine::state::state_path(&dest).exists()
            {
                // Done in an earlier run.
                let mut sh = self.shared.lock().unwrap();
                sh.done += f.size;
                sh.saved_files.push(dest);
                continue;
            }
            let opts = mdm_engine::Options {
                unlocker: unlocker.clone(),
                ..order.net.clone()
            };
            let state = mdm_engine::state::state_path(&dest);
            let dl = if !dest.as_os_str().is_empty() && state.exists() {
                Download::resume(state, Default::default(), opts.clone())
                    .unwrap_or_else(|_| Download::start(request(&link, &dest, &folder), opts))
            } else {
                Download::start(request(&link, &dest, &folder), opts)
            };
            self.shared.lock().unwrap().current = Some(dl);
            // Until it ends (the snapshot reads it meanwhile); then its events join the log.
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                    _ = self.cancel.cancelled() => {
                        if let Some(d) = &self.shared.lock().unwrap().current {
                            d.cancel();
                        }
                    }
                }
                let sh = self.shared.lock().unwrap();
                let ended = sh.current.as_ref().is_none_or(|d| {
                    matches!(
                        d.snapshot().state,
                        State::Done | State::Failed | State::Cancelled
                    )
                });
                if ended {
                    break;
                }
            }
            let dl = {
                let mut sh = self.shared.lock().unwrap();
                let dl = sh.current.take().expect("the file's download");
                let ev = dl.snapshot().events;
                sh.events.extend(ev.into_iter().map(|e| (e.at, e.text)));
                dl
            };
            if self.cancel.is_cancelled() {
                let _ = dl.wait().await;
                return Err(cancelled());
            }
            let out = dl.wait().await.map_err(|e| match e {
                mdm_engine::Error::Refused(mdm_engine::Refusal::Service { fault, .. }) => {
                    Fault::Service(fault)
                }
                e => Fault::Service(ServiceFault::Other(e.to_string())),
            })?;
            let mut sh = self.shared.lock().unwrap();
            sh.done += out.size;
            sh.saved_files.push(out.path.clone());
            // One file: the download is that file.
            if files.len() == 1 {
                sh.folder = Some(out.path);
            }
        }
        // Everything is here: the torrent goes from the account's list (Premiumize: the record
        // only; the files stay in the user's cloud).
        match s.torrent_delete(&id).await {
            Ok(()) => self.note(format!("Removed from {service}'s list")),
            Err(e) => self.note(format!(
                "Not removed from {service}'s list: {}",
                e.service()
            )),
        }
        let mut sh = self.shared.lock().unwrap();
        if files.len() != 1 || sh.folder.is_none() {
            sh.folder = Some(folder);
        }
        Ok(())
    }

    /// The files written for this torrent, for checking whether its folder holds untracked files.
    pub fn saved_files(&self) -> Vec<PathBuf> {
        self.shared.lock().unwrap().saved_files.clone()
    }

    /// True when the multi-file folder already existed with files before this transfer started.
    pub fn preexisting_folder(&self) -> bool {
        self.shared.lock().unwrap().preexisting_folder
    }

    /// Its id at the service, once sent (the app keeps it, to continue after a restart).
    pub fn transfer(&self) -> Option<String> {
        self.shared.lock().unwrap().transfer.clone()
    }

    /// While the service downloads the torrent (before any file comes): its name, and how far
    /// it is (0 to 1) when it says.
    pub fn at_service(&self) -> Option<(&'static str, Option<f32>)> {
        let sh = self.shared.lock().unwrap();
        let running = matches!(sh.state, Some(State::Probing));
        sh.working.filter(|_| running).map(|p| (sh.service, p))
    }

    /// Why it stopped, in the service's terms.
    pub fn failure(&self) -> Option<ServiceFault> {
        self.shared.lock().unwrap().failure.clone()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
        if let Some(d) = &self.shared.lock().unwrap().current {
            d.cancel();
        }
    }

    pub fn set_speed_limit(&self, rate: Option<u64>) {
        if let Some(d) = &self.shared.lock().unwrap().current {
            d.set_speed_limit(rate);
        }
    }

    /// As one download: the service's work first (no bytes yet), then its files' bytes in all.
    pub fn snapshot(&self) -> Snapshot {
        let sh = self.shared.lock().unwrap();
        let cur = sh.current.as_ref().map(Download::snapshot);
        let downloaded = sh.done + cur.as_ref().map_or(0, |c| c.downloaded);
        let state = sh.state.unwrap_or(State::Probing);
        let state = match (state, &cur) {
            (State::Probing, Some(_)) => State::Downloading,
            (s, _) => s,
        };
        // A file whose link the service prepares (AllDebrid's delayed links).
        let at_service = cur.as_ref().and_then(|c| c.at_service.clone());
        let (connections, server, running) = match cur {
            Some(c) => (
                c.connections,
                c.server,
                c.events.into_iter().map(|e| (e.at, e.text)).collect(),
            ),
            None => (Vec::new(), None, Vec::new()),
        };
        let events = sh
            .events
            .iter()
            .cloned()
            .chain(running)
            .enumerate()
            .map(|(i, (at, text))| mdm_engine::download::Event {
                seq: i as u64 + 1,
                at,
                text,
            })
            .collect();
        Snapshot {
            state,
            url: self.url.clone(),
            file: sh.folder.clone(),
            size: sh.total,
            downloaded,
            resumable: Some(true),
            segments: sh
                .total
                .map(|t| {
                    vec![SegmentView {
                        start: 0,
                        cursor: downloaded.min(t),
                        end: t,
                    }]
                })
                .unwrap_or_default(),
            connections,
            error: sh.error.clone(),
            // Said in the interface language by the app, as a link's refusal is.
            failure: sh.failure.clone().map(|fault| {
                mdm_engine::Error::Refused(mdm_engine::Refusal::Service {
                    service: sh.service.to_owned(),
                    fault,
                })
            }),
            events,
            server,
            at_service,
            finishing: None,
        }
    }
}

fn request(link: &Url, dest: &Path, folder: &Path) -> mdm_engine::Request {
    mdm_engine::Request {
        url: link.clone(),
        headers: Default::default(),
        dest: if dest.as_os_str().is_empty() {
            Destination::Dir(folder.to_owned())
        } else {
            Destination::File(dest.to_owned())
        },
    }
}

// ---- each service's answers -------------------------------------------------------------------

/// Real-Debrid's `torrents/info/{id}`.
#[derive(Debug, Deserialize)]
pub(crate) struct RdInfo {
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub files: Vec<RdFile>,
    #[serde(default)]
    pub links: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RdFile {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub selected: i64,
}

/// What Real-Debrid's answer means. `waiting_files_selection` is answered by the caller.
pub(crate) fn rd_status(i: RdInfo) -> Status {
    use ServiceFault as F;
    match i.status.as_str() {
        "downloaded" => {
            let chosen: Vec<&RdFile> = i.files.iter().filter(|f| f.selected == 1).collect();
            // One link per chosen file, in order; otherwise (packed in an archive) the service
            // names each link's file itself.
            let files = if chosen.len() == i.links.len() {
                chosen
                    .iter()
                    .zip(&i.links)
                    .map(|(f, l)| RemoteFile {
                        path: f.path.clone(),
                        size: f.bytes,
                        link: l.clone(),
                    })
                    .collect()
            } else {
                i.links
                    .iter()
                    .map(|l| RemoteFile {
                        path: String::new(),
                        size: 0,
                        link: l.clone(),
                    })
                    .collect()
            };
            Status::Ready {
                name: i.filename,
                files,
            }
        }
        "magnet_error" | "dead" => Status::Failed(F::TorrentUnavailable),
        "virus" => Status::Failed(F::FileRefused),
        "error" => Status::Failed(F::ServiceDown),
        what => Status::Working {
            progress: Some((i.progress / 100.0) as f32),
            what: what.replace('_', " "),
        },
    }
}

/// AllDebrid's `magnet/status` entry (v4.1).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdMagnet {
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub status_code: i64,
    #[serde(default)]
    pub downloaded: u64,
}

/// What AllDebrid's `statusCode` means (Ready is completed by `magnet/files`).
pub(crate) fn ad_status(m: &AdMagnet) -> Option<Status> {
    use ServiceFault as F;
    match m.status_code {
        0..=3 => Some(Status::Working {
            progress: (m.size > 0).then(|| m.downloaded as f32 / m.size as f32),
            what: m.status.to_lowercase(),
        }),
        4 => None,
        11 => Some(Status::Failed(F::FileUnavailable)),
        15 => Some(Status::Failed(F::TorrentUnavailable)),
        _ => Some(Status::Failed(F::Other(if m.status.is_empty() {
            format!("error {}", m.status_code)
        } else {
            m.status.clone()
        }))),
    }
}

/// AllDebrid's `magnet/files` tree: a folder (`n`, `e`) or a file (`n`, `s`, `l`).
#[derive(Debug, Deserialize)]
pub(crate) struct AdNode {
    #[serde(default)]
    pub n: String,
    #[serde(default)]
    pub s: u64,
    pub l: Option<String>,
    #[serde(default)]
    pub e: Vec<AdNode>,
}

pub(crate) fn ad_flatten(nodes: &[AdNode], prefix: &str, out: &mut Vec<RemoteFile>) {
    for n in nodes {
        let path = if prefix.is_empty() {
            n.n.clone()
        } else {
            format!("{prefix}/{}", n.n)
        };
        match &n.l {
            Some(l) => out.push(RemoteFile {
                path,
                size: n.s,
                link: l.clone(),
            }),
            None => ad_flatten(&n.e, &path, out),
        }
    }
}

/// Premiumize's `transfer/list` entry.
#[derive(Debug, Deserialize)]
pub(crate) struct PmTransfer {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub progress: Option<f64>,
    #[serde(default)]
    pub message: Option<String>,
    pub folder_id: Option<String>,
    pub file_id: Option<String>,
}

pub(crate) fn pm_working(t: &PmTransfer) -> Option<Status> {
    match t.status.as_str() {
        "finished" | "seeding" => None,
        "error" | "deleted" | "banned" | "timeout" => Some(Status::Failed(ServiceFault::Other(
            t.message
                .clone()
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| t.status.clone()),
        ))),
        what => Some(Status::Working {
            progress: t.progress.map(|p| p as f32),
            what: what.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_from_the_service_stay_in_the_torrents_folder() {
        assert_eq!(
            safe_path("Show/S01/e1.mkv"),
            Some(PathBuf::from("Show").join("S01").join("e1.mkv"))
        );
        assert_eq!(
            safe_path("/Show/e1.mkv"),
            Some(PathBuf::from("Show").join("e1.mkv"))
        );
        assert_eq!(
            safe_path("a\\b.txt"),
            Some(PathBuf::from("a").join("b.txt"))
        );
        assert_eq!(safe_path("../evil.exe"), None);
        assert_eq!(safe_path("ok/../../evil"), None);
        assert_eq!(safe_path(""), None);
        assert_eq!(safe_path("///"), None);
        // Each part a name MDM writes: no drive, no reserved characters.
        let p = safe_path("C:/x<y>.txt").unwrap();
        assert!(
            p.components().all(|c| matches!(c, Component::Normal(_))),
            "{p:?}"
        );
    }

    #[test]
    fn real_debrids_documented_states() {
        let ready: RdInfo = serde_json::from_str(
            r#"{"id":"T","filename":"Show","hash":"h","bytes":3,"original_bytes":4,"host":"real-debrid.com",
            "split":2000,"progress":100,"status":"downloaded","added":"2026-11-06T00:00:00.000Z",
            "files":[{"id":1,"path":"/Show/a.mkv","bytes":2,"selected":1},{"id":2,"path":"/Show/x.nfo","bytes":1,"selected":0},
            {"id":3,"path":"/Show/b.mkv","bytes":1,"selected":1}],
            "links":["https://real-debrid.com/d/A","https://real-debrid.com/d/B"],"ended":"2026-11-06T00:00:00.000Z"}"#,
        )
        .unwrap();
        let Status::Ready { name, files } = rd_status(ready) else {
            panic!("ready")
        };
        assert_eq!(name, "Show");
        assert_eq!(
            files
                .iter()
                .map(|f| (f.path.as_str(), f.link.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("/Show/a.mkv", "https://real-debrid.com/d/A"),
                ("/Show/b.mkv", "https://real-debrid.com/d/B")
            ]
        );
        let busy: RdInfo = serde_json::from_str(
            r#"{"status":"downloading","progress":42.5,"files":[],"links":[]}"#,
        )
        .unwrap();
        assert_eq!(
            rd_status(busy),
            Status::Working {
                progress: Some(0.425),
                what: "downloading".into()
            }
        );
        let dead: RdInfo = serde_json::from_str(r#"{"status":"dead"}"#).unwrap();
        assert_eq!(
            rd_status(dead),
            Status::Failed(ServiceFault::TorrentUnavailable)
        );
    }

    #[test]
    fn alldebrids_documented_codes_and_tree() {
        let m = |code, status: &str| AdMagnet {
            filename: "x".into(),
            size: 100,
            status: status.into(),
            status_code: code,
            downloaded: 25,
        };
        assert_eq!(
            ad_status(&m(1, "Downloading")),
            Some(Status::Working {
                progress: Some(0.25),
                what: "downloading".into()
            })
        );
        assert_eq!(ad_status(&m(4, "Ready")), None);
        assert_eq!(
            ad_status(&m(15, "File not available - no peer")),
            Some(Status::Failed(ServiceFault::TorrentUnavailable))
        );
        let tree: Vec<AdNode> = serde_json::from_str(
            r#"[{"n":"Show","e":[{"n":"a.mkv","s":2,"l":"https://alldebrid.com/f/A"},{"n":"Sub","e":[{"n":"b.srt","s":1,"l":"https://alldebrid.com/f/B"}]}]}]"#,
        )
        .unwrap();
        let mut files = Vec::new();
        ad_flatten(&tree, "", &mut files);
        assert_eq!(
            files
                .iter()
                .map(|f| (f.path.as_str(), f.size))
                .collect::<Vec<_>>(),
            vec![("Show/a.mkv", 2), ("Show/Sub/b.srt", 1)]
        );
    }
}
