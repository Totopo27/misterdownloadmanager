//! The lab HTTP server: generated files with Range support, a request log, and fault injection.
//!
//! Routes (every parameter is listed by `GET /`, [`HELP`]):
//! - `GET|HEAD /gen/{size}[/{name}]?seed=N&<file params>&<faults>` — deterministic content (see
//!   `content`); the file params shape its headers (`cd`, `cdstar`, `inline`)
//! - `POST /form/{size}[/{name}]?need=FIELD=VALUE[&see_other=1]&<as /gen>` — the same file, only
//!   for a form post that carries `FIELD=VALUE`
//! - `GET /files/{name}` — a real file from `--files DIR` (Range too; 404 without it): archives
//!   for the link grabber's tests
//! - `GET /stream.php`, `/remote_control.php` — a generated file a server's program sends, typed
//!   by `type` and named by nothing but a `file=` value (#695, #800)
//! - `GET /redirect?to=URL[&ttl_ms=N][&uses=N][&code=3xx]` — a redirect (302 unless `code` says)
//!   to `URL` (cross-host redirects, credential scoping); with `ttl_ms` the target gets
//!   `expires=<unix ms>` and answers 403 after it (signed CDN links); with `uses` it is a link of
//!   its own that answers 403 after N requests (a link that expires however slow the machine)
//! - `GET /hop?to=URL&how=meta|header[&wait=S][&cookie=N=V]` — a page that sends the browser on to
//!   `URL` itself (`<meta http-equiv="refresh">` or a `Refresh` header), as download sites' waiting pages do
//! - `GET /page`, `/formpage`, `/cookie`, `/assets/*` — web pages for the browser tests (`pages`)
//! - `GET /hls/*`, `/dash/*` — adaptive streams (`streams`); `/hls2/*`, `/dash2/*` the same with
//!   no `.m3u8`/`.mpd` in their addresses, served as text/plain or octet-stream (#171)
//! - Google Drive's addresses on its own host names (`drive`): links, the virus-scan and quota
//!   pages, Docs exports, the sign-in page
//! - `GET /_log` — the request log as JSON; `DELETE /_log` clears it
//!
//! Every request is logged except `/`, `/_log` and `/redirect` (the engine's tests read the log
//! as the requests that reached the files, the redirect hop excluded).

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use axum::body::{Body, HttpBody};
use axum::extract::{ConnectInfo, Path, RawQuery, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use bytes::Bytes;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::content;
use crate::faults::{Faults, every, has_cookie, parse_flag};
use crate::{debrid, drive, pages, streams};

const CHUNK: u64 = 64 * 1024;

/// One served request. `peer` is the client's ip:port, so the port identifies the TCP
/// connection; `conn_req` > 1 means the connection was reused (keep-alive). `req` numbers every
/// logged request; the faults' "every Nth" count only the file requests (/gen, /form).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub req: u64,
    pub peer: SocketAddr,
    pub conn_req: u64,
    pub t_start_ms: u64,
    pub t_end_ms: u64,
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub range: Option<String>,
    pub if_range: Option<String>,
    pub user_agent: Option<String>,
    /// The `Cookie` header, to check credentials don't follow cross-host redirects.
    #[serde(default)]
    pub cookie: Option<String>,
    /// The `Host` header: which host a request through the lab as an HTTP proxy was for (its
    /// absolute address, `GET http://www.youtube.com/…`, routed by its path like any other).
    #[serde(default)]
    pub host: Option<String>,
    /// The `Authorization` header, never in full: its scheme and first 4 characters ("Bearer
    /// good…"), to check a service token reaches its API only (#44).
    #[serde(default)]
    pub authorization: Option<String>,
    /// The `Referer` header.
    #[serde(default)]
    pub referer: Option<String>,
    /// The `Origin` header (#900: a stream's server that checks it).
    #[serde(default)]
    pub origin: Option<String>,
    /// The `Sec-Fetch-Mode` header, which browsers send and programs don't (`browser_only`).
    #[serde(default)]
    pub sec_fetch_mode: Option<String>,
    /// The `Accept-Encoding` header.
    #[serde(default)]
    pub accept_encoding: Option<String>,
    /// Over HTTPS (`Config::tls`): whether the connection's TLS ClientHello was a browser's as
    /// `browser_only` tells it, with GREASE values among its cipher suites (as Chrome's and
    /// every Chromium browser's are; programs' TLS libraries send none). None over plain HTTP.
    #[serde(default)]
    pub browser_tls: Option<bool>,
    /// The request's `Content-Type` (a form post's encoding).
    #[serde(default)]
    pub req_content_type: Option<String>,
    /// The request body: its length, SHA-256 (hex) and first 256 bytes (lossy UTF-8). Set for
    /// every method but GET and HEAD, and for those too when they carry a body.
    #[serde(default)]
    pub req_body_len: Option<u64>,
    #[serde(default)]
    pub req_body_sha256: Option<String>,
    #[serde(default)]
    pub req_body_head: Option<String>,
    pub status: u16,
    /// First body byte offset within the file, and planned body length (a page: its length).
    pub body_start: u64,
    pub body_len: u64,
    pub bytes_sent: u64,
    /// `complete`, `dropped` (fault), `aborted` (client closed early), `head`, `error`,
    /// `expired`, `fault:*` (the fault that answered), `method` (405), `form` (403, the form
    /// lacked its field), `see_other` (303).
    pub outcome: String,
    pub rate: Option<u64>,
    pub drop_at: Option<u64>,
    /// For a `fault:max_conns` refusal: the client's slots as the server counted them then.
    /// The server only sees that a client dropped a response when it next writes to it, so a
    /// response the client is done with can still hold a slot (#224).
    #[serde(default)]
    pub held: Vec<Held>,
    /// Counts the request as in flight until its entry is logged ([`LabServer::settled`]).
    #[serde(skip)]
    #[doc(hidden)]
    pub in_flight: InFlight,
}

/// One of a client's `max_conns` slots at a refusal ([`LogEntry::held`]): the response holding
/// it began at file offset `from` and had been sent up to `sent_to` (exclusive). A slot held by a
/// test ([`LabServer::hold_slot`]), or a response not begun yet, shows as `0..0`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    pub from: u64,
    pub sent_to: u64,
}

/// A request begun and not logged yet: one of the server's in-flight count while it lives (a
/// request dropped before it is logged counts out too). A copy doesn't count.
#[derive(Default)]
pub struct InFlight(Option<Arc<Pending>>);

impl Clone for InFlight {
    fn clone(&self) -> Self {
        InFlight(None)
    }
}

impl std::fmt::Debug for InFlight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "InFlight"
        } else {
            "Logged"
        })
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            p.count.fetch_sub(1, Ordering::SeqCst);
            p.changed.notify_waiters();
        }
    }
}

/// How many requests are in flight, and a wake-up when that changes.
#[derive(Default)]
pub struct Pending {
    count: AtomicU64,
    changed: Notify,
}

pub type LogSink = Arc<dyn Fn(&LogEntry) + Send + Sync>;

pub struct Config {
    pub bind: SocketAddr,
    pub faults: Faults,
    /// Called for every finished request (e.g. print JSON lines).
    pub on_log: Option<LogSink>,
    /// Real files served under `/files/{name}` (with Range): archives for the link grabber's and
    /// auto-extract's tests (#37), which generated bytes can't stand for.
    pub files: Option<std::path::PathBuf>,
    /// The folder whose .torrent files /rss/dir.xml lists (`feeds`).
    pub rss_dir: Option<std::path::PathBuf>,
    /// Serve HTTPS instead of HTTP, as `localhost`, with a certificate from a test CA made at
    /// start (`LabServer::ca_der`): `browser_only` then looks at the TLS handshake too (#668).
    pub tls: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            faults: Faults::default(),
            on_log: None,
            files: None,
            rss_dir: None,
            tls: false,
        }
    }
}

pub(crate) struct Shared {
    /// The server's faults ([`LabServer::set_faults`] changes them; a URL's own override them).
    faults: RwLock<Faults>,
    /// Woken when `faults` change (a stalled response checks whether it may go on).
    faults_changed: Notify,
    /// Requests begun and not logged yet.
    pending: Arc<Pending>,
    /// `Config::rss_dir`.
    pub(crate) rss_dir: Option<std::path::PathBuf>,
    started: Instant,
    /// File requests, for the faults' "every Nth".
    requests: AtomicU64,
    /// Every logged request (`LogEntry::req`).
    seq: AtomicU64,
    /// One-time links already used (path and query).
    used_once: Mutex<HashSet<String>>,
    /// Requests each signed link with `uses` has served (path and query).
    link_uses: Mutex<HashMap<String, u64>>,
    conn_reqs: Mutex<HashMap<SocketAddr, u64>>,
    /// Each client's `max_conns` slots in use.
    active: Mutex<HashMap<IpAddr, Vec<Arc<SlotUse>>>>,
    log: Mutex<Vec<LogEntry>>,
    on_log: Option<LogSink>,
    /// The fake Drive's virus-scan pages' `uuid`s handed out (`drive`).
    pub(crate) drive_uuids: Mutex<HashSet<String>>,
    files: Option<std::path::PathBuf>,
    /// Over HTTPS: each connection's ClientHello, browser-like or not (`LogEntry::browser_tls`),
    /// by the client's address.
    browser_tls: Mutex<HashMap<SocketAddr, bool>>,
}

