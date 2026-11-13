//! Mister Download Manager's command channel (shared by the app, its command line and the browser host).
//! Single-instance command channel. The running app listens on a localhost port and writes
//! `{port, token}` to `%APPDATA%\misterdownloadmanager\ipc.json`; a second launch (IDM-style command line)
//! or the browser's native messaging host sends one JSON line per command, carrying the token.
//! Only connections from 127.0.0.1 with the right token are accepted.

pub mod api;
pub mod browsers;
pub mod capture;
pub mod extension;

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A request to the running app.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct Command {
    /// The URL to download (`/d`).
    pub url: Option<String>,
    /// Save folder (`/p`) and file name (`/f`).
    pub dir: Option<String>,
    pub name: Option<String>,
    /// `/a`: add to the main queue, don't start.
    pub later: bool,
    /// `/n`: silent — no Download File Info dialog.
    pub silent: bool,
    /// `/s`: start the main queue.
    pub start_queue: bool,
    /// From the browser: request context to reuse.
    pub referer: Option<String>,
    /// From the browser (#900): the `Origin` its page's player sent with a video's stream or
    /// file (`video_page`, `video_lookup`), sent again with it, its playlists and each of its
    /// pieces, as a site's video server may serve its own pages' players only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    pub cookies: Option<String>,
    /// From the browser: the same cookies, each with the domain, path and flags the browser
    /// keeps for it, for yt-dlp's cookie file. yt-dlp may ask another host than the page's (a
    /// YouTube Music page is looked up on www.youtube.com), and a cookie of `.youtube.com` goes
    /// there too. An older extension sends `cookies` only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cookie_jar: Vec<BrowserCookie>,
    pub user_agent: Option<String>,
    /// From the browser: where its redirects from `url` ended (`url` stays the address asked
    /// for, so an expired signed link can be renewed from it).
    pub final_url: Option<String>,
    /// From the browser: the size and type its own download had (no need to look them up).
    pub size: Option<u64>,
    pub mime: Option<String>,
    /// From the browser: a form's download, re-sent as a POST with this body and its
    /// Content-Type (`post_type`).
    pub post: Option<String>,
    pub post_type: Option<String>,
    /// From a private (incognito) window: the download is never written to the saved list.
    pub private: bool,
    /// From the browser (#42): the proxy it uses for this address ("http://host:port",
    /// "https://…", "socks5h://…", "socks4://…"), set by a VPN or proxy extension or its
    /// settings; or its proxy auto-config script's text. The app downloads through the same
    /// one (Options → Proxy / Socks → "Use proxy/socks from a browser's request").
    pub proxy: Option<String>,
    pub proxy_pac: Option<String>,
    /// That proxy asked the browser for a login, which the extension that set it answers with
    /// its own: no other program can have it.
    pub proxy_login: bool,
    /// Automation API (off by default, beyond IDM): `"list"` asks for every download as JSON.
    pub query: Option<String>,
    /// Automation API: turn it on or off.
    pub api: Option<bool>,
    /// Automation API: `"pause"`, `"start"`, `"remove"` (off the list, its file kept) or
    /// `"delete"` (with its file) the downloads `ids`; answered with what was done and skipped.
    /// `"start_queue"` / `"stop_queue"` start or stop the queue named `queue`.
    pub action: Option<String>,
    pub ids: Vec<u64>,
    /// A category for `url` (its `key` in the list's `categories`); none: by the file's type.
    pub category: Option<String>,
    /// A queue by name: where `url` waits (with `later`), or the one `action` starts or stops.
    pub queue: Option<String>,
    /// Sent by the remote control (#141, crates/remote) on behalf of a paired device, never read
    /// from a request line: no window opens for it, and it may only add, list and act.
    #[serde(skip)]
    pub remote: bool,
    /// The remote control: an uploaded .torrent file's bytes (never read from a request line).
    #[serde(skip)]
    pub torrent_file: Option<Vec<u8>>,
    /// The remote control (#157, `misterdownloadmanager-cli pair`): show a new pairing code;
    /// answered with it, the page's addresses and a QR code to scan ([`api::PairAnswer`]).
    pub pair: bool,
    /// `/exit` (beyond IDM): the running app saves and quits (the installer, before an upgrade
    /// or an uninstall). Starts nothing when no app runs.
    pub exit: bool,
    /// `/startup` (beyond IDM): started with the session (Options → General → Launch on
    /// startup, or the installer's "Start when Windows starts", #226): MDM starts in the
    /// notification area, its window hidden. Changes nothing in a running app.
    pub startup: bool,
    /// `/show <window>` (beyond IDM): the running app opens one of its windows by name, for
    /// screenshots and scripted checks (tools/lab/docs-shots.ps1): `addurl`, `batch`, `find`,
    /// `scheduler`, `about`, `language`, `shortcuts`, `speedlimit`, `properties`, `delete`,
    /// `progress`, `existing-files`, `video-page`, `video-helpers`, `video-subs`,
    /// `torrent-settings`, `add-trackers`, `menu` (the first download's right-click menu),
    /// `options[:general|types|saveto|downloads|connection|proxy|logins|sounds|regional|panels]`.
    pub show: Option<String>,
    /// From the browser's download button over a video: open the Download video window for this
    /// page (its address; an embedded player's own).
    pub video_page: Option<String>,
    /// The title of the page the button was clicked on: a video yt-dlp knows only by its address
    /// is named by it.
    pub video_title: Option<String>,
    /// From the browser's quality menu over a video (the button's click): look this video up
    /// with yt-dlp, off the window, and answer at once with a token to ask about it
    /// ([`api::VideoLookupAnswer`], `pending`). The browser's request context (`referer`,
    /// `cookies`, `cookie_jar`, `user_agent`, `private`) goes with it and stays the look-up's.
    pub video_lookup: Option<VideoLookup>,
    /// The quality menu: how is the look-up of this token going (`pending`, `ready` with the
    /// choices, `failed`, `list`, `gone`)?
    pub video_poll: Option<String>,
    /// The quality menu: download the choice made there, with the look-up's own selector and
    /// context (`private` must be the look-up's); answered `started` or `queued`, or
    /// `use-window` when it can't be (the Download video window then does it).
    pub video_pick: Option<VideoPick>,
    /// From that button's menu: never show it on this site (a host name) again.
    pub panel_hide_site: Option<String>,
    /// From that button's "Turn on" (after "Never on <site>"): show it on this site again.
    pub panel_show_site: Option<String>,
    /// With `video_page`, from that button over a video that plays a plain file (no stream, no
    /// page to read): `video_page` is that file, downloaded as any file at once, with no window.
    pub video_direct: bool,
    /// That button's "Show", once a download was added: the main window comes up with the newest
    /// download of this address (the video's page as it was looked up, or its file) selected.
    pub show_download: Option<String>,
    /// From the browser's right-click menu: the links of a page or a selection, offered in the
    /// list window to pick from (the browser's `referer` and `cookies` go with them).
    pub links: Vec<String>,
    /// From the browser's toolbar button: leave this site's downloads (a host name) to the
    /// browser from now on (Options → File types → excluded sites).
    pub exclude_site: Option<String>,
}