impl Shared {
    fn ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn faults(&self) -> Faults {
        self.faults.read().unwrap().clone()
    }

    /// Resolves once the server's faults leave the byte at `at` to be sent.
    async fn unstalled(&self, at: u64) {
        loop {
            let changed = self.faults_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.faults.read().unwrap().stall_at != Some(at) {
                return;
            }
            changed.await;
        }
    }

    /// The log entry of a request that starts now; `body` is the request's body.
    pub(crate) fn begin(
        &self,
        peer: SocketAddr,
        method: &Method,
        path: String,
        query: Option<String>,
        headers: &HeaderMap,
        body: &[u8],
    ) -> LogEntry {
        let conn_req = {
            let mut m = self.conn_reqs.lock().unwrap();
            let c = m.entry(peer).or_insert(0);
            *c += 1;
            *c
        };
        let has_body = !body.is_empty() || !matches!(*method, Method::GET | Method::HEAD);
        self.pending.count.fetch_add(1, Ordering::SeqCst);
        LogEntry {
            req: self.seq.fetch_add(1, Ordering::SeqCst) + 1,
            peer,
            conn_req,
            t_start_ms: self.ms(),
            t_end_ms: 0,
            method: method.to_string(),
            path,
            query,
            range: header_str(headers, header::RANGE),
            if_range: header_str(headers, header::IF_RANGE),
            user_agent: header_str(headers, header::USER_AGENT),
            cookie: header_str(headers, header::COOKIE),
            host: header_str(headers, header::HOST),
            authorization: header_str(headers, header::AUTHORIZATION)
                .map(|a| a.chars().take(11).collect::<String>() + "…"),
            referer: header_str(headers, header::REFERER),
            origin: header_str(headers, header::ORIGIN),
            sec_fetch_mode: header_str(headers, header::HeaderName::from_static("sec-fetch-mode")),
            accept_encoding: header_str(headers, header::ACCEPT_ENCODING),
            browser_tls: self.browser_tls.lock().unwrap().get(&peer).copied(),
            req_content_type: header_str(headers, header::CONTENT_TYPE),
            req_body_len: has_body.then_some(body.len() as u64),
            req_body_sha256: has_body.then(|| hex::encode(Sha256::digest(body))),
            req_body_head: has_body
                .then(|| String::from_utf8_lossy(&body[..body.len().min(256)]).into_owned()),
            status: 0,
            body_start: 0,
            body_len: 0,
            bytes_sent: 0,
            outcome: String::new(),
            rate: None,
            drop_at: None,
            held: Vec::new(),
            in_flight: InFlight(Some(self.pending.clone())),
        }
    }

    pub(crate) fn record(&self, mut entry: LogEntry, status: u16, outcome: &str) {
        entry.status = status;
        entry.outcome = outcome.to_owned();
        entry.t_end_ms = self.ms();
        // In flight until it is in the log, so `settled` finds it there.
        let in_flight = std::mem::take(&mut entry.in_flight);
        if let Some(sink) = &self.on_log {
            sink(&entry);
        }
        self.log.lock().unwrap().push(entry);
        drop(in_flight);
    }
}

/// A running lab server. Stops when dropped.
pub struct LabServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
    /// Over HTTPS (`Config::tls`): the test CA's certificate (DER).
    ca: Option<Vec<u8>>,
}

impl LabServer {
    pub async fn start(cfg: Config) -> io::Result<Self> {
        let listener = TcpListener::bind(cfg.bind).await?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            faults: RwLock::new(cfg.faults),
            faults_changed: Notify::new(),
            pending: Arc::default(),
            rss_dir: cfg.rss_dir,
            started: Instant::now(),
            requests: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            used_once: Mutex::default(),
            link_uses: Mutex::default(),
            conn_reqs: Mutex::default(),
            active: Mutex::default(),
            log: Mutex::default(),
            on_log: cfg.on_log,
            drive_uuids: Mutex::default(),
            files: cfg.files,
            browser_tls: Mutex::default(),
        });
        let tls = match cfg.tls {
            true => Some(crate::ftp::certificate().map_err(io::Error::other)?),
            false => None,
        };
        // Routes answered from memory, logged by `log_simple`.
        let simple = Router::new()
            .route("/page", get(pages::page))
            .route("/leadon", get(pages::leadon))
            .route("/formpage", get(pages::formpage))
            .route("/cookie", get(pages::cookie))
            .route("/assets/{file}", get(pages::asset))
            .route("/hls/master.m3u8", get(streams::hls_master))
            .route("/hls/broken-audio.m3u8", get(streams::hls_broken_master))
            .route("/hls/{track}/{file}", get(streams::hls_file))
            .route("/hlsg/{site}/master.m3u8", get(streams::hlsg_master))
            .route("/hlsg/{site}/{track}/{file}", get(streams::hlsg_file))
            .route("/dash/manifest.mpd", get(streams::dash_manifest))
            .route("/dash/protected.mpd", get(streams::dash_protected))
            .route("/dash/{rep}/{file}", get(streams::dash_file))
            .route("/hls2/playlist", get(streams::hls2_playlist))
            .route("/hls2/seg", get(streams::hls2_segment))
            .route("/hls2/config", get(streams::hls2_config))
            .route("/dash2/manifest", get(streams::dash2_manifest))
            .route("/dash2/{rep}/{file}", get(streams::dash_file))
            .route_layer(axum::middleware::from_fn_with_state(
                shared.clone(),
                log_simple,
            ));
        let app = Router::new()
            .route("/", get(help))
            .route("/gen/{size}", get(generated))
            .route("/gen/{size}/{*name}", get(generated))
            .route("/form/{size}", any(form))
            .route("/form/{size}/{*name}", any(form))
            .route("/files/{*name}", get(files))
            // A file a server's program sends (#695, #800).
            .route("/stream.php", get(script))
            .route("/remote_control.php", get(script))
            .route("/redirect", get(redirect))
            .route("/hop", get(hop))
            .route("/_log", get(get_log).delete(clear_log))
            // A fake Real-Debrid (#44, `debrid`).
            .route("/rd/rest/1.0/user", get(debrid::user))
            .route("/rd/rest/1.0/hosts/domains", get(debrid::domains))
            .route("/rd/rest/1.0/unrestrict/link", post(debrid::unrestrict))
            .route("/rd/rest/1.0/disable_access_token", get(debrid::disable))
            .route("/rd/oauth/v2/device/code", get(debrid::device_code))
            .route(
                "/rd/oauth/v2/device/credentials",
                get(debrid::device_credentials),
            )
            .route("/rd/oauth/v2/token", post(debrid::token))
            // A fake AllDebrid and a fake Premiumize (#44 phase 2).
            .route("/ad/v4/user", get(debrid::ad_user))
            .route("/ad/v4/hosts/domains", get(debrid::ad_domains))
            .route("/ad/v4/link/unlock", post(debrid::ad_unlock))
            .route("/ad/v4/link/delayed", post(debrid::ad_delayed))
            .route("/ad/v4.1/pin/get", get(debrid::ad_pin))
            .route("/ad/v4/pin/check", post(debrid::ad_pin_check))
            .route("/pm/api/account/info", get(debrid::pm_account))
            .route("/pm/api/services/list", get(debrid::pm_services))
            .route("/pm/api/transfer/directdl", post(debrid::pm_directdl))
            // A fake Mega-Debrid.
            .route("/md/api.php", post(debrid::md_api))
            // Torrents through the fake services (#44, `debrid_torrents`).
            .route(
                "/rd/rest/1.0/torrents/addMagnet",
                post(crate::debrid_torrents::rd_add),
            )
            .route(
                "/rd/rest/1.0/torrents/info/{id}",
                get(crate::debrid_torrents::rd_info),
            )
            .route(
                "/rd/rest/1.0/torrents/selectFiles/{id}",
                post(crate::debrid_torrents::rd_select),
            )
            .route(
                "/rd/rest/1.0/torrents/delete/{id}",
                axum::routing::delete(crate::debrid_torrents::rd_delete),
            )
            .route(
                "/ad/v4/magnet/upload",
                post(crate::debrid_torrents::ad_upload),
            )
            .route(
                "/ad/v4.1/magnet/status",
                post(crate::debrid_torrents::ad_status),
            )
            .route(
                "/ad/v4/magnet/files",
                post(crate::debrid_torrents::ad_files),
            )
            .route(
                "/ad/v4/magnet/delete",
                post(crate::debrid_torrents::ad_delete),
            )
            .route(
                "/pm/api/transfer/create",
                post(crate::debrid_torrents::pm_create),
            )
            .route(
                "/pm/api/transfer/list",
                get(crate::debrid_torrents::pm_list),
            )
            .route(
                "/pm/api/folder/list",
                get(crate::debrid_torrents::pm_folder),
            )
            .route(
                "/pm/api/transfer/delete",
                post(crate::debrid_torrents::pm_delete),
            )
            // A fake TorBox: web downloads and torrents (`debrid_torrents`).
            .route("/tb/v1/api/user/me", get(crate::debrid_torrents::tb_user))
            .route(
                "/tb/v1/api/user/auth/device/start",
                get(crate::debrid_torrents::tb_device_start),
            )
            .route(
                "/tb/v1/api/user/auth/device/token",
                post(crate::debrid_torrents::tb_device_token),
            )
            .route(
                "/tb/v1/api/webdl/hosters",
                get(crate::debrid_torrents::tb_hosters),
            )
            .route(
                "/tb/v1/api/webdl/createwebdownload",
                post(crate::debrid_torrents::tb_web_create),
            )
            .route(
                "/tb/v1/api/torrents/createtorrent",
                post(crate::debrid_torrents::tb_torrent_create),
            )
            .route(
                "/tb/v1/api/{kind}/mylist",
                get(crate::debrid_torrents::tb_list),
            )
            .route(
                "/tb/v1/api/{kind}/requestdl",
                get(crate::debrid_torrents::tb_requestdl),
            )
            .route(
                "/tb/v1/api/webdl/controlwebdownload",
                post(crate::debrid_torrents::tb_control),
            )
            .route(
                "/tb/v1/api/torrents/controltorrent",
                post(crate::debrid_torrents::tb_control),
            )
            .route("/_debrid/torrents", get(crate::debrid_torrents::held))
            // RSS feeds for the app's RSS tests (#39, `feeds`).
            .route("/rss/shows.xml", get(crate::feeds::shows))
            .route("/rss/dir.xml", get(crate::feeds::dir))
            .route("/rss/files/{name}", get(crate::feeds::file))
            .merge(simple)
            // Every other address: the fake Google Drive's, by the Host header (`drive`).
            .fallback(drive::handle)
            .with_state(shared.clone());
        let ca = tls.as_ref().map(|(_, ca)| ca.clone());
        let task = match tls {
            None => tokio::spawn(async move {
                let svc = app.into_make_service_with_connect_info::<SocketAddr>();
                if let Err(e) = axum::serve(listener, svc).await {
                    eprintln!("labserver: {e}");
                }
            }),
            Some((config, _)) => {
                tokio::spawn(serve_tls(listener, app, Arc::new(config), shared.clone()))
            }
        };
        Ok(LabServer {
            addr,
            shared,
            task,
            ca,
        })
    }

    /// Over HTTPS (`Config::tls`): the test CA's certificate (DER); a client trusting it trusts
    /// this server as `localhost`.
    pub fn ca_der(&self) -> Option<Vec<u8>> {
        self.ca.clone()
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://<addr><path>`; `path` starts with `/`.
    /// The address of `path` (starting with `/`): `http://127.0.0.1:port`, or over HTTPS
    /// `https://localhost:port` (the certificate's name).
    pub fn url(&self, path: &str) -> String {
        match self.ca {
            None => format!("http://{}{}", self.addr, path),
            Some(_) => format!("https://localhost:{}{}", self.addr.port(), path),
        }
    }

    pub fn log(&self) -> Vec<LogEntry> {
        self.shared.log.lock().unwrap().clone()
    }

    pub fn clear_log(&self) {
        self.shared.log.lock().unwrap().clear();
    }

    /// Resolves once every request begun so far is in the log, and has given back its
    /// `max_conns` slot. A response is logged when its body is dropped, which can trail the
    /// client: after a download is stopped, the server sees its connections close only as it
    /// next writes to them.
    pub async fn settled(&self) {
        let pending = &self.shared.pending;
        loop {
            let changed = pending.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if pending.count.load(Ordering::SeqCst) == 0 {
                return;
            }
            changed.await;
        }
    }

    /// Take one of `ip`'s `max_conns` slots, as another client's response would, until the
    /// guard is dropped: a test holds a slot for exactly as long as it needs.
    pub fn hold_slot(&self, ip: IpAddr) -> SlotHold {
        SlotHold {
            _slot: ActiveSlot::take(&self.shared, ip),
        }
    }

    /// The server-wide faults (`--faults`, [`Config::faults`]).
    pub fn faults(&self) -> Faults {
        self.shared.faults()
    }

    /// Change the server-wide faults for the requests from now on, and lift a `stall_at` that
    /// responses are waiting at.
    pub fn set_faults(&self, faults: Faults) {
        *self.shared.faults.write().unwrap() = faults;
        self.shared.faults_changed.notify_waiters();
    }

    /// Resolves when the server task ends (only on error; runs until dropped otherwise).
    pub async fn wait(&mut self) {
        let _ = (&mut self.task).await;
    }
}

impl Drop for LabServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The index page (`GET /`): every route, parameter and fault.
pub const HELP: &str = r#"labserver: an HTTP test server for Mister Download Manager