/// One cookie as the browser keeps it (`cookies.getAll`'s fields, under the extension's names
/// too: `hostOnly`, `expirationDate`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct BrowserCookie {
    pub name: String,
    pub value: String,
    /// `.youtube.com` (sent to its subdomains too) or a host name.
    pub domain: String,
    /// Sent to `domain` itself only, never its subdomains.
    #[serde(alias = "hostOnly")]
    pub host_only: bool,
    pub path: String,
    /// Sent over HTTPS only.
    pub secure: bool,
    /// When it expires, in seconds since 1970; none: when the browser closes.
    #[serde(alias = "expirationDate")]
    pub expires: Option<f64>,
}

/// The video the browser's quality menu asks about ([`Command::video_lookup`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct VideoLookup {
    /// What yt-dlp looks up: the video's own file or its stream's manifest when the page plays
    /// one, else the page.
    pub address: String,
    /// The title of the page the button was clicked on: a video yt-dlp knows only by its address
    /// is named by it (as [`Command::video_title`]).
    pub title: Option<String>,
    /// The browser's cookie store the page is in (Firefox's containers): a choice is only taken
    /// from the same one.
    pub store_id: Option<String>,
}

/// A choice made in the browser's quality menu ([`Command::video_pick`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct VideoPick {
    /// The look-up's token.
    pub token: String,
    /// The choice's place in the look-up's `choices`.
    pub choice: usize,
    /// The cookie store the choice was made in: the look-up's, or it isn't taken.
    pub store_id: Option<String>,
}

impl Command {
    /// Asks for an answer (a JSON line) rather than "ok".
    pub fn wants_reply(&self) -> bool {
        self.query.is_some()
            || self.api.is_some()
            || self.action.is_some()
            || self.pair
            || self.video_lookup.is_some()
            || self.video_poll.is_some()
            || self.video_pick.is_some()
    }
}