FILES  deterministic content: byte i of seed s is the same on every request (src/content.rs)
GET|HEAD /gen/{size}[/{name}]?seed=N&<file params>&<faults>   size: 12345, 64K, 100M, 1G
  file params:
    cd=NAME        Content-Disposition: attachment; filename="NAME" (whatever the path's name;
                   UTF-8 is sent raw, as many servers do)
    cdstar=NAME    attachment; filename="fallback-name.zip"; filename*=UTF-8''<NAME %-encoded>
    inline=1       Content-Disposition: inline (filename: cdstar, cd or the path's name) and the
                   name's real Content-Type (.pdf application/pdf, .mp3 audio/mpeg, .mp4
                   video/mp4, ...): a browser displays or plays it
    inline=bare    the real Content-Type and no Content-Disposition at all
    expires=UNIXMS 403 after that time (what /redirect?ttl_ms= adds)
    uses=N         403 once this path and query has served N requests (what /redirect?uses= adds,
                   with a link= of its own)
  faults (also server-wide with --faults; per-URL values override):
    rate=SIZE slow_every=N slow_rate=SIZE max_conns=N drop_after=SIZE drop_every=N
    fail_every=N fail_first=N no_range=1 no_range_every=N html=1 etag_change_every=N
    html=1         a small web page (a sign-in wall, a file host's waiting page) instead of the
                   file, text/html; with html_type=TYPE|none the page comes as TYPE (none: no
                   Content-Type), with html_cd=1 under the file's name (Content-Disposition)
    html_after=N   the first N requests for this path and query get the file, every later one
                   the page (as html=1 with html_type and html_cd): a link that leads to a
                   waiting page once the download is resumed; vary seed= for a fresh link
    etag=weak|per-request|none latency_ms=N range_latency_ms=N (only requests with a Range)
    range_rate=SIZE  the rate of answers to requests with a Range (instead of rate)
    burst=SIZE     with a rate, a link: a response the client stopped reading catches up with
                   at most SIZE at once (by default all it fell behind by)
    stall_at=SIZE  never send the file's byte at that offset: a response stops there and waits
                   (until the server's faults change, from a test; a URL's own value stays)
    require_cookie=NAME=VALUE|NAME   403 unless the Cookie header has it
    require_referer=PREFIX           403 unless the Referer starts with PREFIX (%-encode ? & =)
    refuse_agent=TEXT                403 when the User-Agent contains TEXT
    refuse=STATUS  every request answered with that 4xx or 5xx status, before any byte (a site
                   that turns away whatever isn't a browser: FileHippo's 412, #630)
    browser_only=1 a bot check: 403 with cf-mitigated: challenge and a challenge page unless the
                   request carries a browser's Sec-Fetch-Mode header and, over HTTPS (--tls),
                   came on a TLS handshake with GREASE, as Chrome's has (#668)
    refuse_programs=STATUS  the same check answering STATUS, empty (FileHippo's 412, #630)
    once=1         a one-time link: the first request for this path and query is served, every
                   later one (HEAD and ranges too) gets 410 Gone; vary seed= for a fresh link
  "every Nth" counts file requests (/gen and /form) only.
POST /form/{size}[/{name}]?need=FIELD=VALUE&see_other=1&<as /gen>
    the file, only for a POST whose form (urlencoded, multipart or text/plain) has FIELD=VALUE
    (default need=token=abc): 405 to other methods, 403 to a form without it;
    see_other=1: 303 to /gen/{size}/{name}?<the rest of the query> instead
GET /files/{name}   a real file from --files DIR (Range too; 404 without --files)
GET /stream.php, /remote_control.php?id=N|file=NAME&size=1M&type=MIME|none&magic=mp4|webm|mp3&<as /gen>
    a generated file a server's program sends (#695, #800): no Content-Disposition, Content-Type
    video/mp4 (type= another, none for none at all), magic= its first bytes a media type's
GET /redirect?to=URL[&ttl_ms=N][&uses=N][&code=3xx]   302 (or code) to URL (not logged); ttl_ms: a signed link expiring after N ms; uses: a new link each time, expiring after N requests
/rd/rest/1.0/{user,hosts/domains,unrestrict/link,disable_access_token}, /rd/oauth/v2/{device/code,device/credentials,token}
                                              a fake Real-Debrid (#44): tokens good (premium), free; links http://hoster.test/file/SIZE/NAME[?gone=1&down=1&chunks=N&ttl_ms=N&rate=SIZE]
/ad/v4/{user,hosts/domains,link/unlock,link/delayed,pin/check}, /ad/v4.1/pin/get      a fake AllDebrid: the same tokens and links, plus delayed=N
/pm/api/{account/info,services/list,transfer/directdl}      a fake Premiumize: the same tokens and links
/tb/v1/api/{user/me,user/auth/device/{start,token},webdl/hosters,webdl/createwebdownload,torrents/createtorrent,*/mylist,*/requestdl,*/control*}
                                              a fake TorBox: the same tokens and links (ready after 2 polls), and torrents
POST /md/api.php?action={connectUser,getUserHistory,getHostersList,getLink}   a fake Mega-Debrid: the same keys and links, the form's token; users labuser (premium) and freeuser, password labpass
GET /hop?to=URL&how=meta|header[&wait=S][&cookie=N=V]   a page that sends the browser on to URL (meta refresh or Refresh header)

RSS    made-up feeds for the RSS feeds window (#39); never real ones
GET /rss/shows.xml?upto=N&atom=1&torrent=URL   "Open Show" S01E01..N (default 3) in 1080p and 720p, and
                   "Other Show" 1x01..N: magnet links of made-up hashes, or every article URL
                   (a .torrent to download); atom=1: an Atom feed
GET /rss/dir.xml   the .torrent files in --rss-dir, newest first, linked to /rss/files/NAME

PAGES  for the browser extension's tests (tools/extension/e2e.mjs); styles and scripts are files
GET /page?<any of>
    link=PATH      <a id=link href=PATH>download</a>; again for more (#link2, #link3...)
    video=PATH|1   <video id=video controls> 640x360 (1: /gen/1M/clip.mp4?inline=1, also shown
                   when nothing else is asked for, or with muted_loop, iframe or shadow)
    w=PX h=PX      the videos' size
    title=TEXT     the page's title (default "labserver page")
    muted_loop=1   the video muted, looping and autoplaying, without controls
    iframe=1       the video inside <iframe id=frame> (a /page of its own)
    shadow=1       the video inside the open shadow root of <div id=host> (declarative)
    several=N      N more videos #video1..N, each with its own <source> /gen/1M/clipK.mp4?seed=K
    audio=PATH|1   <audio id=audio controls> (1: /gen/512K/sound.mp3?inline=1)
    hls=1, dash=1, drm=1   a <video> and /assets/stream.js for each (#player, then #player2,
                   #player3), which fetches /hls/master.m3u8, /dash/manifest.mpd or
                   /dash/protected.mpd, the first variant's and the audio's playlists and every
                   segment, then sets <body data-stream=done> once all are (or error)
    hlsg=SITE      a player for http://SITE/hlsg/<this page's host>/master.m3u8 (another site's
                   address for this page's stream: localhost:PORT for a page on 127.0.0.1, and
                   back), which serves it only to this page's Origin and Referer (#900)
    hls2=1, dash2=1  the same for /hls2/playlist?id=lab and /dash2/manifest?id=lab (no .m3u8 or
                   .mpd, text/plain and octet-stream, #171); each player first fetches
                   /hls2/config?id=lab, a text/plain answer that is no manifest
    csp=strict     Content-Security-Policy: default-src 'none'; style-src 'self'; img-src 'self';
                   media-src 'self'; script-src 'self' (+ frame-src 'self' with iframe,
                   + connect-src 'self' and media-src blob: with hls or dash)
GET /formpage?size=1M&name=report.zip&token=abc&with=QUERY   three forms posting to /form:
    #form (urlencoded) to /form/SIZE/NAME?seed=5, #multipart to /form/SIZE/multipart-NAME?seed=6,
    #see-other (urlencoded) to /form/SIZE/see-other-NAME?seed=7&see_other=1; each sends
    token=TOKEN and note=hello world; with=QUERY is added to each action (with=once%3D1)
GET /cookie?set=NAME=VALUE&httponly=NAME=VALUE&clear=NAME   (each repeatable) a small page with
    Set-Cookie: NAME=VALUE; Path=/ (; HttpOnly), or NAME=; Path=/; Max-Age=0
GET /assets/page.css  /assets/stream.js

STREAMS  3 segments of 4 s per track. HLS: real MPEG-TS, a grey H.264 picture (25 fps) on the
    video tracks, AAC silence on the audio track (ffmpeg remuxes and merges them); DASH: fake bytes
GET /hls/master.m3u8   two variants (720p: 1280x720, 1280000 b/s; 360p: 640x360, 640000 b/s)
    and an audio rendition (GROUP-ID="aud"): /hls/{720p,360p,audio}/index.m3u8 -> seg0..2.ts
    of 48128, 24064, 6016 bytes (application/vnd.apple.mpegurl, video/mp2t)
GET /hls/broken-audio.m3u8  the same, its sound /hls/audio/broken.m3u8 -> broken0..2.ts: seeded
    bytes, no MPEG-TS, so ffmpeg can't join it with a picture (a video's failed merge)
GET /dash/manifest.mpd  (application/dash+xml) the same tracks as Representations with a
    SegmentTemplate: /dash/{720p,360p,audio}/init.mp4 and seg1..3.m4s (video/mp4, audio/mp4)
GET /dash/protected.mpd  the same, each AdaptationSet marked encrypted (CENC, Widevine's
    ContentProtection), as a DRM-protected stream's manifest is
GET /hlsg/{SITE}/master.m3u8  the HLS stream again (/hlsg/SITE/{720p,360p,audio}/...), only for
    a page of SITE (host:port): 403 to any request, the playlists' and each segment's, without
    Origin: http://SITE and a Referer under it, as a site's video server checks (#900)
GET /hls2/playlist?id=ID  the HLS stream again with no .m3u8 anywhere, all text/plain (#171):
    variants playlist?id=ID&track={720p,360p,audio}, segments seg?id=ID&track=T&i=0..2
    (application/octet-stream)
GET /dash2/manifest?id=ID  the DASH manifest as application/octet-stream, no .mpd; segments
    /dash2/{720p,360p,audio}/init.mp4 and seg1..3.m4s
GET /hls2/config?id=ID  {"autoplay":false,"volume":0.8} as text/plain: not a manifest

GOOGLE DRIVE  a fake one for the engine's tests, on Google's host names (resolve
    drive.google.com, drive.usercontent.google.com, docs.google.com, accounts.google.com and
    doc-0.docs.googleusercontent.com to this server); hand-made pages shaped like Drive's (src/drive.rs)
  IDs: KIND-SIZE[-FLAG...] padded with -000 to 25+ characters, e.g. big-8M-uuid-0000000000000
    small, uni   the file at once (uni: a non-ASCII name in filename*=)
    big          the virus-scan page unless the query has confirm= (flag uuid: and the page's uuid;
                 flag ttlN: its link expires after N ms); a Cookie SID=lab adds an at= input
    private      302 to accounts.google.com/ServiceLogin unless the Cookie has SID=lab, then as big
    quota, doc   the "Quota exceeded" page (doc: a Google Doc, as Drive's file host answers it)
    noaccess 403   rk 404 unless resourcekey=0-lab   gone 404
  drive.google.com          /file/d/ID/view (a 150 KB viewer page), /uc?id= (303 to the file
                            host), /open?id= (307 to the viewer, or a doc's editor)
  drive.usercontent.google.com  /download?id=&export=download[&confirm=&uuid=]   /uc (303)
  docs.google.com           /document|spreadsheets|presentation/d/ID/edit, .../export?format=
                            (307 to doc-0.docs.googleusercontent.com/export/tok/ID: chunked, no
                            length, ranges ignored)
  accounts.google.com       /ServiceLogin   the sign-in page

LOG
GET /_log   the request log as JSON: method, path, query, range, if_range, user_agent, cookie,
            referer, origin, req_content_type, req_body_len, req_body_sha256, req_body_head,
            status, bytes_sent, outcome (complete, head, fault:*, ...)
DELETE /_log   clears it
"#;

async fn help() -> &'static str {
    HELP
}

/// Logs a request to a route answered from memory (pages, streams), whose length is known.
async fn log_simple(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(body, 16 << 20)
        .await
        .unwrap_or_default();
    let mut entry = shared.begin(
        peer,
        &parts.method,
        parts.uri.path().to_owned(),
        parts.uri.query().map(str::to_owned),
        &parts.headers,
        &body,
    );
    let head = parts.method == Method::HEAD;
    let resp = next.run(Request::from_parts(parts, Body::from(body))).await;
    let len = resp.body().size_hint().exact().unwrap_or(0);
    entry.body_len = len;
    entry.bytes_sent = if head { 0 } else { len };
    let outcome = if head { "head" } else { "complete" };
    shared.record(entry, resp.status().as_u16(), outcome);
    resp
}

/// `/redirect?to=URL[&ttl_ms=N][&uses=N][&code=3xx]`: a redirect (302 unless `code` says) to
/// `URL`; with `ttl_ms`, to a signed link that expires; with `uses`, to a link of its own that
/// expires after that many requests.
async fn redirect(RawQuery(q): RawQuery) -> Response {
    let mut m = q
        .and_then(|q| serde_urlencoded::from_str::<HashMap<String, String>>(&q).ok())
        .unwrap_or_default();
    let Some(mut to) = m.remove("to") else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if let Some(ttl) = m.get("ttl_ms").and_then(|t| t.parse::<u64>().ok()) {
        let sep = if to.contains('?') { '&' } else { '?' };
        to = format!("{to}{sep}expires={}", unix_ms() + ttl);
    }
    if let Some(uses) = m.get("uses").and_then(|n| n.parse::<u64>().ok()) {
        static LINKS: AtomicU64 = AtomicU64::new(0);
        let sep = if to.contains('?') { '&' } else { '?' };
        let link = LINKS.fetch_add(1, Ordering::Relaxed);
        to = format!("{to}{sep}uses={uses}&link={link}");
    }
    let code = m
        .get("code")
        .and_then(|c| c.parse::<u16>().ok())
        .and_then(|c| StatusCode::from_u16(c).ok())
        .filter(StatusCode::is_redirection)
        .unwrap_or(StatusCode::FOUND);
    (code, [(header::LOCATION, to)]).into_response()
}

/// `/hop?to=URL&how=meta|header[&wait=S][&cookie=NAME=VALUE]`: a web page (200) that sends the
/// browser on to `URL` itself, as download sites' "your download starts in a moment" pages do:
/// with a `<meta http-equiv="refresh">` in it, or a `Refresh` header; `cookie` is set on the way.
async fn hop(RawQuery(q): RawQuery) -> Response {
    let m = q
        .and_then(|q| serde_urlencoded::from_str::<HashMap<String, String>>(&q).ok())
        .unwrap_or_default();
    let Some(to) = m.get("to") else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let wait = m.get("wait").map_or("0", String::as_str);
    let content = format!("{wait}; url={to}");
    let mut resp = match m.get("how").map(String::as_str) {
        Some("header") => (
            [(header::REFRESH, content)],
            Html("<p>Your download starts in a moment.</p>".to_owned()),
        )
            .into_response(),
        _ => Html(format!(
            "<!doctype html><title>Download</title><meta http-equiv=\"refresh\" \
             content=\"{}\"><p>Your download starts in a moment.</p>",
            content.replace('&', "&amp;").replace('"', "&quot;")
        ))
        .into_response(),
    };
    if let Some(cookie) = m.get("cookie").and_then(|c| c.parse().ok()) {
        resp.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    resp
}

pub(crate) fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

async fn get_log(State(s): State<Arc<Shared>>) -> Json<Vec<LogEntry>> {
    Json(s.log.lock().unwrap().clone())
}

async fn clear_log(State(s): State<Arc<Shared>>) -> StatusCode {
    s.log.lock().unwrap().clear();
    StatusCode::NO_CONTENT
}

fn header_str(h: &HeaderMap, name: header::HeaderName) -> Option<String> {
    h.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned)
}

/// A `max_conns` slot held by a test ([`LabServer::hold_slot`]), given back when dropped.
pub struct SlotHold {
    _slot: ActiveSlot,
}

/// What holds a slot: how far its response has been sent ([`Held`]).
#[derive(Default)]
struct SlotUse {
    from: AtomicU64,
    sent_to: AtomicU64,
}

impl SlotUse {
    fn held(&self) -> Held {
        Held {
            from: self.from.load(Ordering::SeqCst),
            sent_to: self.sent_to.load(Ordering::SeqCst),
        }
    }
}

/// Releases a client's concurrent-response slot (for `max_conns`) when dropped.
struct ActiveSlot {
    shared: Arc<Shared>,
    ip: IpAddr,
    used: Arc<SlotUse>,
}

impl ActiveSlot {
    /// Take one of `ip`'s slots, however many it has.
    fn take(shared: &Arc<Shared>, ip: IpAddr) -> Self {
        let mut active = shared.active.lock().unwrap();
        Self::take_locked(shared, active.entry(ip).or_default(), ip)
    }

    fn take_locked(shared: &Arc<Shared>, slots: &mut Vec<Arc<SlotUse>>, ip: IpAddr) -> Self {
        let used = Arc::new(SlotUse::default());
        slots.push(used.clone());
        ActiveSlot {
            shared: shared.clone(),
            ip,
            used,
        }
    }

    /// The response holding the slot begins at file offset `at`.
    fn begins(&self, at: u64) {
        self.used.from.store(at, Ordering::SeqCst);
        self.used.sent_to.store(at, Ordering::SeqCst);
    }
}

impl Drop for ActiveSlot {
    fn drop(&mut self) {
        let mut active = self.shared.active.lock().unwrap();
        if let Some(slots) = active.get_mut(&self.ip) {
            slots.retain(|u| !Arc::ptr_eq(u, &self.used));
        }
    }
}

/// `/{route}/{size}[/{name}]` from the path parameters.
fn file_path(route: &str, params: &HashMap<String, String>) -> (String, Option<String>, String) {
    let size = params.get("size").cloned().unwrap_or_default();
    let name = params.get("name").cloned();
    let path = format!(
        "/{route}/{size}{}",
        name.as_ref().map(|x| format!("/{x}")).unwrap_or_default()
    );
    (size, name, path)
}

async fn generated(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    Path(params): Path<HashMap<String, String>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let (size, name, path) = file_path("gen", &params);
    let entry = shared.begin(peer, &method, path, query.clone(), &headers, &[]);
    let query = query.unwrap_or_default();
    serve(shared, entry, &method, &size, name.as_deref(), &query).await
}

/// `GET /stream.php` and `/remote_control.php` (#695, #800): a generated file a server's program
/// sends, with no Content-Disposition (as such sites do): `size` (1M by default), `type` its
/// Content-Type (video/mp4 by default; `none` for no Content-Type at all), `magic` its first bytes
/// a media type's (`mp4`, `webm`, `mp3`). `id`, `file`, `filename` and `token` are the page's own
/// (a `file=` value names the file). Every generated file's key works too.
async fn script(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    uri: axum::http::Uri,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let entry = shared.begin(
        peer,
        &method,
        uri.path().to_owned(),
        query.clone(),
        &headers,
        &[],
    );
    let mut pairs: Vec<(String, String)> =
        serde_urlencoded::from_str(query.as_deref().unwrap_or_default()).unwrap_or_default();
    let size = pairs
        .iter()
        .position(|(k, _)| k == "size")
        .map_or_else(|| "1M".to_owned(), |i| pairs.remove(i).1);
    for (k, v) in [("type", "video/mp4"), ("inline", "bare")] {
        if !pairs.iter().any(|(key, _)| key == k) {
            pairs.push((k.into(), v.into()));
        }
    }
    let query = serde_urlencoded::to_string(pairs).unwrap_or_default();
    serve(shared, entry, &method, &size, None, &query).await
}

/// `GET /files/{name}`: a real file from `Config::files` (404 without one, or for a name that
/// leaves that folder), whole or one range of it.
async fn files(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let entry = shared.begin(peer, &method, format!("/files/{name}"), None, &headers, &[]);
    let inside = !name
        .split(['/', '\\'])
        .any(|c| c == ".." || c.contains(':'));
    let data = match shared.files.as_ref().filter(|_| inside) {
        Some(dir) => tokio::fs::read(dir.join(&name)).await.ok(),
        None => None,
    };
    let Some(data) = data else {
        shared.record(entry, 404, "error");
        return (StatusCode::NOT_FOUND, "no such file\n").into_response();
    };
    let size = data.len() as u64;
    let range = entry
        .range
        .as_deref()
        .and_then(|r| http_range_header::parse_range_header(r).ok())
        .and_then(|r| r.validate(size).ok())
        .filter(|v| v.len() == 1)
        .map(|v| (*v[0].start(), *v[0].end() + 1));
    let (start, end, status) = match range {
        Some((s, e)) => (s, e, StatusCode::PARTIAL_CONTENT),
        None => (0, size, StatusCode::OK),
    };
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, end - start)
        .header(header::ACCEPT_RANGES, "bytes");
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{}/{size}", end - 1),
        );
    }
    shared.record(entry, status.as_u16(), "files");
    if method == Method::HEAD {
        return builder.body(Body::empty()).unwrap();
    }
    builder
        .body(Body::from(data[start as usize..end as usize].to_vec()))
        .unwrap()
}

/// `POST /form/{size}[/{name}]?need=FIELD=VALUE&see_other=1&<as /gen>`: the file, only for a
/// POST whose form carries `FIELD=VALUE` (default `token=abc`); 405 to any other method, 403 to
/// a form without it; with `see_other`, 303 to the file on /gen instead.
async fn form(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    Path(params): Path<HashMap<String, String>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (size, name, path) = file_path("form", &params);
    let entry = shared.begin(peer, &method, path, query.clone(), &headers, &body);
    if method != Method::POST {
        shared.record(entry, 405, "method");
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "POST")],
            "POST a form here\n",
        )
            .into_response();
    }
    let pairs: Vec<(String, String)> =
        match serde_urlencoded::from_str(query.as_deref().unwrap_or("")) {
            Ok(p) => p,
            Err(e) => {
                shared.record(entry, 400, "error");
                return (StatusCode::BAD_REQUEST, format!("{e}\n")).into_response();
            }
        };
    let mut need = "token=abc".to_owned();
    let mut see_other = false;
    let mut rest = Vec::new();
    for (k, v) in pairs {
        match k.as_str() {
            "need" => need = v,
            "see_other" => see_other = parse_flag(&v).unwrap_or(false),
            _ => rest.push((k, v)),
        }
    }
    let rest = serde_urlencoded::to_string(rest).unwrap_or_default();
    let (field, value) = need.split_once('=').unwrap_or((&need, ""));
    let fields = form_fields(entry.req_content_type.as_deref(), &body);
    if !fields.iter().any(|(k, v)| k == field && v == value) {
        shared.record(entry, 403, "form");
        return (StatusCode::FORBIDDEN, format!("the form lacks {need}\n")).into_response();
    }
    if see_other {
        let to = format!(
            "/gen/{size}{}{}{rest}",
            name.as_ref().map(|x| format!("/{x}")).unwrap_or_default(),
            if rest.is_empty() { "" } else { "?" }
        );
        shared.record(entry, 303, "see_other");
        return (StatusCode::SEE_OTHER, [(header::LOCATION, to)]).into_response();
    }
    serve(shared, entry, &method, &size, name.as_deref(), &rest).await
}

/// A form body's fields: `application/x-www-form-urlencoded` (also when no type is given),
/// `multipart/form-data` (text values) or `text/plain` (`name=value` lines).
pub(crate) fn form_fields(content_type: Option<&str>, body: &[u8]) -> Vec<(String, String)> {
    let ct = content_type.unwrap_or("");
    let mime = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "multipart/form-data" => multipart_fields(ct, body),
        "text/plain" => String::from_utf8_lossy(body)
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
        _ => serde_urlencoded::from_bytes(body).unwrap_or_default(),
    }
}

fn multipart_fields(ct: &str, body: &[u8]) -> Vec<(String, String)> {
    let Some(boundary) = ct
        .split(';')
        .find_map(|p| p.trim().strip_prefix("boundary="))
        .map(|b| b.trim_matches('"'))
    else {
        return Vec::new();
    };
    let delim = format!("--{boundary}");
    let text = String::from_utf8_lossy(body);
    let mut out = Vec::new();
    for part in text.split(delim.as_str()).skip(1) {
        if part.starts_with("--") {
            break; // the closing delimiter
        }
        let part = part.strip_prefix("\r\n").unwrap_or(part);
        let Some((head, value)) = part.split_once("\r\n\r\n") else {
            continue;
        };
        let name = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-disposition:"))
            .and_then(|l| {
                l.split(';')
                    .find_map(|p| p.trim().strip_prefix("name="))
                    .map(|n| n.trim_matches('"').to_owned())
            });
        if let Some(name) = name {
            out.push((name, value.strip_suffix("\r\n").unwrap_or(value).to_owned()));
        }
    }
    out
}

/// How a generated file is presented: `inline=` absent (attachment), `1` or `bare`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Disposition {
    Attachment,
    Inline,
    Bare,
}

/// A generated file's query: content, validity and headers, and the fault overrides.
struct FileQuery {
    seed: u64,
    /// A signed link's end, unix ms.
    expires: Option<u64>,
    /// The requests a signed link serves (`link` tells the links apart).
    uses: Option<u64>,
    cd: Option<String>,
    cdstar: Option<String>,
    disposition: Disposition,
    /// `type`: the Content-Type sent instead of the name's (`none`: none at all).
    content_type: Option<String>,
    /// `magic`: the file starts as a media type's ([`content::media_head`]).
    magic: Option<&'static [u8]>,
    faults: Faults,
}

/// The lab server over HTTPS (`Config::tls`): each connection's ClientHello is read first, and
/// whether it is a browser's noted for `browser_only` (`LogEntry::browser_tls`), then its
/// requests are served as over HTTP (HTTP/1.1).
async fn serve_tls(
    listener: TcpListener,
    app: Router,
    config: Arc<tokio_rustls::rustls::ServerConfig>,
    shared: Arc<Shared>,
) {
    use hyper_util::rt::{TokioExecutor, TokioIo};
    loop {
        let Ok((tcp, peer)) = listener.accept().await else {
            continue;
        };
        let (app, config, shared) = (app.clone(), config.clone(), shared.clone());
        tokio::spawn(async move {
            let accepting = tokio_rustls::LazyConfigAcceptor::new(
                tokio_rustls::rustls::server::Acceptor::default(),
                tcp,
            );
            let Ok(start) = accepting.await else {
                return;
            };
            let grease = start
                .client_hello()
                .cipher_suites()
                .iter()
                .any(|c| is_grease(u16::from(*c)));
            shared.browser_tls.lock().unwrap().insert(peer, grease);
            let Ok(tls) = start.into_stream(config).await else {
                return;
            };
            let svc = app.layer(axum::Extension(ConnectInfo(peer)));
            let svc = hyper_util::service::TowerToHyperService::new(svc);
            let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(tls), svc)
                .await;
        });
    }
}