/// A command as the app receives it: requests that want an answer carry where to send it.
pub struct Incoming {
    pub cmd: Command,
    pub reply: Option<std::sync::mpsc::SyncSender<String>>,
}

/// The app's command channel from inside its own process (the remote control, #141): where
/// commands go, and what wakes the app to read them.
#[derive(Clone)]
pub struct Sink {
    tx: Sender<Incoming>,
    wake: std::sync::Arc<dyn Fn() + Send + Sync>,
}

impl Sink {
    pub fn new(tx: Sender<Incoming>, wake: std::sync::Arc<dyn Fn() + Send + Sync>) -> Sink {
        Sink { tx, wake }
    }

    /// Hand `cmd` to the app and wait up to `wait` for its one-line answer (blocking). None:
    /// the app is quitting or didn't answer in time.
    pub fn ask(&self, cmd: Command, wait: Duration) -> Option<String> {
        let (rtx, rrx) = std::sync::mpsc::sync_channel(1);
        self.tx
            .send(Incoming {
                cmd,
                reply: Some(rtx),
            })
            .ok()?;
        (self.wake)();
        rrx.recv_timeout(wait).ok()
    }

    /// Wake the app (to show something that changed: a device paired).
    pub fn wake(&self) {
        (self.wake)();
    }
}

#[derive(Serialize, Deserialize)]
struct Endpoint {
    port: u16,
    token: String,
    /// The app's process id (0: written by an older app), so a client that may bring windows to
    /// the front can let the app do it (the native host on Windows: `AllowSetForegroundWindow`).
    #[serde(default)]
    pid: u32,
}

/// The running app's process id, as its ipc.json says (None: not running, or an older app).
pub fn app_pid() -> Option<u32> {
    let ep: Endpoint = serde_json::from_slice(&std::fs::read(endpoint_path()?).ok()?).ok()?;
    (ep.pid != 0).then_some(ep.pid)
}

/// One request line: the token from ipc.json and the command's fields.
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "Request")]
struct Envelope {
    /// The `token` from ipc.json (it changes at every start of the app).
    token: String,
    #[serde(flatten)]
    cmd: Command,
}

/// The data folder a headless run was given (`--data`, #157), before anything reads it.
static DATA_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Use `dir` as the data folder from now on (over `MDM_DATA_DIR`): `--headless --data <dir>`.
pub fn set_data_dir(dir: PathBuf) {
    let _ = DATA_DIR.set(full(dir));
}

/// `dir` as a full path (#834): the helpers run in a folder of their own, and are given paths in
/// the data folder (their programs, ffmpeg's place, the files MDM writes for them).
fn full(dir: PathBuf) -> PathBuf {
    std::path::absolute(&dir).unwrap_or(dir)
}

/// The app's data folder, `%APPDATA%\misterdownloadmanager` (list, settings, channel, crash
/// log). The folder of the app's former codename, `idmnext`, is moved here once.
///
/// `MDM_DATA_DIR` overrides it (a separate instance with its own list and channel: tests,
/// screenshots). Portable mode: a writable `data` folder next to the program holds it all.
///
/// Worked out once per run and kept (#565): the window asks for it on every tick (torrents'
/// state, the list), and the portable check below reads and writes the disk, so a slow disk
/// held the window up. A folder given on the command line is still taken first.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = DATA_DIR.get() {
        return Some(dir.clone());
    }
    static RESOLVED: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    once(&RESOLVED, || {
        if let Some(dir) = std::env::var_os("MDM_DATA_DIR") {
            return Some(full(PathBuf::from(dir)));
        }
        if let Some(dir) = portable_dir() {
            return Some(dir);
        }
        let base = dirs::data_dir()?;
        let dir = base.join("misterdownloadmanager");
        let legacy = base.join("idmnext");
        if !dir.exists() && legacy.is_dir() {
            let _ = std::fs::rename(&legacy, &dir);
        }
        Some(dir)
    })
}

/// `cell`'s value, worked out by `make` the first time only.
fn once<T: Clone>(cell: &std::sync::OnceLock<T>, make: impl FnOnce() -> T) -> T {
    cell.get_or_init(make).clone()
}

/// Create the data folder `dir`, readable by its owner only on Linux and macOS (it holds the
/// command channel's token and the saved logins; a home folder is often open to other users).
pub fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Write `bytes` to `path` atomically, readable by its owner only (Linux and macOS: mode 0600;
/// Windows: the profile's own access rules).
pub fn private_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    craft_atomic::WriteOptions::default()
        .with_new_file_permissions(craft_atomic::NewFilePermissions::Private)
        .atomic_write(path, bytes)
}