/// A GREASE value (RFC 8701: 0x0A0A, 0x1A1A … 0xFAFA), which Chrome's TLS puts among its
/// cipher suites and extensions so that servers keep tolerating unknown ones.
fn is_grease(v: u16) -> bool {
    v & 0x0f0f == 0x0a0a && v >> 8 == v & 0xff
}

/// Serves the generated file `size`/`name` for a request whose log entry is `entry`.
pub(crate) async fn serve(
    shared: Arc<Shared>,
    mut entry: LogEntry,
    method: &Method,
    size_param: &str,
    name: Option<&str>,
    query: &str,
) -> Response {
    let n = shared.requests.fetch_add(1, Ordering::SeqCst) + 1;
    let Some(size) = content::parse_size(size_param) else {
        shared.record(entry, 404, "error");
        return (StatusCode::NOT_FOUND, "bad size\n").into_response();
    };

    // `seed` selects the content; the file params shape the headers; every other query key is
    // a fault override.
    let q = match split_query(query) {
        Ok(v) => v,
        Err(msg) => {
            shared.record(entry, 400, "error");
            return (StatusCode::BAD_REQUEST, msg).into_response();
        }
    };
    let seed = q.seed;
    let used_up = q.uses.is_some_and(|uses| {
        let key = format!("{}?{query}", entry.path);
        let mut served = shared.link_uses.lock().unwrap();
        let n = served.entry(key).or_insert(0);
        *n += 1;
        *n > uses
    });
    if used_up || q.expires.is_some_and(|t| unix_ms() > t) {
        shared.record(entry, 403, "expired");
        return (StatusCode::FORBIDDEN, "link expired\n").into_response();
    }
    let f = shared.faults().overridden_by(&q.faults);

    // The bot check stands in front of the site, as Cloudflare's does: before its own checks.
    let a_program = entry.sec_fetch_mode.is_none() || entry.browser_tls == Some(false);
    if let Some(status) = f
        .refuse_programs
        .and_then(|c| StatusCode::from_u16(c).ok())
        .filter(|s| a_program && (s.is_client_error() || s.is_server_error()))
    {
        shared.record(entry, status.as_u16(), "fault:refuse_programs");
        return (status, [(header::RETRY_AFTER, "0")]).into_response();
    }
    if f.browser_only == Some(true) && a_program {
        shared.record(entry, 403, "fault:browser_only");
        return (
            StatusCode::FORBIDDEN,
            [
                ("cf-mitigated", "challenge"),
                ("content-type", "text/html; charset=UTF-8"),
            ],
            "<!DOCTYPE html><title>Just a moment...</title>\n",
        )
            .into_response();
    }
    if let Some(want) = &f.require_cookie
        && !has_cookie(entry.cookie.as_deref(), want)
    {
        shared.record(entry, 403, "fault:require_cookie");
        return (StatusCode::FORBIDDEN, format!("cookie {want} required\n")).into_response();
    }
    if let Some(want) = &f.require_referer
        && !entry
            .referer
            .as_deref()
            .is_some_and(|r| r.starts_with(want.as_str()))
    {
        shared.record(entry, 403, "fault:require_referer");
        return (StatusCode::FORBIDDEN, format!("referer {want}… required\n")).into_response();
    }
    if let Some(want) = &f.refuse_agent
        && entry
            .user_agent
            .as_deref()
            .is_some_and(|ua| ua.contains(want.as_str()))
    {
        shared.record(entry, 403, "fault:refuse_agent");
        return (StatusCode::FORBIDDEN, format!("agent {want} refused\n")).into_response();
    }
    if let Some(status) = f
        .refuse
        .and_then(|c| StatusCode::from_u16(c).ok())
        .filter(|s| s.is_client_error() || s.is_server_error())
    {
        shared.record(entry, status.as_u16(), "fault:refuse");
        return status.into_response();
    }
    if let Some(ms) = f.latency_ms {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    if let Some(ms) = f.range_latency_ms.filter(|_| entry.range.is_some()) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
    if every(n, f.fail_every) {
        shared.record(entry, 503, "fault:fail_every");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if f.fail_first.is_some_and(|k| n <= k) {
        shared.record(entry, 503, "fault:fail_first");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let page_now = f.html_after.is_some_and(|after| {
        let key = format!("html_after:{}?{query}", entry.path);
        let mut served = shared.link_uses.lock().unwrap();
        let n = served.entry(key).or_insert(0);
        *n += 1;
        *n > after
    });
    if f.html == Some(true) || page_now {
        shared.record(entry, 200, "fault:html");
        return web_page(&f, name, size);
    }
    let slot = {
        let mut active = shared.active.lock().unwrap();
        let slots = active.entry(entry.peer.ip()).or_default();
        if f.max_conns.is_some_and(|max| slots.len() >= max as usize) {
            entry.held = slots.iter().map(|u| u.held()).collect();
            drop(active);
            shared.record(entry, 503, "fault:max_conns");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        ActiveSlot::take_locked(&shared, slots, entry.peer.ip())
    };
    if f.once == Some(true) {
        let key = format!("{}?{query}", entry.path);
        if !shared.used_once.lock().unwrap().insert(key) {
            drop(slot);
            shared.record(entry, 410, "fault:once");
            return (StatusCode::GONE, "this link was already used\n").into_response();
        }
    }

    let generation = f.etag_change_every.filter(|k| *k > 0).map_or(0, |k| n / k);
    let etag = match f.etag.as_deref() {
        Some("weak") => Some(format!("W/\"{seed:x}-{size:x}-{generation}\"")),
        Some("per-request") => Some(format!("\"{seed:x}-{size:x}-{generation}-r{n}\"")),
        Some("none") => None,
        _ => Some(format!("\"{seed:x}-{size:x}-{generation}\"")),
    };
    let last_modified =
        httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(1_790_000_000 + generation * 60));

    // Decide the byte range to send.
    let if_range_ok = entry
        .if_range
        .as_deref()
        .is_none_or(|v| Some(v) == etag.as_deref() || v == last_modified);
    let mut range = None;
    // A syntactically invalid Range is ignored (RFC 9110 §14.2), as is a multi-range request.
    if f.no_range != Some(true)
        && !every(n, f.no_range_every)
        && if_range_ok
        && let Some(r) = entry.range.as_deref()
        && let Ok(parsed) = http_range_header::parse_range_header(r)
    {
        match parsed.validate(size) {
            Ok(v) if v.len() == 1 => range = Some((*v[0].start(), *v[0].end() + 1)),
            Ok(_) => {}
            Err(_) => {
                drop(slot);
                shared.record(entry, 416, "error");
                let mut resp = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
                resp.headers_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{size}")).unwrap(),
                );
                return resp;
            }
        }
    }
    let (start, end, status) = match range {
        Some((s, e)) => (s, e, StatusCode::PARTIAL_CONTENT),
        None => (0, size, StatusCode::OK),
    };
    entry.body_start = start;
    entry.body_len = end - start;

    let file_name = q.cdstar.as_deref().or(q.cd.as_deref()).or(name);
    let content_type = match (q.content_type.as_deref(), q.disposition) {
        (Some("none"), _) => None,
        (Some(t), _) => Some(t),
        (None, Disposition::Attachment) => Some("application/octet-stream"),
        (None, _) => Some(mime_type(file_name.unwrap_or(""))),
    };
    let mut builder = Response::builder().status(status);
    if let Some(t) = content_type {
        builder = builder.header(header::CONTENT_TYPE, t);
    }
    let mut builder = builder
        .header(header::CONTENT_LENGTH, end - start)
        .header(header::LAST_MODIFIED, &last_modified);
    if f.no_range != Some(true) {
        builder = builder.header(header::ACCEPT_RANGES, "bytes");
    }
    if let Some(etag) = &etag {
        builder = builder.header(header::ETAG, etag);
    }
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{}/{size}", end - 1),
        );
    }
    if let Some(cd) = content_disposition(&q, name) {
        builder = builder.header(header::CONTENT_DISPOSITION, cd);
    }

    if method == Method::HEAD {
        drop(slot);
        shared.record(entry, status.as_u16(), "head");
        return builder.body(Body::empty()).unwrap();
    }

    let rate = if every(n, f.slow_every) {
        f.slow_rate.or(f.rate)
    } else if entry.range.is_some() {
        f.range_rate.or(f.rate)
    } else {
        f.rate
    };
    let drop_at = f
        .drop_after
        .filter(|_| f.drop_every.is_none() || every(n, f.drop_every));
    entry.rate = rate;
    entry.drop_at = drop_at;
    entry.status = status.as_u16();
    slot.begins(start);

    let transfer = Transfer {
        shared: shared.clone(),
        entry: Some(entry),
        seed,
        magic: q.magic,
        pos: start,
        end,
        rate,
        burst: f.burst,
        drop_at,
        // A URL's own stall stays; the server's is lifted with `set_faults`.
        stall_at: f.stall_at.filter(|at| (start..end).contains(at)),
        stall_lifts: q.faults.stall_at.is_none(),
        started: Instant::now(),
        sent: 0,
        dropped: false,
        slot: Some(slot),
    };
    builder
        .body(Body::from_stream(transfer.into_stream()))
        .unwrap()
}

/// `html=1`'s page: a sign-in wall, or (with `html_type` or `html_cd`) a file host's waiting page
/// as it comes, its blank lines first (#701).
fn web_page(f: &Faults, name: Option<&str>, size: u64) -> Response {
    if f.html_type.is_none() && f.html_cd != Some(true) {
        return (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<!doctype html><title>Sign in</title><form>Please sign in</form>\n",
        )
            .into_response();
    }
    let mut builder = Response::builder().status(StatusCode::OK);
    match f.html_type.as_deref() {
        Some("none") => {}
        Some(t) => builder = builder.header(header::CONTENT_TYPE, t),
        None => builder = builder.header(header::CONTENT_TYPE, "text/html; charset=utf-8"),
    }
    if f.html_cd == Some(true) {
        let name = name.unwrap_or("file.zip").replace(['"', '\\'], "_");
        builder = builder.header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        );
    }
    let mut page = String::from(
        "\r\n\r\n<!DOCTYPE html>\n<html><head><title>Your file is almost ready</title></head>\
         <body><p>Please wait 30 seconds, then prove you are not a robot.</p>\
         <form method=post><input type=submit value=\"Create download link\"></form>\
         </body></html>\n",
    );
    // As long as the path's size says, up to 64 KiB (the report's page: 37 KB).
    let want = size.min(64 << 10) as usize;
    if want > page.len() + 8 {
        page.push_str("<!--");
        page.push_str(&" ".repeat(want - page.len() - 8));
        page.push_str("-->\n");
    }
    builder.body(Body::from(page)).unwrap()
}

/// RFC 8187 `attr-char`: what `filename*=` leaves unencoded.
const ATTR_CHAR: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'!')
    .remove(b'#')
    .remove(b'$')
    .remove(b'&')
    .remove(b'+')
    .remove(b'-')
    .remove(b'.')
    .remove(b'^')
    .remove(b'_')
    .remove(b'`')
    .remove(b'|')
    .remove(b'~');