/// Portable mode: the `data` folder next to the program, when it exists and can be written
/// (else the profile folder is used). Nothing is written to the registry in this mode.
/// Asked once per run and kept, like [`data_dir`] (#565).
pub fn portable_dir() -> Option<PathBuf> {
    static PORTABLE: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    once(&PORTABLE, portable_dir_now)
}

fn portable_dir_now() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.join("data");
    if !dir.is_dir() {
        return None;
    }
    let probe = dir.join(".write-test");
    let writable = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    writable.then_some(dir)
}

/// Become the one running instance: an OS lock on `instance.lock` in the data folder, held for
/// as long as the returned file lives. `None` when another instance holds it (or is starting).
pub fn instance_lock() -> Option<std::fs::File> {
    let dir = data_dir()?;
    private_dir(&dir).ok()?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("instance.lock"))
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
}

/// Hand `cmd` to the instance that holds the lock, waiting up to `wait` for it to start
/// listening (it may have just been launched too).
pub fn send_waiting(cmd: &Command, wait: Duration) -> Result<(), String> {
    let start = std::time::Instant::now();
    loop {
        match send(cmd) {
            Ok(()) => return Ok(()),
            Err(e) if start.elapsed() >= wait => return Err(e.to_string()),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

pub fn endpoint_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("ipc.json"))
}

/// Ask the running app something (automation API) and return its one-line answer.
pub fn request(cmd: &Command) -> Result<String, String> {
    exchange(cmd, ANSWER_WAIT).map_err(|e| e.to_string())
}

/// Ask the running app something and return its one-line answer, whatever it is (an older app
/// answers `ok` to a question it doesn't know); or why it didn't reach an app: none listens
/// (`NotRunning`), or it took the line but gave no answer (`Refused`). Never starts one.
///
/// It waits a second longer than the app's channel gives the app (2 s), so an app
/// too busy to answer in time is heard saying `timeout` rather than taken for a closed line.
pub fn request_checked(cmd: &Command) -> Result<String, SendError> {
    exchange(cmd, REPLY_WAIT + Duration::from_secs(1))
}

/// How long a client reads for its answer (`send`, `request`).
const ANSWER_WAIT: Duration = Duration::from_secs(2);
/// How long the channel waits for the app's answer to a question before saying `timeout`.
const REPLY_WAIT: Duration = Duration::from_secs(2);

/// Why a command didn't reach the app.
#[derive(Debug)]
pub enum SendError {
    /// No app listens (no channel file, or nothing at its port): starting one may help.
    NotRunning(String),
    /// An app answered but didn't take it (`too-large`, `gone` while it quits, …), or stopped
    /// answering: starting another can't help.
    Refused(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::NotRunning(e) => write!(f, "not running: {e}"),
            SendError::Refused(e) => write!(f, "refused: {e}"),
        }
    }
}

/// Send `cmd` to a running Mister Download Manager.
pub fn send(cmd: &Command) -> Result<(), SendError> {
    let ack = exchange(cmd, ANSWER_WAIT)?;
    if ack.trim() == "ok" {
        Ok(())
    } else {
        Err(SendError::Refused(if ack.is_empty() {
            "no answer".into()
        } else {
            ack
        }))
    }
}

/// One request line out, one answer line back (read for up to `wait`).
fn exchange(cmd: &Command, wait: Duration) -> Result<String, SendError> {
    let not_running = |e: String| SendError::NotRunning(e);
    let refused = |e: std::io::Error| SendError::Refused(e.to_string());
    let path = endpoint_path().ok_or_else(|| not_running("no data dir".into()))?;
    let ep: Endpoint =
        serde_json::from_slice(&std::fs::read(&path).map_err(|e| not_running(e.to_string()))?)
            .map_err(|e| not_running(e.to_string()))?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, ep.port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500))
        .map_err(|e| not_running(e.to_string()))?;
    s.set_read_timeout(Some(wait)).ok();
    let line = serde_json::to_string(&Envelope {
        token: ep.token,
        cmd: cmd.clone(),
    })
    .map_err(|e| SendError::Refused(e.to_string()))?;
    s.write_all(format!("{line}\n").as_bytes())
        .map_err(refused)?;
    let mut ack = String::new();
    BufReader::new(s).read_line(&mut ack).map_err(refused)?;
    Ok(ack.trim_end().to_owned())
}