/// The `Content-Disposition` of a generated file; `path_name` is the name in its path.
fn content_disposition(q: &FileQuery, path_name: Option<&str>) -> Option<String> {
    let kind = match q.disposition {
        Disposition::Attachment => "attachment",
        Disposition::Inline => "inline",
        Disposition::Bare => return None,
    };
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let params = if let Some(star) = &q.cdstar {
        format!(
            "; filename=\"fallback-name.zip\"; filename*=UTF-8''{}",
            utf8_percent_encode(star, ATTR_CHAR)
        )
    } else if let Some(name) =
        q.cd.as_deref()
            // A name with a control character in the path can't go in a header: none then.
            .or(path_name.filter(|n| !n.chars().any(char::is_control)))
    {
        format!("; filename=\"{}\"", quote(name))
    } else if q.disposition == Disposition::Inline {
        String::new()
    } else {
        return None;
    };
    Some(format!("{kind}{params}"))
}

/// The media type of a file name, by its extension.
pub(crate) fn mime_type(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
    match ext.to_ascii_lowercase().as_str() {
        "pdf" => "application/pdf",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "ts" => "video/mp2t",
        "m3u8" => "application/vnd.apple.mpegurl",
        "mpd" => "application/dash+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "txt" => "text/plain; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "json" => "application/json",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Splits a generated file's query into its own params and the fault overrides.
fn split_query(q: &str) -> Result<FileQuery, String> {
    let pairs: Vec<(String, String)> = serde_urlencoded::from_str(q).map_err(|e| e.to_string())?;
    let mut out = FileQuery {
        seed: 0,
        expires: None,
        uses: None,
        cd: None,
        cdstar: None,
        disposition: Disposition::Attachment,
        content_type: None,
        magic: None,
        faults: Faults::default(),
    };
    let name = |k: &str, v: String| {
        if v.is_empty() || v.chars().any(char::is_control) {
            Err(format!("invalid {k} `{}`\n", v.escape_debug()))
        } else {
            Ok(Some(v))
        }
    };
    let mut rest = Vec::new();
    for (k, v) in pairs {
        match k.as_str() {
            "seed" => out.seed = v.parse().map_err(|_| format!("invalid seed `{v}`\n"))?,
            "expires" => {
                out.expires = Some(v.parse().map_err(|_| format!("invalid expires `{v}`\n"))?)
            }
            "uses" => out.uses = Some(v.parse().map_err(|_| format!("invalid uses `{v}`\n"))?),
            // A page's own keys for a server's program (`script`): they change nothing here.
            "link" | "id" | "file" | "filename" | "token" => {}
            "type" => out.content_type = name(&k, v)?,
            "magic" => {
                out.magic =
                    Some(content::media_head(&v).ok_or_else(|| format!("invalid magic `{v}`\n"))?)
            }
            "cd" => out.cd = name(&k, v)?,
            "cdstar" => out.cdstar = name(&k, v)?,
            "inline" => {
                out.disposition = match (v.as_str(), parse_flag(&v)) {
                    ("bare" | "none", _) => Disposition::Bare,
                    (_, Some(true)) => Disposition::Inline,
                    (_, Some(false)) => Disposition::Attachment,
                    _ => return Err(format!("invalid inline `{v}` (1, 0 or bare)\n")),
                }
            }
            _ => rest.push((k, v)),
        }
    }
    let rest = serde_urlencoded::to_string(rest).map_err(|e| e.to_string())?;
    out.faults = Faults::parse(&rest).map_err(|e| format!("{e}\n"))?;
    Ok(out)
}

/// Streams one response body; logs the request when dropped. The outcome is decided there,
/// because hyper stops polling once Content-Length bytes are out and never sees end-of-stream.
struct Transfer {
    shared: Arc<Shared>,
    entry: Option<LogEntry>,
    seed: u64,
    /// The file's first bytes, over the generated ones (`magic`).
    magic: Option<&'static [u8]>,
    pos: u64,
    end: u64,
    rate: Option<u64>,
    burst: Option<u64>,
    drop_at: Option<u64>,
    /// The file offset this response stops at (`stall_at`), and whether `set_faults` can lift it.
    stall_at: Option<u64>,
    stall_lifts: bool,
    started: Instant,
    sent: u64,
    dropped: bool,
    /// Given back before the request is logged, so `settled` means every slot is free.
    slot: Option<ActiveSlot>,
}

impl Transfer {
    fn into_stream(self) -> impl futures::Stream<Item = io::Result<Bytes>> + Send + 'static {
        futures::stream::unfold(self, |mut t| async move {
            if t.pos >= t.end {
                return None;
            }
            if t.drop_at.is_some_and(|d| t.sent >= d) {
                // hyper discards unflushed output when a body errors; returning Pending once
                // lets it flush, so the client really receives the bytes sent before the drop.
                tokio::task::yield_now().await;
                t.dropped = true;
                return Some((Err(io::Error::other("fault: drop_after")), t));
            }
            if let Some(at) = t.stall_at.filter(|at| t.pos >= *at) {
                if t.stall_lifts {
                    t.shared.unstalled(at).await;
                } else {
                    std::future::pending::<()>().await;
                }
                t.stall_at = None;
            }
            let mut n = CHUNK.min(t.end - t.pos);
            if let Some(at) = t.stall_at {
                n = n.min(at - t.pos);
            }
            if let Some(r) = t.rate.filter(|r| *r > 0) {
                n = n.min((r / 20).max(1024));
                if let Some(b) = t.burst {
                    // A link doesn't bank what the client didn't take: catch up with `b` at most.
                    let paced = t.started + Duration::from_secs_f64(t.sent as f64 / r as f64);
                    let ahead = Duration::from_secs_f64(b as f64 / r as f64);
                    let behind = Instant::now().saturating_duration_since(paced);
                    if behind > ahead {
                        t.started += behind - ahead;
                    }
                }
                let due = Duration::from_secs_f64((t.sent + n) as f64 / r as f64);
                tokio::time::sleep_until((t.started + due).into()).await;
            }
            if let Some(d) = t.drop_at {
                n = n.min(d - t.sent);
            }
            let mut buf = vec![0u8; n as usize];
            content::fill_with_head(t.seed, t.pos, &mut buf, t.magic.unwrap_or_default());
            t.pos += n;
            t.sent += n;
            if let Some(slot) = &t.slot {
                slot.used.sent_to.store(t.pos, Ordering::SeqCst);
            }
            Some((Ok(Bytes::from(buf)), t))
        })
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        self.slot.take();
        if let Some(mut entry) = self.entry.take() {
            entry.bytes_sent = self.sent;
            let outcome = if self.dropped {
                "dropped"
            } else if self.sent == entry.body_len {
                "complete"
            } else {
                "aborted"
            };
            let status = entry.status;
            self.shared.record(entry, status, outcome);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_bodies() {
        let url = form_fields(
            Some("application/x-www-form-urlencoded"),
            b"token=abc&note=hello+world",
        );
        assert_eq!(url[0], ("token".into(), "abc".into()));
        assert_eq!(url[1], ("note".into(), "hello world".into()));
        let body = "--XyZ\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhi\r\nthere\r\n\
                    --XyZ\r\ncontent-disposition: form-data; name=\"token\"\r\n\r\nabc\r\n--XyZ--\r\n";
        let mp = form_fields(Some("multipart/form-data; boundary=XyZ"), body.as_bytes());
        assert_eq!(
            mp,
            vec![
                ("note".into(), "hi\r\nthere".into()),
                ("token".into(), "abc".into())
            ]
        );
        let plain = form_fields(Some("text/plain"), b"token=abc\r\nnote=x\r\n");
        assert!(plain.contains(&("token".into(), "abc".into())));
        assert!(form_fields(Some("multipart/form-data"), body.as_bytes()).is_empty());
    }

    #[test]
    fn file_query_and_disposition() {
        let q = split_query("seed=3&cd=report%20final.pdf&rate=1K").unwrap();
        assert_eq!(q.seed, 3);
        assert_eq!(q.faults.rate, Some(1024));
        assert_eq!(
            content_disposition(&q, Some("x.bin")).unwrap(),
            "attachment; filename=\"report final.pdf\""
        );
        let q = split_query("cdstar=r%C3%A9sum%C3%A9%20(1).pdf&inline=1").unwrap();
        assert_eq!(
            content_disposition(&q, None).unwrap(),
            "inline; filename=\"fallback-name.zip\"; filename*=UTF-8''r%C3%A9sum%C3%A9%20%281%29.pdf"
        );
        let q = split_query("inline=1").unwrap();
        assert_eq!(content_disposition(&q, None).unwrap(), "inline");
        assert!(content_disposition(&split_query("").unwrap(), None).is_none());
        assert!(content_disposition(&split_query("inline=bare").unwrap(), Some("a.pdf")).is_none());
        assert!(split_query("inline=maybe").is_err());
        assert!(split_query("cd=a%0Ab").is_err());
        assert_eq!(mime_type("A.PDF"), "application/pdf");
        assert_eq!(mime_type("x.mp3"), "audio/mpeg");
        assert_eq!(mime_type("noext"), "application/octet-stream");
    }
}