/// Start listening; commands arrive on `tx` and `wake` is called for each (repaint the UI).
pub fn serve(tx: Sender<Incoming>, wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let token = random_token();
    if let Some(path) = endpoint_path() {
        if let Some(dir) = path.parent() {
            private_dir(dir)?;
        }
        let json = serde_json::to_vec(&Endpoint {
            port,
            token: token.clone(),
            pid: std::process::id(),
        })
        .map_err(std::io::Error::other)?;
        private_write(&path, &json)?;
    }
    std::thread::Builder::new()
        .name("mdm-ipc".into())
        .spawn(move || accept_loop(listener, token, tx, wake))?;
    Ok(())
}

/// Connections handled at once; more are closed unanswered.
const MAX_IN_FLIGHT: usize = 8;

/// Each loopback connection on its own thread (a slow or hostile client can't hold up a real
/// command), at most `MAX_IN_FLIGHT` at a time.
fn accept_loop(
    listener: TcpListener,
    token: String,
    tx: Sender<Incoming>,
    wake: impl Fn() + Send + Sync + 'static,
) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (token, wake) = (Arc::new(token), Arc::new(wake));
    let in_flight = Arc::new(AtomicUsize::new(0));
    for conn in listener.incoming().flatten() {
        if !conn.peer_addr().is_ok_and(|a| a.ip().is_loopback()) {
            continue;
        }
        if in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT {
            in_flight.fetch_sub(1, Ordering::SeqCst);
            continue; // dropped: closes the connection
        }
        let (token, wake, tx, slot) = (token.clone(), wake.clone(), tx.clone(), in_flight.clone());
        let spawned = std::thread::Builder::new()
            .name("mdm-ipc-conn".into())
            .spawn(move || {
                handle(&conn, &token, &tx, &*wake);
                slot.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            in_flight.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn handle(conn: &TcpStream, token: &str, tx: &Sender<Incoming>, wake: &dyn Fn()) {
    let line = match read_request(conn) {
        Ok(line) => line,
        // Said so, so the browser's host doesn't take silence for an app that isn't there.
        Err(Some(TooLarge)) => {
            let _ = (&*conn).write_all(b"too-large\n");
            linger_close(conn);
            return;
        }
        Err(None) => return,
    };
    // A web page reaching the port (fetch / form post): no answer at all.
    if looks_like_http(&line) {
        return;
    }
    let reply = match serde_json::from_str::<Envelope>(&line) {
        Ok(env) if same_secret(&env.token, token) => {
            if env.cmd.wants_reply() {
                // The app answers on its next frame (woken now); a stuck app means no answer.
                // `timeout` therefore always means the app has the command and will act on it
                // (the browser's host counts on that: a quality menu's choice is then `sent`);
                // an app that is quitting never gets it, and says `gone`, as for a command.
                let (rtx, rrx) = std::sync::mpsc::sync_channel(1);
                if tx
                    .send(Incoming {
                        cmd: env.cmd,
                        reply: Some(rtx),
                    })
                    .is_err()
                {
                    let _ = (&*conn).write_all(b"gone\n");
                    return;
                }
                wake();
                let answer = rrx
                    .recv_timeout(REPLY_WAIT)
                    .unwrap_or_else(|_| "timeout".into());
                let _ = (&*conn).write_all(format!("{answer}\n").as_bytes());
                return;
            }
            // The app is quitting (its receiver gone): the command would be lost.
            if tx
                .send(Incoming {
                    cmd: env.cmd,
                    reply: None,
                })
                .is_err()
            {
                "gone\n"
            } else {
                wake();
                "ok\n"
            }
        }
        _ => "denied\n",
    };
    let _ = (&*conn).write_all(reply.as_bytes());
}

/// Close after an answer given before the request was read to its end: no more writing, then
/// what the client still sends is read and dropped (up to a limit, for a second at most).
/// Closing with it unread makes the system reset the connection (macOS at once), and a reset can
/// lose the answer before the client reads it.
fn linger_close(conn: &TcpStream) {
    use std::io::Read;
    let _ = conn.shutdown(std::net::Shutdown::Write);
    let _ = conn.set_read_timeout(Some(Duration::from_millis(250)));
    let start = std::time::Instant::now();
    let mut drained = 0;
    let mut chunk = vec![0u8; 64 << 10];
    while drained < 4 * MAX_REQUEST && start.elapsed() < Duration::from_secs(1) {
        match (&*conn).read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => drained += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
}

/// An HTTP request line (`POST / HTTP/1.1`) instead of our JSON.
fn looks_like_http(line: &str) -> bool {
    let line = line.trim_end_matches('\r');
    line.rsplit(' ')
        .next()
        .is_some_and(|v| v.starts_with("HTTP/"))
}

/// Token comparison in time independent of where the first difference is.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// The largest message the browser's native host takes from the extension (a download with
/// its cookies and a form's body); a request line carrying one, escaped, fits `MAX_REQUEST`.
pub const MAX_MESSAGE: usize = 1 << 20;
/// Largest request line accepted, and the time a client gets to send it.
const MAX_REQUEST: usize = 2 * MAX_MESSAGE + 4096;
const REQUEST_DEADLINE: Duration = Duration::from_secs(3);

/// A request line over `MAX_REQUEST`.
struct TooLarge;

/// One request line, read with a size cap checked as bytes arrive and an overall deadline, so a
/// client trickling bytes (or a web page probing the port) can't hold the listener. `Err(None)`:
/// nothing usable (closed, too slow, not UTF-8).
fn read_request(conn: &std::net::TcpStream) -> Result<String, Option<TooLarge>> {
    use std::io::Read;
    let start = std::time::Instant::now();
    conn.set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|_| None)?;
    let mut buf = Vec::new();
    // Large reads: a request up to the limit (2 MB) comes in well within the deadline even on a
    // busy machine (4 KB reads ran past it on a CI Mac).
    let mut chunk = vec![0u8; 64 << 10];
    loop {
        if buf.len() > MAX_REQUEST {
            return Err(Some(TooLarge));
        }
        if start.elapsed() > REQUEST_DEADLINE {
            return Err(None);
        }
        match (&*conn).read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(i) = buf.iter().position(|&b| b == b'\n') {
                    buf.truncate(i);
                    break;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return Err(None),
        }
    }
    if buf.len() > MAX_REQUEST {
        return Err(Some(TooLarge));
    }
    String::from_utf8(buf).map_err(|_| None)
}

/// 128 random bits from the OS, as hex (no extra crate: the std hasher's per-process keys).
/// The channel's token, and each look-up's of the browser's quality menu
/// (crates/app/src/videolookup.rs).
pub fn random_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    (0..4)
        .map(|i| {
            let mut h = RandomState::new().build_hasher();
            h.write_u64(i ^ std::process::id() as u64);
            format!("{:016x}", h.finish())
        })
        .collect()
}

/// The argument every startup entry passes (the Run key, the XDG autostart entry, the
/// LaunchAgent, the installer's): MDM starts in the notification area ([`Command::startup`]).
pub const STARTUP: &str = "/startup";

/// Parse IDM's command line: `/d URL [/p dir] [/f name] [/a] [/n] [/q] [/s]` (case-insensitive;
/// `-d` also accepted). Returns None when there is nothing to do.
pub fn parse_args(args: &[String]) -> Option<Command> {
    let mut cmd = Command::default();
    let mut it = args.iter();
    let mut any = false;
    while let Some(a) = it.next() {
        let flag = a.trim_start_matches(['/', '-']).to_ascii_lowercase();
        match flag.as_str() {
            "d" => cmd.url = it.next().cloned(),
            "p" => cmd.dir = it.next().cloned(),
            "f" => cmd.name = it.next().cloned(),
            "a" => cmd.later = true,
            "n" => cmd.silent = true,
            "s" => cmd.start_queue = true,
            "q" | "h" => {}
            "exit" => cmd.exit = true,
            "startup" => cmd.startup = true,
            "show" => cmd.show = it.next().map(|w| w.to_ascii_lowercase()),
            // A bare magnet link or .torrent file (what Windows passes for an association).
            _ if a
                .get(..7)
                .is_some_and(|s| s.eq_ignore_ascii_case("magnet:"))
                || a.to_ascii_lowercase().ends_with(".torrent") =>
            {
                cmd.url = Some(a.clone());
            }
            _ => continue,
        }
        any = true;
    }
    any.then_some(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #565: the data folder is worked out once; later calls (every tick of the window) don't
    /// look at the disk again.
    #[test]
    fn a_kept_value_is_worked_out_once() {
        let cell = std::sync::OnceLock::new();
        let calls = std::cell::Cell::new(0);
        let make = || {
            calls.set(calls.get() + 1);
            Some(PathBuf::from("data"))
        };
        assert_eq!(once(&cell, make), Some(PathBuf::from("data")));
        assert_eq!(once(&cell, make), Some(PathBuf::from("data")));
        assert_eq!(once(&cell, make), Some(PathBuf::from("data")));
        assert_eq!(calls.get(), 1);
        assert_eq!(data_dir(), data_dir());
    }

    #[test]
    fn bare_magnets_and_torrent_files() {
        let c = parse_args(&["magnet:?xt=urn:btih:abc".into()]).unwrap();
        assert_eq!(c.url.as_deref(), Some("magnet:?xt=urn:btih:abc"));
        let c = parse_args(&[r"C:\dl\x.TORRENT".into()]).unwrap();
        assert_eq!(c.url.as_deref(), Some(r"C:\dl\x.TORRENT"));
        assert!(parse_args(&["stray".into()]).is_none());
        let c = parse_args(&["/show".into(), "Options:Connection".into()]).unwrap();
        assert_eq!(c.show.as_deref(), Some("options:connection"));
    }

    #[test]
    fn idm_command_line() {
        let a = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let c = parse_args(&a("/d http://x/a.zip /p C:\\dl /f b.zip /a /n")).unwrap();
        assert_eq!(c.url.as_deref(), Some("http://x/a.zip"));
        assert_eq!(c.dir.as_deref(), Some("C:\\dl"));
        assert_eq!(c.name.as_deref(), Some("b.zip"));
        assert!(c.later && c.silent && !c.start_queue);
        assert!(parse_args(&a("/s")).unwrap().start_queue);
        assert!(parse_args(&a("/exit")).unwrap().exit);
        // The startup entries' argument (#226), as written and as a Linux user might.
        assert!(parse_args(&[STARTUP.into()]).unwrap().startup);
        assert!(parse_args(&a("--startup")).unwrap().startup);
        assert!(!parse_args(&a("/s")).unwrap().startup);
        assert!(parse_args(&[]).is_none());
    }

    /// Send `data` to a fresh local socket and return what `read_request` makes of it.
    fn request_from(data: Vec<u8>) -> Result<String, Option<TooLarge>> {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = l.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            let _ = s.write_all(&data);
            std::thread::sleep(Duration::from_millis(300));
        });
        let (conn, _) = l.accept().unwrap();
        let got = read_request(&conn);
        client.join().unwrap();
        got
    }

    #[test]
    fn request_lines_are_bounded() {
        assert_eq!(
            request_from(b"{\"a\":1}\nrest".to_vec()).ok().as_deref(),
            Some("{\"a\":1}")
        );
        assert!(
            matches!(
                request_from(vec![b'x'; MAX_REQUEST + 10]),
                Err(Some(TooLarge))
            ),
            "oversized"
        );
    }

    /// A running accept loop on a fresh port: (port, received commands).
    fn test_server(token: &str) -> (u16, std::sync::mpsc::Receiver<Incoming>) {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        let token = token.to_owned();
        std::thread::spawn(move || accept_loop(l, token, tx, || {}));
        (port, rx)
    }

    fn exchange(port: u16, data: &[u8]) -> String {
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(data).unwrap();
        let mut reply = String::new();
        let _ = BufReader::new(s).read_line(&mut reply);
        reply
    }

    #[test]
    fn a_slow_client_does_not_hold_up_a_real_command() {
        let (port, rx) = test_server("secret");
        // Connects and says nothing (it would hold a one-at-a-time server for 3 s).
        let _slow = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        let started = std::time::Instant::now();
        let line = r#"{"token":"secret","url":"http://x/a.zip"}"#.to_owned() + "\n";
        assert_eq!(exchange(port, line.as_bytes()), "ok\n");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            rx.recv().unwrap().cmd.url.as_deref(),
            Some("http://x/a.zip")
        );
    }

    #[test]
    fn too_large_and_quitting_are_said() {
        let (port, rx) = test_server("secret");
        let big = format!(
            "{{\"token\":\"secret\",\"cookies\":\"{}\"}}\n",
            "x".repeat(MAX_REQUEST + 10)
        );
        assert_eq!(exchange(port, big.as_bytes()), "too-large\n");
        // A command as large as the browser's host takes still goes through.
        let fits = format!(
            "{{\"token\":\"secret\",\"url\":\"http://x/a.zip\",\"cookies\":\"{}\"}}\n",
            "x".repeat(MAX_MESSAGE)
        );
        assert_eq!(exchange(port, fits.as_bytes()), "ok\n");
        drop(rx);
        let line = b"{\"token\":\"secret\",\"url\":\"http://x/a.zip\"}\n";
        assert_eq!(exchange(port, line), "gone\n", "the app is quitting");
    }

    #[test]
    fn web_pages_and_wrong_tokens_get_nothing() {
        let (port, rx) = test_server("secret");
        let post = b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n{\"token\":\"secret\"}";
        assert_eq!(exchange(port, post), "", "no answer to a web page");
        let wrong = b"{\"token\":\"guess\",\"url\":\"http://x/\"}\n";
        assert_eq!(exchange(port, wrong), "denied\n");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn queries_get_the_apps_answer() {
        let (port, rx) = test_server("secret");
        // The app's side: answer the query from its frame.
        std::thread::spawn(move || {
            let inc = rx.recv().unwrap();
            assert_eq!(inc.cmd.query.as_deref(), Some("list"));
            inc.reply
                .unwrap()
                .send("{\"downloads\":[]}".into())
                .unwrap();
        });
        let line = b"{\"token\":\"secret\",\"query\":\"list\"}\n";
        assert_eq!(exchange(port, line), "{\"downloads\":[]}\n");
    }

    /// The browser's quality menu asks questions: each of its three wants the app's answer, and
    /// each comes through a request line whole.
    #[test]
    fn the_quality_menus_requests_want_answers() {
        let lookup = Command {
            video_lookup: Some(VideoLookup {
                address: "https://cdn.test/v/master.m3u8".into(),
                title: Some("A talk".into()),
                store_id: Some("firefox-container-1".into()),
            }),
            referer: Some("https://videos.test/watch".into()),
            private: true,
            startup: true,
            ..Default::default()
        };
        let poll = Command {
            video_poll: Some("abc".into()),
            startup: true,
            ..Default::default()
        };
        let pick = Command {
            video_pick: Some(VideoPick {
                token: "abc".into(),
                choice: 2,
                store_id: None,
            }),
            startup: true,
            ..Default::default()
        };
        for cmd in [&lookup, &poll, &pick] {
            assert!(cmd.wants_reply(), "{cmd:?}");
        }
        assert!(!Command::default().wants_reply());
        assert!(
            !Command {
                video_page: Some("https://videos.test/watch".into()),
                ..Default::default()
            }
            .wants_reply(),
            "the window's click is a command, not a question"
        );
        // To the app and back, as the channel carries them.
        let back = |cmd: &Command| {
            let line = serde_json::to_string(&Envelope {
                token: "t".into(),
                cmd: cmd.clone(),
            })
            .unwrap();
            serde_json::from_str::<Envelope>(&line).unwrap().cmd
        };
        let got = back(&lookup);
        assert_eq!(got.video_lookup, lookup.video_lookup);
        assert_eq!(got.referer, lookup.referer);
        assert!(got.private && got.startup);
        assert_eq!(back(&poll).video_poll.as_deref(), Some("abc"));
        assert_eq!(back(&pick).video_pick, pick.video_pick);
        // As the browser's host writes them (snake_case), missing parts defaulted.
        let raw: Command = serde_json::from_str(
            r#"{"video_pick":{"token":"abc","choice":3,"store_id":"firefox-private"},"private":true}"#,
        )
        .unwrap();
        assert_eq!(
            raw.video_pick,
            Some(VideoPick {
                token: "abc".into(),
                choice: 3,
                store_id: Some("firefox-private".into()),
            })
        );
        let raw: Command =
            serde_json::from_str(r#"{"video_lookup":{"address":"https://x.test/w"}}"#).unwrap();
        let ask = raw.video_lookup.unwrap();
        assert_eq!((ask.title, ask.store_id), (None, None));
    }

    /// An app that doesn't answer a question in time says so (`timeout`), and the one asking
    /// hears it (it waits longer than the app is given).
    #[test]
    fn a_question_the_app_does_not_answer_is_timeout() {
        let (port, rx) = test_server("secret");
        let line = b"{\"token\":\"secret\",\"video_poll\":\"abc\"}\n";
        let started = std::time::Instant::now();
        assert_eq!(exchange(port, line), "timeout\n");
        assert!(started.elapsed() >= REPLY_WAIT);
        // It reached the app, which was too slow.
        assert_eq!(rx.recv().unwrap().cmd.video_poll.as_deref(), Some("abc"));
        // An app that is quitting never gets it: not `timeout` (which says it has it), `gone`.
        drop(rx);
        let pick = b"{\"token\":\"secret\",\"video_pick\":{\"token\":\"abc\",\"choice\":0}}\n";
        assert_eq!(exchange(port, pick), "gone\n");
    }

    #[test]
    fn secrets_compare_whole() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd") && !same_secret("abc", "ab"));
    }

    #[test]
    fn tokens_are_random() {
        assert_ne!(random_token(), random_token());
        assert_eq!(random_token().len(), 64);
    }
}
