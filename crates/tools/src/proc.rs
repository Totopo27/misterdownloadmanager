//! Supervised helper processes (yt-dlp, gallery-dl): every child is in one Windows Job Object
//! that kills it when we exit (even on a crash), runs without a console window, gets its
//! arguments in fixed layers (the user's extras, ours, then the fixed ones and `--` before the
//! URL) with CR / LF / NUL refused, keeps only a short stderr tail for messages, and goes through
//! the chosen network adapter (#35, `crate::relay`).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Lines of stderr kept for error messages.
const TAIL: usize = 20;
/// How many `WARNING:` lines a run keeps (`Spawned::warnings`).
const WARNINGS_KEPT: usize = 50;

#[derive(Debug, thiserror::Error)]
pub enum ProcError {
    #[error("an argument contains a line break or NUL")]
    BadArgument,
    #[error("could not start {0}: {1}")]
    Start(String, std::io::Error),
    #[error("{0} took too long and was stopped")]
    Timeout(String),
    #[error("{tool} failed: {message}")]
    Failed { tool: String, message: String },
    /// The chosen network adapter is down, or yt-dlp can't be held to it here (#35).
    #[error(transparent)]
    Adapter(#[from] mdm_engine::NetError),
}

/// Options → Proxy / Socks → Network adapter (#35) for the helpers and the media-server checks;
/// the app shares it as it changes ([`set_route`]).
static ROUTE: std::sync::RwLock<mdm_engine::Route> = std::sync::RwLock::new(mdm_engine::Route::Any);
/// Options → Connection → DNS (#36): what the relay looks the helpers' servers up with.
static DNS: std::sync::RwLock<Option<mdm_engine::DnsChoice>> = std::sync::RwLock::new(None);
/// The relay the helpers go through while an adapter is chosen, started when first needed.
static RELAY: Mutex<Option<Arc<crate::relay::Relay>>> = Mutex::new(None);

/// The network adapter the helpers and the media-server checks go through (the app's setting).
/// A change closes what the relay has open (it was held to the old route).
pub fn set_route(route: mdm_engine::Route) {
    if *ROUTE.read().unwrap() != route {
        *ROUTE.write().unwrap() = route;
        if let Some(relay) = RELAY.lock().unwrap().as_ref() {
            relay.route_changed();
        }
    }
}

pub fn route() -> mdm_engine::Route {
    ROUTE.read().unwrap().clone()
}

/// The DNS the helpers' servers are looked up with while an adapter is chosen (the app's
/// setting; the system's until set).
pub fn set_dns(dns: mdm_engine::DnsChoice) {
    if DNS.read().unwrap().as_ref() != Some(&dns) {
        *DNS.write().unwrap() = Some(dns);
    }
}

fn dns() -> mdm_engine::DnsChoice {
    DNS.read().unwrap().clone().unwrap_or_default()
}

/// The browsers yt-dlp reads cookies from (`--cookies-from-browser`; yt-dlp's
/// `cookies.SUPPORTED_BROWSERS`), in the order Options lists them. Safari only on macOS.
pub const COOKIE_BROWSERS: [&str; 9] = [
    "firefox", "chrome", "edge", "brave", "opera", "vivaldi", "chromium", "whale", "safari",
];

/// The browsers this system offers (Safari is macOS's only).
pub fn cookie_browsers() -> Vec<&'static str> {
    COOKIE_BROWSERS
        .iter()
        .copied()
        .filter(|b| *b != "safari" || cfg!(target_os = "macos"))
        .collect()
}

/// Options → Sites Logins → Cookies for video sites (#229): the browser whose cookies yt-dlp
/// reads on this computer, and its profile (a name, or a profile folder: a Chromium fork's
/// `chromium:<its profile folder>`), for the runs the browser's extension passed no cookies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserCookies {
    pub browser: String,
    pub profile: Option<String>,
}

impl BrowserCookies {
    /// `browser` (one of [`COOKIE_BROWSERS`]) and its profile; None for any other name.
    pub fn new(browser: &str, profile: &str) -> Option<Self> {
        let browser = browser.trim().to_ascii_lowercase();
        (!browser.is_empty() && COOKIE_BROWSERS.contains(&browser.as_str())).then(|| {
            BrowserCookies {
                browser,
                profile: Some(profile.trim().to_owned()).filter(|p| !p.is_empty()),
            }
        })
    }

    /// yt-dlp's `BROWSER[:PROFILE]`.
    pub fn spec(&self) -> String {
        match &self.profile {
            Some(p) => format!("{}:{p}", self.browser),
            None => self.browser.clone(),
        }
    }
}

/// The browser yt-dlp reads cookies from, as the app shares it ([`set_browser_cookies`]).
static BROWSER_COOKIES: std::sync::RwLock<Option<BrowserCookies>> = std::sync::RwLock::new(None);

/// Options → Sites Logins → Cookies for video sites (the app's setting; None: off).
pub fn set_browser_cookies(chosen: Option<BrowserCookies>) {
    if *BROWSER_COOKIES.read().unwrap() != chosen {
        *BROWSER_COOKIES.write().unwrap() = chosen;
    }
}

pub fn browser_cookies() -> Option<BrowserCookies> {
    BROWSER_COOKIES.read().unwrap().clone()
}

/// The cookies a yt-dlp run with arguments `args` gets from the chosen browser: none when the
/// run has cookies already, else `--cookies-from-browser BROWSER[:PROFILE]`.
///
/// The browser's extension passes the cookies of the very page (or stream) the video was on,
/// from the window and profile it was in (a private window, a Firefox container), as the
/// browser sent them a moment ago (`FromBrowser`, written to the file `--cookies` names). Those
/// win: they are that session's, and reading the whole cookie store of a browser may give
/// another profile's, an older session's, or nothing at all (Chrome on Windows). The browser's
/// store is read only when nothing came: a pasted address, a link typed into Add URL, a
/// playlist listed later. yt-dlp would merge the two if given both; it never is.
fn browser_cookie_args(args: &[String], chosen: Option<&BrowserCookies>) -> Vec<String> {
    let settled = args
        .iter()
        .any(|a| a == "--cookies" || a == "--cookies-from-browser" || a == NO_COOKIES);
    match chosen {
        Some(c) if !settled => vec!["--cookies-from-browser".into(), c.spec()],
        _ => Vec::new(),
    }
}

/// yt-dlp's "no cookies from any browser": a run told this gets none of the chosen browser's.
const NO_COOKIES: &str = "--no-cookies-from-browser";

/// `ours` for a look-up without cookies (#352): the cookie file taken out, the browser's store
/// refused.
fn without_cookies(ours: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(ours.len() + 1);
    let mut args = ours.iter();
    while let Some(a) = args.next() {
        if a == "--cookies" || a == "--cookies-from-browser" {
            args.next();
        } else {
            out.push(a.clone());
        }
    }
    out.push(NO_COOKIES.to_owned());
    out
}

/// Does a run with arguments `ours` have cookies (its own, or the chosen browser's)?
fn with_cookies(ours: &[String], chosen: Option<&BrowserCookies>) -> bool {
    chosen.is_some()
        || ours
            .iter()
            .any(|a| a == "--cookies" || a == "--cookies-from-browser")
}

/// A page's `-J` output, whether it took looking the page up without cookies, and yt-dlp's
/// `WARNING:` lines on the way (#852: the one saying it has no JavaScript runtime).
pub struct LookedUp {
    pub json: Vec<u8>,
    pub without_cookies: bool,
    pub warnings: Vec<String>,
}

/// Look a page up (`yt-dlp -J`, `ours` holding its arguments). yt-dlp's `-J` chooses a format too,
/// and fails when none is left. Signed-in cookies make that likely on YouTube (#352): yt-dlp then
/// asks only the players that need a JavaScript runtime to decode their links, and without one
/// (it looks for Deno alone) every video is dropped: "Requested format is not available". The
/// same page without the cookies lists its qualities, so a look-up that fails this way and had
/// cookies is made once more without them; if that fails too, the first error is the one told.
pub fn look_up(
    exe: &Path,
    ours: &[String],
    url: &str,
    timeout: Duration,
) -> Result<LookedUp, ProcError> {
    // A plain file a server's program sends gets its real type (#695).
    let run = |args: &[String]| {
        output_warned(ytdlp(exe, &[], args, url)?, "yt-dlp", timeout)
            .map(|(json, warnings)| (crate::resolve::typed_scripts(json), warnings))
    };
    match run(ours) {
        Ok((json, warnings)) => Ok(LookedUp {
            json,
            without_cookies: false,
            warnings,
        }),
        Err(first)
            if with_cookies(ours, browser_cookies().as_ref())
                && crate::servers::format_unavailable(&first.to_string()) =>
        {
            match run(&without_cookies(ours)) {
                Ok((json, warnings)) => Ok(LookedUp {
                    json,
                    without_cookies: true,
                    warnings,
                }),
                Err(_) => Err(first),
            }
        }
        Err(e) => Err(e),
    }
}

/// Did yt-dlp's look-up say YouTube's qualities went missing for want of a JavaScript runtime
/// (#852)? yt-dlp needs Deno, Node.js or Bun to solve YouTube's challenges; without one it asks
/// only the players that need none, and their list is short (on some days only the sound and a
/// low quality). Its own words, `warnings` as [`Spawned::warnings`] gives them:
/// - "No supported JavaScript runtime could be found. [...] some formats may be missing" (when
///   it looks a YouTube video up without one);
/// - "n challenge solving failed" / "Signature solving failed: Some formats may be missing"
///   (signed in, it asks the players that need one all the same): a runtime's want only when
///   this computer has none (`runtime_here`); otherwise its solver script is what failed.
pub fn missing_js_runtime(warnings: &[String], runtime_here: bool) -> bool {
    warnings.iter().any(|w| {
        let w = w.to_ascii_lowercase();
        w.contains("no supported javascript runtime")
            || (!runtime_here
                && (w.contains("challenge solving failed")
                    || w.contains("signature solving failed")))
    })
}

/// Has this computer a JavaScript runtime yt-dlp can use (Deno, Node.js or Bun), where the
/// helpers' runs look for one ([`js_runtime_found`])?
pub fn js_runtime_here() -> bool {
    js_runtime_found().is_some()
}

/// Does yt-dlp's `WARNING:` (or `ERROR:`) line say it couldn't read the browser's cookies?
/// Kept on the failure's message (`Spawned::failure`), where `lookup_error` names it.
pub fn cookie_trouble(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    [
        "cookie database",
        "cookies database",
        "decrypt",
        "unknown cookie version",
        "keyring",
        "secretstorage",
        "kwallet",
        "local state",
        "failed to load cookies",
    ]
    .iter()
    .any(|k| l.contains(k))
}

/// Options -> Proxy / Socks for the helpers (#363): yt-dlp and gallery-dl go through the same proxy
/// as MDM's downloads. The app shares it as it changes ([`set_proxy`]).
static PROXY: std::sync::RwLock<mdm_engine::Proxy> =
    std::sync::RwLock::new(mdm_engine::Proxy::None);

/// The proxy every helper run goes through (the app's setting).
pub fn set_proxy(proxy: mdm_engine::Proxy) {
    if *PROXY.read().unwrap() != proxy {
        *PROXY.write().unwrap() = proxy;
    }
}

/// How the proxy for a helper's run is given to it. A login is never put on the command line,
/// where other programs can read it (the cookies neither): only in the environment.
#[derive(Debug, PartialEq, Eq)]
enum ProxyGiven {
    /// Nothing: yt-dlp keeps doing what it always did (the system's and the environment's).
    Nothing,
    /// `--proxy URL`.
    Argument(String),
    /// `HTTP_PROXY` and the like: the address with its login.
    Environment(String),
}

/// The proxy for `url` under `proxy`, as a helper takes it. `None` proxy and an address the
/// setting doesn't cover (Manual with its scheme unticked; a script that says DIRECT): nothing,
/// so the helper does what it did before. A script that gives no usable answer leaves it to the
/// system's settings, as for a download.
fn proxy_given(proxy: &mdm_engine::Proxy, url: Option<&url::Url>) -> ProxyGiven {
    use mdm_engine::{Proxy, pac};
    let system = |url: Option<&url::Url>| {
        // A helper's run with no address known yet (a page's looked-up info) asks for any.
        let any = url::Url::parse("https://example.com/").expect("a valid address");
        mdm_engine::net::system_proxy(url.unwrap_or(&any))
    };
    let plain = |p: Option<String>| p.map_or(ProxyGiven::Nothing, ProxyGiven::Argument);
    match proxy {
        Proxy::None => ProxyGiven::Nothing,
        Proxy::System => plain(system(url)),
        Proxy::Pac { script } => {
            let first = url.and_then(|u| {
                pac::routes(script, u)
                    .ok()?
                    .into_iter()
                    .next()
                    .map(|r| match r {
                        pac::Route::Direct => None,
                        pac::Route::Proxy(p) => Some(p),
                    })
            });
            match first {
                Some(Some(p)) => ProxyGiven::Argument(p),
                Some(None) => ProxyGiven::Nothing,
                None => plain(system(url)),
            }
        }
        Proxy::Manual {
            url: address,
            login,
            http,
            https,
        } => {
            let https_page = url.is_none_or(|u| u.scheme() == "https");
            if !(if https_page { *https } else { *http }) {
                return ProxyGiven::Nothing;
            }
            match login {
                None => ProxyGiven::Argument(address.clone()),
                Some((user, pass)) => {
                    let Ok(mut u) = url::Url::parse(address) else {
                        return ProxyGiven::Nothing;
                    };
                    let _ = u.set_username(user);
                    let _ = u.set_password(Some(pass));
                    ProxyGiven::Environment(u.to_string())
                }
            }
        }
    }
}

/// Give `helper` the proxy for `url`: returns the arguments to pass (before `--`).
fn through_proxy(cmd: &mut Command, url: Option<&str>) -> Vec<String> {
    let page = url.and_then(|u| url::Url::parse(u).ok());
    match proxy_given(&PROXY.read().unwrap(), page.as_ref()) {
        ProxyGiven::Nothing => Vec::new(),
        ProxyGiven::Argument(p) => vec!["--proxy".into(), p],
        ProxyGiven::Environment(p) => {
            for var in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ] {
                cmd.env(var, &p);
            }
            cmd.env_remove("NO_PROXY").env_remove("no_proxy");
            Vec::new()
        }
    }
}

/// The helpers' relay, started on first use.
fn relay() -> Result<Arc<crate::relay::Relay>, ProcError> {
    let mut slot = RELAY.lock().unwrap();
    if let Some(r) = slot.as_ref() {
        return Ok(r.clone());
    }
    let relay = crate::relay::Relay::start(Arc::new(|| (route(), dns())))
        .map(Arc::new)
        .map_err(|e| ProcError::Start("the network adapter's relay".into(), e))?;
    *slot = Some(relay.clone());
    Ok(relay)
}

/// Which helper [`through_adapter`] sets up.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Helper {
    YtDlp,
    GalleryDl,
}

/// A helper and the chosen adapter (#35): nothing to do when there's none; refused while it is
/// down; else every connection the helper and what it starts make goes through the relay
/// (`crate::relay`), which holds them to the adapter. The arguments returned go before `--`.
///
/// - yt-dlp: `--proxy` for its own requests (and the external downloaders it passes it to), and
///   `--geo-verification-proxy`. The ffmpeg it starts gets the proxy from it (`http_proxy`), and
///   may only open what goes through it: HTTP(S) and the layers under them, local files, `data:`
///   and `crypto:` (`-protocol_whitelist`). RTMP and FTP formats go to that ffmpeg, which refuses
///   them, instead of rtmpdump or yt-dlp's own FTP, which would ignore the proxy; RTSP and MMS
///   formats (mpv or mplayer, in older yt-dlp) stop the download (`--break-match-filters`; `?`
///   lets an entry not yet chosen pass, a playlist's).
/// - gallery-dl: `--proxy`.
/// - Both, and anything they start: `HTTP_PROXY`, `HTTPS_PROXY` and `ALL_PROXY` (curl, aria2c,
///   gallery-dl's Python), with `NO_PROXY` taken away so no host is let around it.
fn through_adapter(
    cmd: &mut Command,
    helper: Helper,
    url: Option<&str>,
) -> Result<Vec<String>, ProcError> {
    match route() {
        // No adapter: Options -> Proxy / Socks (#363).
        mdm_engine::Route::Any => Ok(through_proxy(cmd, url)),
        mdm_engine::Route::Down { label } => Err(mdm_engine::NetError::Down { label }.into()),
        mdm_engine::Route::Bound(_) => {
            let url = relay()?.url();
            for var in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ] {
                cmd.env(var, &url);
            }
            cmd.env_remove("NO_PROXY").env_remove("no_proxy");
            Ok(match helper {
                Helper::YtDlp => vec![
                    "--proxy".into(),
                    url.clone(),
                    "--geo-verification-proxy".into(),
                    url,
                    // yt-dlp's keys are http, ftp, m3u8, dash and rtmp only: anything else
                    // ("rtsp:ffmpeg") would make ffmpeg the downloader for everything.
                    "--downloader".into(),
                    "rtmp:ffmpeg".into(),
                    "--downloader".into(),
                    "ftp:ffmpeg".into(),
                    "--downloader-args".into(),
                    format!("ffmpeg_i:-protocol_whitelist {FFMPEG_PROTOCOLS}"),
                    "--break-match-filters".into(),
                    "protocol!*=?rtsp & protocol!*=?mms".into(),
                ],
                Helper::GalleryDl => vec!["--proxy".into(), url],
            })
        }
    }
}

/// What the ffmpeg yt-dlp starts may open while an adapter is chosen: HTTP(S) through the relay
/// (`httpproxy`, `tls` and `tcp` are the layers under them; HLS and DASH open nothing else) and
/// what stays on this computer.
const FFMPEG_PROTOCOLS: &str = "file,crypto,data,http,https,tls,tcp,httpproxy";

/// `gallery-dl` (#58) with arguments in the same layers as [`ytdlp`]: `ours`, then the fixed
/// ones (no configuration file of the person's, never a question on the console), then `--` and
/// the address. Its output is read as UTF-8, as yt-dlp's is (#79). With an adapter chosen it goes
/// through the relay (#35, [`through_adapter`]). Only options `tool`'s version accepts
/// (`gallery::Accepts`, #236: a distribution's may be older than MDM's own copy).
pub fn gallery_dl(
    tool: &crate::Installed,
    ours: &[String],
    url: &str,
) -> Result<Command, ProcError> {
    let mut cmd = Command::new(&tool.exe);
    let route = through_adapter(&mut cmd, Helper::GalleryDl, Some(url))?;
    cmd.env("PYTHONIOENCODING", "utf-8");
    for a in ours {
        cmd.arg(safe(a)?);
    }
    cmd.arg("--ignore-config");
    if crate::gallery::Accepts::of(&tool.version).no_input {
        cmd.arg("--no-input");
    }
    cmd.args(route);
    cmd.arg("--");
    cmd.arg(safe(url)?);
    Ok(cmd)
}

/// Text that may go into an argument list: no line breaks or NUL (they would let a page title
/// or URL smuggle in options or split a line-based protocol).
pub fn safe(s: &str) -> Result<&str, ProcError> {
    if s.contains(['\r', '\n', '\0']) {
        Err(ProcError::BadArgument)
    } else {
        Ok(s)
    }
}

/// What the browser's request for a page carried, so yt-dlp asks as the browser did: the page
/// that embeds a player (Referer), the browser's User-Agent, and the page's cookies, which sign a
/// member's or a site's own videos in: each with its domain and flags (`jar`), or, from an older
/// extension, a Cookie header's `name=value; …` (`cookies`). For a stream the page's player
/// asked for, the `Origin` it sent too (#900): a site's video server may serve its own pages'
/// players only.
#[derive(Clone, Debug, Default)]
pub struct FromBrowser {
    pub referer: Option<String>,
    pub origin: Option<String>,
    pub user_agent: Option<String>,
    pub cookies: Option<String>,
    pub jar: Vec<mdm_ipc::BrowserCookie>,
}

/// A private cookie file in the Netscape format yt-dlp's `--cookies` reads, removed when
/// dropped (yt-dlp may write it back at its end: keep it until then).
pub struct CookieFile(PathBuf);

impl Drop for CookieFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl FromBrowser {
    /// yt-dlp's arguments for `url`'s page, and the cookie file they name. Cookies never go on
    /// the command line, where other programs can read them.
    pub fn args(&self, url: &str) -> Result<(Vec<String>, Option<CookieFile>), ProcError> {
        fn given(s: &Option<String>) -> Option<&str> {
            s.as_deref().map(str::trim).filter(|s| !s.is_empty())
        }
        let mut args = Vec::new();
        if let Some(r) = given(&self.referer) {
            args.extend(["--referer".to_owned(), safe(r)?.to_owned()]);
        }
        if let Some(ua) = given(&self.user_agent) {
            args.extend(["--user-agent".to_owned(), safe(ua)?.to_owned()]);
        }
        // yt-dlp sends it with the address and keeps it in each format's headers, which MDM's
        // engine sends with the stream's playlists and each of its pieces (#769).
        if let Some(origin) = given(&self.origin) {
            args.extend([
                "--add-header".to_owned(),
                format!("Origin:{}", safe(origin)?),
            ]);
        }
        let host = url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned));
        let text = match (given(&self.cookies), host) {
            _ if !self.jar.is_empty() => Some(jar_cookies(&self.jar)),
            (Some(cookies), Some(host)) => Some(header_cookies(&host, cookies)),
            _ => None,
        };
        let file = match text {
            Some(text) => {
                let file = cookie_file(&text).map_err(|e| ProcError::Start("yt-dlp".into(), e))?;
                args.extend([
                    "--cookies".to_owned(),
                    file.0.to_string_lossy().into_owned(),
                ]);
                Some(file)
            }
            _ => None,
        };
        Ok((args, file))
    }
}

/// The first line of a cookie file in the Netscape format (yt-dlp's `--cookies`). Each cookie is
/// a line: domain, sent to its subdomains too (TRUE, FALSE), path, HTTPS only (TRUE, FALSE),
/// expiry (seconds since 1970; 0: a session cookie), name, value, separated by tabs.
const NETSCAPE: &str = "# Netscape HTTP Cookie File\n";

/// Can `s` go in a cookie file's field (no tab or line break to shift the others)?
fn field(s: &str) -> bool {
    !s.contains(['\t', '\r', '\n', '\0'])
}

/// The browser's cookies (`cookies.getAll`'s), each for its own domain, path and flags: yt-dlp
/// sends a cookie of `.youtube.com` to www.youtube.com when the page was music.youtube.com, and
/// one of the page's host alone to that host only.
fn jar_cookies(jar: &[mdm_ipc::BrowserCookie]) -> String {
    let flag = |b: bool| if b { "TRUE" } else { "FALSE" };
    let mut text = String::from(NETSCAPE);
    for c in jar {
        let host = c.domain.trim_start_matches('.');
        let usable = !c.name.is_empty()
            && !host.is_empty()
            && !host.contains([' ', '/'])
            && [&c.name, &c.value, &c.domain, &c.path]
                .iter()
                .all(|s| field(s));
        if !usable {
            continue;
        }
        let domain = if c.host_only {
            host.to_owned()
        } else {
            format!(".{host}")
        };
        let path = if c.path.starts_with('/') {
            c.path.as_str()
        } else {
            "/"
        };
        let expires = c
            .expires
            .filter(|e| e.is_finite() && *e >= 1.0)
            .map_or(0, |e| e as u64);
        text.push_str(&format!(
            "{domain}\t{}\t{path}\t{}\t{expires}\t{}\t{}\n",
            flag(!c.host_only),
            flag(c.secure),
            c.name,
            c.value
        ));
    }
    text
}

/// An older extension's Cookie header (`a=1; b=2`, the page's cookies without their domains),
/// for the site's domain and its subdomains ([`header_domain`]), as session cookies.
fn header_cookies(host: &str, cookies: &str) -> String {
    let domain = header_domain(host);
    let mut text = String::from(NETSCAPE);
    for pair in cookies.split(';') {
        let Some((name, value)) = pair.trim().split_once('=') else {
            continue;
        };
        if ![domain, name, value].iter().all(|s| field(s)) {
            continue;
        }
        text.push_str(&format!(".{domain}\tTRUE\t/\tFALSE\t0\t{name}\t{value}\n"));
    }
    text
}

/// Where a page's cookies without their domains go: its host, or the site's own domain where
/// yt-dlp asks another of its hosts. Every YouTube host (music., m., www.: yt-dlp looks videos
/// up on www.youtube.com, and the sign-in cookies are `.youtube.com`'s), and a site's mobile
/// pages (m., mobile.), whose sign-in is the site's.
fn header_domain(host: &str) -> &str {
    let host = host.trim_end_matches('.');
    let youtube = "youtube.com";
    let lower = host.to_ascii_lowercase();
    if lower == youtube || lower.ends_with(".youtube.com") {
        return &host[host.len() - youtube.len()..];
    }
    for mobile in ["m.", "mobile."] {
        if let Some(site) = host.strip_prefix(mobile)
            && site.contains('.')
        {
            return site;
        }
    }
    host
}

/// A cookie file's `text` ([`jar_cookies`], [`header_cookies`]) in a file only this user can
/// read, in the helpers' temporary folder ([`crate::workdir::temp_dir`]).
fn cookie_file(text: &str) -> std::io::Result<CookieFile> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let path = crate::workdir::temp_dir().join(format!(
        "mdm-cookies-{}-{}.txt",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    // The temporary folder is the user's own on Windows; elsewhere it may be shared.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let file = CookieFile(path);
    open.open(&file.0)?.write_all(text.as_bytes())?;
    Ok(file)
}

/// What yt-dlp prints is read as UTF-8 (#79): on a Windows with a legacy code page (Turkish,
/// Russian…) Python writes to a pipe in that code page, and a title's letters made the
/// final-path line unreadable. `--encoding` sets yt-dlp's own output, `PYTHONIOENCODING`
/// Python's.
const UTF8: [&str; 2] = ["--encoding", "utf-8"];

fn ytdlp_command(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.env("PYTHONIOENCODING", "utf-8");
    cmd
}

/// The user's extras and ours, then the chosen browser's cookies when the run has none
/// ([`browser_cookie_args`]). Reading them, yt-dlp's warnings stay on (a look-up's
/// `--no-warnings` is dropped): the one saying the browser's cookies couldn't be decrypted is
/// what explains the site's "sign in" that follows (`Spawned::failure`).
fn layers(cmd: &mut Command, user: &[String], ours: &[String]) -> Result<(), ProcError> {
    layers_with(cmd, user, ours, browser_cookies().as_ref())
}

fn layers_with(
    cmd: &mut Command,
    user: &[String],
    ours: &[String],
    chosen: Option<&BrowserCookies>,
) -> Result<(), ProcError> {
    let all: Vec<String> = user.iter().chain(ours).cloned().collect();
    let from_store = browser_cookie_args(&all, chosen);
    for a in &all {
        if from_store.is_empty() || a != "--no-warnings" {
            cmd.arg(safe(a)?);
        }
    }
    for a in &from_store {
        cmd.arg(safe(a)?);
    }
    Ok(())
}

/// The file named `name` (`name.exe` on Windows) in one of `dirs`.
fn program_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    dirs.iter().map(|d| d.join(&file)).find(|p| p.is_file())
}

/// Node.js from nvm (`~/.nvm/versions/node/<version>/bin/node`), the newest name first: a program
/// started from the desktop doesn't have nvm's folders on its PATH (#360).
fn nvm_node(home: &Path) -> Option<PathBuf> {
    let mut versions: Vec<PathBuf> = std::fs::read_dir(home.join(".nvm/versions/node"))
        .ok()?
        .flatten()
        .map(|e| e.path().join("bin"))
        .collect();
    versions.sort();
    versions.reverse();
    program_in(&versions, "node")
}

/// What yt-dlp should be told about JavaScript (#360, #352, #351): YouTube's formats need a
/// runtime, and yt-dlp looks for Deno alone, on the PATH, unless told of another. Deno on the
/// PATH (`dirs`): nothing to say. Else Deno where its own installer puts it
/// (`~/.deno/bin`, #852: no PATH of a program started from the desktop has it), or in `more`
/// (folders a runtime installed after MDM started is in, [`runtime_dirs`]); else Node.js or Bun.
fn js_runtime(
    dirs: &[PathBuf],
    more: &[PathBuf],
    home: Option<&Path>,
) -> Option<(&'static str, PathBuf)> {
    if program_in(dirs, "deno").is_some() {
        return None;
    }
    let own_deno = home.map(|h| h.join(".deno").join("bin"));
    program_in(own_deno.as_slice(), "deno")
        .or_else(|| program_in(more, "deno"))
        .map(|p| ("deno", p))
        .or_else(|| {
            program_in(dirs, "node")
                .or_else(|| home.and_then(nvm_node))
                .or_else(|| program_in(more, "node"))
                .map(|p| ("node", p))
        })
        .or_else(|| program_in(dirs, "bun").map(|p| ("bun", p)))
}

/// Where Windows' installers put a JavaScript runtime (#852): winget's links (Deno, Node.js) and
/// Node.js's own installer. They add these to the PATH of programs started after them, not to
/// MDM's own, so a runtime installed while MDM runs is found here when the video is looked up
/// again. None elsewhere: a package manager's folder is on the PATH already ([`usable_dirs`]).
fn runtime_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let under = |var: &str, rel: &str| std::env::var_os(var).map(|v| Path::new(&v).join(rel));
        [
            under("LOCALAPPDATA", r"Microsoft\WinGet\Links"),
            under("ProgramFiles", "nodejs"),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
    #[cfg(not(windows))]
    Vec::new()
}

/// The folders of `path` (a PATH value) a helper can walk, in their order (#508): yt-dlp looks
/// through the PATH folders, and one Windows refused to open ("[WinError 448] The path cannot be
/// traversed because it contains an untrusted mount point": a junction Redirection Guard
/// doesn't trust) ended the look-up. A folder MDM can't open (gone, refused, not a folder) is
/// left out; one reached through a junction or link is given as the folder it leads to, so the
/// helper crosses no mount point on its way, even when Windows holds the helper to stricter
/// rules than MDM. Every other folder stays as it was written.
///
/// On Linux and macOS the package managers' folders follow, when the PATH lacks them (#659): a
/// helper found there ([`crate::system`]) looks there for its own tools too (yt-dlp for Deno
/// and ffmpeg), even when MDM was opened from Finder, whose PATH has no Homebrew.
fn usable_dirs(path: &std::ffi::OsStr) -> Vec<PathBuf> {
    #[allow(unused_mut)]
    let mut dirs: Vec<PathBuf> = std::env::split_paths(path)
        .filter_map(|dir| usable_dir(&dir))
        .collect();
    #[cfg(unix)]
    for extra in crate::system::PACKAGE_DIRS.map(PathBuf::from) {
        if !dirs.contains(&extra)
            && let Some(dir) = usable_dir(&extra)
        {
            dirs.push(dir);
        }
    }
    dirs
}

fn usable_dir(dir: &Path) -> Option<PathBuf> {
    if !std::fs::metadata(dir).ok()?.is_dir() {
        return None;
    }
    // Only Windows refuses mount points (Redirection Guard); elsewhere a link is just followed.
    #[cfg(windows)]
    if dir.is_absolute()
        && dir.ancestors().any(reparse_point)
        && let Ok(real) = std::fs::canonicalize(dir)
    {
        return Some(without_verbatim(real));
    }
    Some(dir.to_owned())
}

/// A junction, a link or a mounted volume's folder.
#[cfg(windows)]
fn reparse_point(p: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    std::fs::symlink_metadata(p)
        .is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

/// `\\?\C:\x` as `C:\x` and `\\?\UNC\server\share` as `\\server\share`, the way PATH writes
/// them (`canonicalize` gives the long forms).
#[cfg(windows)]
fn without_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p.clone(),
    }
}

/// The PATH a helper gets ([`usable_dirs`] of `path`); `None` when it can't be written back
/// (then it's left as it was).
fn child_path(path: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
    std::env::join_paths(usable_dirs(path)).ok()
}

/// Give `cmd` the PATH of [`child_path`]: of the one it was given, else of MDM's own. Every
/// helper starts here ([`spawn`]), and ffmpeg, started by yt-dlp, inherits it.
fn with_usable_path(cmd: &mut Command) {
    let is_path = |k: &std::ffi::OsStr| {
        if cfg!(windows) {
            k.eq_ignore_ascii_case("PATH")
        } else {
            k == "PATH"
        }
    };
    let path = match cmd.get_envs().find(|(k, _)| is_path(k)) {
        Some((_, Some(given))) => given.to_owned(),
        Some((_, None)) => return,
        None => match std::env::var_os("PATH") {
            Some(own) => own,
            None => return,
        },
    };
    if let Some(usable) = child_path(&path) {
        cmd.env("PATH", usable);
    }
}

/// This computer's JavaScript runtime for yt-dlp ([`js_runtime`]): `Some` with the arguments
/// that tell yt-dlp of it (none for Deno on the PATH, which it finds itself), in the PATH
/// folders a helper gets ([`usable_dirs`]: a node.exe behind a junction is named by its real
/// folder). Kept once found; while there is none it is looked for again at each run, so one
/// installed meanwhile counts at the next look-up (#852).
fn js_runtime_found() -> Option<Vec<String>> {
    static FOUND: Mutex<Option<Vec<String>>> = Mutex::new(None);
    let mut found = FOUND
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if found.is_none() {
        let dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| usable_dirs(&p))
            .unwrap_or_default();
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        *found = if program_in(&dirs, "deno").is_some() {
            Some(Vec::new())
        } else {
            js_runtime(&dirs, &runtime_dirs(), home.as_deref()).map(|(name, path)| {
                vec![
                    "--js-runtimes".to_owned(),
                    format!("{name}:{}", path.display()),
                ]
            })
        };
    }
    found.clone()
}

/// `--js-runtimes` for this computer's runtime, when yt-dlp must be told of it.
fn js_runtime_args() -> Vec<String> {
    js_runtime_found().unwrap_or_default()
}

/// `yt-dlp` with arguments in layers: `user` extras, then `ours` (then the chosen browser's
/// cookies when neither brought any, [`layers`]), then the fixed ones last (so nothing can
/// switch them off), then `--` and the URL (so a URL starting with `-` is never read as an
/// option).
pub fn ytdlp(
    exe: &Path,
    user: &[String],
    ours: &[String],
    url: &str,
) -> Result<Command, ProcError> {
    let mut cmd = ytdlp_command(exe);
    let route = through_adapter(&mut cmd, Helper::YtDlp, Some(url))?;
    layers(&mut cmd, user, ours)?;
    cmd.args(["--ignore-config", "--no-exec", "--no-update"]);
    cmd.args(js_runtime_args());
    cmd.args(UTF8);
    cmd.args(route);
    cmd.arg("--");
    cmd.arg(safe(url)?);
    Ok(cmd)
}

/// `yt-dlp` working from a looked-up page (`-J` output, maybe edited) instead of an address:
/// the same layers, then `--load-info-json` and the file.
pub fn ytdlp_info(
    exe: &Path,
    user: &[String],
    ours: &[String],
    info: &Path,
) -> Result<Command, ProcError> {
    let mut cmd = ytdlp_command(exe);
    let route = through_adapter(&mut cmd, Helper::YtDlp, None)?;
    layers(&mut cmd, user, ours)?;
    cmd.args(["--ignore-config", "--no-exec", "--no-update"]);
    cmd.args(js_runtime_args());
    cmd.args(UTF8);
    cmd.args(route);
    cmd.arg("--load-info-json");
    cmd.arg(info);
    Ok(cmd)
}

/// A job of its own for one helper and every process it starts (#61): on Windows `yt-dlp.exe`
/// is a launcher whose child does the work, and ffmpeg runs under that child. Closing the job
/// (`Spawned::kill`, or MDM ending in any way) ends them all; ending the launcher alone left the
/// download running, holding its file and the bandwidth.
#[cfg(windows)]
fn new_job() -> Option<win32job::Job> {
    let mut info = win32job::ExtendedLimitInfo::new();
    info.limit_kill_on_job_close();
    win32job::Job::create_with_limit_info(&info).ok()
}

/// A running helper with its stderr tail.
pub struct Spawned {
    pub child: Child,
    tail: Arc<Mutex<VecDeque<String>>>,
    /// Every `WARNING:` line (a few dozen at most): a subtitle yt-dlp couldn't get is said early,
    /// and the tail has moved past it by the end (#68).
    warnings: Arc<Mutex<Vec<String>>>,
    /// The stderr reader has read to the end (the helper and all it started closed it).
    stderr_done: Arc<std::sync::atomic::AtomicBool>,
    name: String,
    #[cfg(windows)]
    job: Option<win32job::Job>,
}

impl Spawned {
    /// End the helper and everything it started (Windows: its job; elsewhere yt-dlp runs as one
    /// process, its ffmpeg ending with it).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        #[cfg(windows)]
        drop(self.job.take());
    }

    /// The last stderr lines, for an error message.
    pub fn tail(&self) -> String {
        self.tail
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The tool's `WARNING:` lines so far, oldest first, without the word.
    pub fn warnings(&self) -> Vec<String> {
        self.warnings.lock().unwrap().clone()
    }

    /// Once it has ended: wait for its last stderr lines (the reader is another thread, and on
    /// a busy machine it comes after the exit was seen), up to two seconds, as something it
    /// started may keep stderr open.
    fn settle(&self) {
        let end = Instant::now() + Duration::from_secs(2);
        while !self.stderr_done.load(std::sync::atomic::Ordering::Acquire) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A failure with what the tool said (its last `ERROR:` line, else its last line). Called
    /// once it has ended: its last lines are waited for (the stderr reader is another thread, and
    /// on a busy machine it came after the exit was seen: "stopped without saying why"), up to
    /// two seconds, as something it started may keep stderr open.
    pub fn failure(&self) -> ProcError {
        self.settle();
        let tail = self.tail.lock().unwrap();
        let message = tail
            .iter()
            .rev()
            .find(|l| l.starts_with("ERROR:"))
            .or(tail.back())
            .map(|l| l.trim_start_matches("ERROR:").trim().to_owned())
            .unwrap_or_else(|| "it stopped without saying why".into());
        let warnings = self.warnings.lock().unwrap();
        ProcError::Failed {
            tool: self.name.clone(),
            message: with_cookie_trouble(message, &warnings, &tail),
        }
    }
}

/// A failure's `message` with what yt-dlp said earlier about the browser's cookies, when that
/// isn't the message itself (#229): Chrome's cookies it couldn't decrypt are only a warning,
/// and the site's "sign in" that follows is the error; "failed to load cookies" comes after the
/// line saying why.
fn with_cookie_trouble(message: String, warnings: &[String], tail: &VecDeque<String>) -> String {
    let earlier = warnings
        .iter()
        .map(String::as_str)
        .chain(tail.iter().map(|l| {
            l.trim_start_matches("ERROR:")
                .trim_start_matches("WARNING:")
                .trim()
        }))
        .find(|l| cookie_trouble(l) && *l != message && !message.contains(*l));
    match earlier {
        Some(why) => format!("{message} ({why})"),
        None => message,
    }
}

/// Start `cmd` supervised: no window, in the job, stdout piped for the caller, stderr kept as a
/// tail, with a PATH it can walk ([`with_usable_path`], #508), in a folder it can write that is
/// also its TEMP ([`crate::workdir`], #834: never MDM's install folder).
pub fn spawn(mut cmd: Command, name: &str) -> Result<Spawned, ProcError> {
    with_usable_path(&mut cmd);
    crate::workdir::apply(&mut cmd, crate::workdir::folders());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| ProcError::Start(name.to_owned(), e))?;
    #[cfg(windows)]
    let job = new_job().filter(|job| {
        use std::os::windows::io::AsRawHandle;
        job.assign_process(child.as_raw_handle() as isize).is_ok()
    });
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let stderr_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(err) = child.stderr.take() {
        let (tail, warnings, done) = (tail.clone(), warnings.clone(), stderr_done.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if let Some(w) = line.strip_prefix("WARNING:") {
                    let mut ws = warnings.lock().unwrap();
                    if ws.len() < WARNINGS_KEPT {
                        ws.push(w.trim().to_owned());
                    }
                }
                let mut t = tail.lock().unwrap();
                if t.len() >= TAIL {
                    t.pop_front();
                }
                t.push_back(line);
            }
            done.store(true, std::sync::atomic::Ordering::Release);
        });
    } else {
        stderr_done.store(true, std::sync::atomic::Ordering::Release);
    }
    Ok(Spawned {
        child,
        tail,
        warnings,
        stderr_done,
        name: name.to_owned(),
        #[cfg(windows)]
        job,
    })
}

/// Run `cmd` to the end within `timeout` and return its stdout (killed on timeout).
pub fn output(cmd: Command, name: &str, timeout: Duration) -> Result<Vec<u8>, ProcError> {
    output_warned(cmd, name, timeout).map(|(out, _)| out)
}

/// [`output`], with the `WARNING:` lines it wrote ([`Spawned::warnings`]).
pub fn output_warned(
    cmd: Command,
    name: &str,
    timeout: Duration,
) -> Result<(Vec<u8>, Vec<String>), ProcError> {
    let mut s = spawn(cmd, name)?;
    let mut out = s.child.stdout.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    loop {
        match s.child.try_wait() {
            Ok(Some(status)) => {
                let stdout = reader.join().unwrap_or_default();
                return if status.success() {
                    s.settle();
                    Ok((stdout, s.warnings()))
                } else {
                    // `failure` waits for the stderr reader's last lines.
                    Err(s.failure())
                };
            }
            Ok(None) if Instant::now() >= deadline => {
                s.kill();
                let _ = s.child.wait();
                return Err(ProcError::Timeout(name.to_owned()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(ProcError::Start(name.to_owned(), e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browsers_context_for_ytdlp() {
        let b = FromBrowser {
            referer: Some("https://course.test/lesson".into()),
            user_agent: Some("UA/1".into()),
            cookies: Some("sid=abc; theme=dark".into()),
            ..Default::default()
        };
        let (args, file) = b.args("https://player.test/video/1").unwrap();
        assert_eq!(
            &args[..4],
            [
                "--referer",
                "https://course.test/lesson",
                "--user-agent",
                "UA/1"
            ]
        );
        assert_eq!(args[4], "--cookies");
        let file = file.unwrap();
        let text = std::fs::read_to_string(&file.0).unwrap();
        assert!(text.starts_with("# Netscape HTTP Cookie File"));
        assert!(text.contains(".player.test\tTRUE\t/\tFALSE\t0\tsid\tabc\n"));
        assert!(
            !args.iter().any(|a| a.contains("abc")),
            "no cookie on the command line"
        );
        let path = file.0.clone();
        drop(file);
        assert!(!path.exists(), "removed once done");
        let none = FromBrowser::default().args("https://x.test/").unwrap();
        assert!(none.0.is_empty() && none.1.is_none());
    }

    /// #900: the Origin a page's player sent with a stream goes to yt-dlp as a header of its
    /// own, which yt-dlp keeps in each format's headers (the engine's for every piece); one that
    /// would break the command line is refused, as a Referer is.
    #[test]
    fn the_origin_the_player_sent_goes_to_ytdlp() {
        let b = FromBrowser {
            referer: Some("https://player.test/".into()),
            origin: Some("https://player.test".into()),
            ..Default::default()
        };
        let (args, _) = b.args("https://cdn.test/x.m3u8").unwrap();
        assert_eq!(
            args,
            [
                "--referer",
                "https://player.test/",
                "--add-header",
                "Origin:https://player.test"
            ]
        );
        let bad = FromBrowser {
            origin: Some("https://x.test\r\nCookie: a=1".into()),
            ..Default::default()
        };
        assert!(matches!(
            bad.args("https://x.test/"),
            Err(ProcError::BadArgument)
        ));
    }

    /// #229: the button over a YouTube video sends the watch page (its player plays a blob:, and
    /// a video's own requests aren't a manifest) with the cookies the browser has for that page:
    /// youtube.com's sign-in, `__Secure-` ones included. An older extension sends them as a
    /// header, without their domains: they are filed for youtube.com and its subdomains, so they
    /// reach www.youtube.com, the host yt-dlp's YouTube extractor asks (and checks the sign-in
    /// on), from a YouTube Music or mobile page too; as session cookies (expiry 0, which
    /// yt-dlp's loader keeps) sent over HTTP and HTTPS.
    #[test]
    fn a_youtube_pages_cookies_reach_the_host_ytdlp_asks() {
        let b = FromBrowser {
            cookies: Some(
                "VISITOR_INFO1_LIVE=v; SAPISID=s/1; __Secure-3PAPISID=s3; __Secure-3PSID=p3; \
                 LOGIN_INFO=AFmmF2s:QUQ3; __Host-GAPS=1:g"
                    .into(),
            ),
            ..Default::default()
        };
        let (args, file) = b
            .args("https://www.youtube.com/watch?v=A1WG3x-Ebpk")
            .unwrap();
        assert_eq!(args[0], "--cookies");
        let file = file.unwrap();
        let text = std::fs::read_to_string(&file.0).unwrap();
        for (name, value) in [
            ("VISITOR_INFO1_LIVE", "v"),
            ("SAPISID", "s/1"),
            ("__Secure-3PAPISID", "s3"),
            ("__Secure-3PSID", "p3"),
            ("LOGIN_INFO", "AFmmF2s:QUQ3"),
            ("__Host-GAPS", "1:g"),
        ] {
            let line = format!(".youtube.com\tTRUE\t/\tFALSE\t0\t{name}\t{value}\n");
            assert!(text.contains(&line), "{line:?} in {text}");
        }
        assert_eq!(text.lines().count(), 7, "the header and six cookies");
        for page in [
            "https://music.youtube.com/watch?v=A1WG3x-Ebpk",
            "https://m.youtube.com/watch?v=A1WG3x-Ebpk",
            "https://youtube.com/shorts/A1WG3x-Ebpk",
        ] {
            let (_, file) = b.args(page).unwrap();
            let text = std::fs::read_to_string(&file.unwrap().0).unwrap();
            assert!(
                text.contains(".youtube.com\tTRUE\t/\tFALSE\t0\tSAPISID\ts/1\n"),
                "{page}: {text}"
            );
        }
    }

    /// The domain an older extension's header is filed under: the page's host, or its site's
    /// where yt-dlp asks another of the site's hosts.
    #[test]
    fn a_header_goes_to_the_sites_own_domain() {
        for (host, domain) in [
            ("www.youtube.com", "youtube.com"),
            ("music.youtube.com", "youtube.com"),
            ("m.youtube.com", "youtube.com"),
            ("youtube.com", "youtube.com"),
            ("Music.YouTube.com.", "YouTube.com"),
            ("m.facebook.com", "facebook.com"),
            ("mobile.twitter.com", "twitter.com"),
            ("m.vk.com", "vk.com"),
            ("player.test", "player.test"),
            ("www.example.org", "www.example.org"),
            ("notyoutube.com", "notyoutube.com"),
            ("youtube.com.evil.test", "youtube.com.evil.test"),
            ("m.org", "m.org"),
            ("127.0.0.1", "127.0.0.1"),
        ] {
            assert_eq!(header_domain(host), domain, "{host}");
        }
    }

    fn cookie(name: &str, domain: &str, host_only: bool) -> mdm_ipc::BrowserCookie {
        mdm_ipc::BrowserCookie {
            name: name.into(),
            value: format!("{name}-v"),
            domain: domain.into(),
            host_only,
            path: "/".into(),
            ..Default::default()
        }
    }

    /// A newer extension sends each cookie with the domain, path and flags the browser keeps: the
    /// file has each as it is (a host's own cookie for that host alone, a site's for its
    /// subdomains too), and the header, sent for older apps, is left out.
    #[test]
    fn each_cookie_keeps_its_domain_and_flags() {
        let b = FromBrowser {
            cookies: Some("SID=SID-v; pref=pref-v".into()),
            jar: vec![
                mdm_ipc::BrowserCookie {
                    secure: true,
                    expires: Some(1893456000.75),
                    path: "/".into(),
                    ..cookie("SID", ".youtube.com", false)
                },
                cookie("pref", "music.youtube.com", true),
                // Without a leading dot, as Firefox may name a domain cookie.
                mdm_ipc::BrowserCookie {
                    path: "/account".into(),
                    ..cookie("acct", "youtube.com", false)
                },
                // A path that isn't one: the whole site.
                mdm_ipc::BrowserCookie {
                    path: "account".into(),
                    ..cookie("odd", "youtube.com", true)
                },
                // Never a field that would shift the others, an empty name or domain.
                cookie("tab\tname", ".youtube.com", false),
                mdm_ipc::BrowserCookie {
                    value: "a\nb".into(),
                    ..cookie("line", ".youtube.com", false)
                },
                cookie("", ".youtube.com", false),
                cookie("nodomain", "", false),
                cookie("dots", "...", false),
                cookie("slash", "youtube.com/x", false),
            ],
            ..Default::default()
        };
        let (args, file) = b.args("https://music.youtube.com/watch?v=x").unwrap();
        assert_eq!(args[0], "--cookies");
        let text = std::fs::read_to_string(&file.unwrap().0).unwrap();
        assert_eq!(
            text,
            "# Netscape HTTP Cookie File\n\
             .youtube.com\tTRUE\t/\tTRUE\t1893456000\tSID\tSID-v\n\
             music.youtube.com\tFALSE\t/\tFALSE\t0\tpref\tpref-v\n\
             .youtube.com\tTRUE\t/account\tFALSE\t0\tacct\tacct-v\n\
             youtube.com\tFALSE\t/\tFALSE\t0\todd\todd-v\n"
        );
        // A page with no address of its own (none parsed) still gets the jar's.
        assert!(b.args("not a url").unwrap().1.is_some());
    }

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn proxied(cmd: &Command) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| k.to_string_lossy() == "HTTPS_PROXY")
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    /// #363: Options -> Proxy / Socks reaches yt-dlp: the address on the command line, a login
    /// only in the environment, never as an argument; a manual proxy only for its ticked
    /// schemes; "no proxy" and a script's DIRECT: nothing added, so yt-dlp's own default stays.
    #[test]
    fn a_helper_is_given_the_proxy_of_the_settings() {
        use mdm_engine::Proxy;
        let manual = |login: Option<(&str, &str)>, http, https| Proxy::Manual {
            url: "http://127.0.0.1:10808".into(),
            login: login.map(|(u, p)| (u.to_owned(), p.to_owned())),
            http,
            https,
        };
        let youtube = url::Url::parse("https://www.youtube.com/watch?v=x").unwrap();
        assert_eq!(
            proxy_given(&Proxy::None, Some(&youtube)),
            ProxyGiven::Nothing
        );
        assert_eq!(
            proxy_given(&manual(None, true, true), Some(&youtube)),
            ProxyGiven::Argument("http://127.0.0.1:10808".into())
        );
        assert_eq!(
            proxy_given(&manual(None, true, false), Some(&youtube)),
            ProxyGiven::Nothing,
            "https unticked: this https page isn't covered"
        );
        // A login: in the environment, encoded, never an argument.
        assert_eq!(
            proxy_given(&manual(Some(("u@me", "p:ss")), true, true), Some(&youtube)),
            ProxyGiven::Environment("http://u%40me:p%3Ass@127.0.0.1:10808/".into())
        );
        let pac = |script: &str| Proxy::Pac {
            script: std::sync::Arc::from(script),
        };
        assert_eq!(
            proxy_given(
                &pac("function FindProxyForURL(u,h){return 'PROXY 10.0.0.1:3128';}"),
                Some(&youtube)
            ),
            ProxyGiven::Argument("http://10.0.0.1:3128".into())
        );
        assert_eq!(
            proxy_given(
                &pac("function FindProxyForURL(u,h){return 'DIRECT';}"),
                Some(&youtube)
            ),
            ProxyGiven::Nothing
        );
    }

    /// #363, the wiring: the setting reaches a real command line.
    #[test]
    fn yt_dlp_runs_through_the_settings_proxy() {
        use mdm_engine::Proxy;
        let url = "https://www.youtube.com/watch?v=x";
        let exe = std::path::Path::new("yt-dlp");
        set_proxy(Proxy::Manual {
            url: "http://127.0.0.1:10808".into(),
            login: None,
            http: true,
            https: true,
        });
        let plain = args_of(&ytdlp(exe, &[], &["-J".into()], url).unwrap());
        let at = plain.iter().position(|a| a == "--proxy").expect("--proxy");
        assert_eq!(plain[at + 1], "http://127.0.0.1:10808");
        assert!(at < plain.iter().position(|a| a == "--").unwrap());
        set_proxy(Proxy::Manual {
            url: "http://127.0.0.1:10808".into(),
            login: Some(("me".into(), "secret".into())),
            http: true,
            https: true,
        });
        let cmd = ytdlp(exe, &[], &["-J".into()], url).unwrap();
        assert!(!args_of(&cmd).iter().any(|a| a.contains("secret")));
        assert_eq!(
            proxied(&cmd).as_deref(),
            Some("http://me:secret@127.0.0.1:10808/")
        );
        set_proxy(Proxy::None);
        let none = args_of(&ytdlp(exe, &[], &["-J".into()], url).unwrap());
        assert!(!none.iter().any(|a| a == "--proxy"));
    }

    fn yt(chosen: Option<&BrowserCookies>, ours: &[&str]) -> Vec<String> {
        let mut cmd = Command::new("yt-dlp");
        let ours: Vec<String> = ours.iter().map(|s| s.to_string()).collect();
        layers_with(&mut cmd, &[], &ours, chosen).unwrap();
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// #229: Options → Cookies for video sites. Off: nothing added. On: every run without
    /// cookies of its own reads the browser's (a pasted address, a look-up), with yt-dlp's
    /// warnings kept on; a run with the extension's cookies keeps those alone.
    #[test]
    fn the_browsers_cookies_only_when_the_extension_sent_none() {
        assert_eq!(yt(None, &["-J", "--no-warnings"]), ["-J", "--no-warnings"]);
        let firefox = BrowserCookies::new("Firefox", "").unwrap();
        assert_eq!(firefox.spec(), "firefox");
        assert_eq!(
            yt(Some(&firefox), &["-J", "--no-warnings"]),
            ["-J", "--cookies-from-browser", "firefox"]
        );
        assert_eq!(
            yt(Some(&firefox), &["-J", "--cookies", "/tmp/c.txt"]),
            ["-J", "--cookies", "/tmp/c.txt"],
            "the extension's cookies win"
        );
        // A profile by name, or a Chromium fork's profile folder (Helium, Thorium...).
        let named = BrowserCookies::new("chrome", " Profile 1 ").unwrap();
        assert_eq!(named.spec(), "chrome:Profile 1");
        let fork = BrowserCookies::new(
            "chromium",
            r"C:\Users\a\AppData\Local\Helium\User Data\Default",
        )
        .unwrap();
        assert_eq!(
            yt(Some(&fork), &["-f", "b"]),
            [
                "-f",
                "b",
                "--cookies-from-browser",
                r"chromium:C:\Users\a\AppData\Local\Helium\User Data\Default"
            ]
        );
        // Only the browsers yt-dlp reads; Off is no browser.
        assert!(BrowserCookies::new("helium", "").is_none());
        assert!(BrowserCookies::new("", "x").is_none());
        // A profile with a line break never reaches yt-dlp.
        let bad = BrowserCookies::new("firefox", "a\nb").unwrap();
        let mut cmd = Command::new("yt-dlp");
        assert!(layers_with(&mut cmd, &[], &[], Some(&bad)).is_err());
        assert_eq!(
            cookie_browsers().contains(&"safari"),
            cfg!(target_os = "macos")
        );
    }

    /// #360: yt-dlp is told of Node.js or Bun, from the PATH or nvm, and left alone with Deno.
    #[test]
    fn the_javascript_runtime_yt_dlp_is_told_of() {
        let touch = |dir: &Path, name: &str| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(
                dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)),
                b"",
            )
            .unwrap();
        };
        let root = tempfile::tempdir().unwrap();
        let (path, home) = (root.path().join("bin"), root.path().join("home"));
        let dirs = vec![path.clone()];
        std::fs::create_dir_all(&path).unwrap();
        assert_eq!(js_runtime(&dirs, &[], Some(&home)), None);
        for v in ["v20.1.0", "v22.3.0"] {
            touch(&home.join(".nvm/versions/node").join(v).join("bin"), "node");
        }
        let (name, found) = js_runtime(&dirs, &[], Some(&home)).unwrap();
        assert_eq!(name, "node");
        assert!(found.to_string_lossy().contains("v22.3.0"));
        touch(&path, "bun");
        assert_eq!(js_runtime(&dirs, &[], None).unwrap().0, "bun");
        touch(&path, "node");
        assert_eq!(
            js_runtime(&dirs, &[], None).unwrap().1,
            path.join(format!("node{}", std::env::consts::EXE_SUFFIX))
        );
        // #852: Deno from its own installer (`~/.deno/bin`), or one in a folder a runtime
        // installed after MDM started is in (Windows: winget's links), named to yt-dlp; Node.js
        // there when there is nothing else.
        let more = root.path().join("winget-links");
        let exe =
            |dir: &Path, name: &str| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        touch(&more, "node");
        assert_eq!(
            js_runtime(&[], std::slice::from_ref(&more), None),
            Some(("node", exe(&more, "node")))
        );
        touch(&more, "deno");
        assert_eq!(
            js_runtime(&dirs, std::slice::from_ref(&more), None),
            Some(("deno", exe(&more, "deno")))
        );
        touch(&home.join(".deno/bin"), "deno");
        assert_eq!(
            js_runtime(&dirs, &[], Some(&home)),
            Some(("deno", exe(&home.join(".deno/bin"), "deno")))
        );
        // Deno on the PATH: yt-dlp finds it itself.
        touch(&path, "deno");
        assert_eq!(js_runtime(&dirs, &[], Some(&home)), None);
    }

    /// #852: what yt-dlp 2026.10.26 wrote looking a YouTube video up (`-J`, its stderr as is):
    /// with no runtime on the PATH; asking a player that needs one with none there (as when
    /// signed in); and with Node.js.
    #[test]
    fn a_youtube_look_up_without_a_javascript_runtime_is_told_apart() {
        let warnings = |stderr: &str| -> Vec<String> {
            stderr
                .lines()
                .filter_map(|l| l.strip_prefix("WARNING:"))
                .map(|w| w.trim().to_owned())
                .collect()
        };
        let none = warnings(include_str!(
            "../tests/fixtures/ytdlp-js-runtime/no-runtime.stderr"
        ));
        let unsolved = warnings(include_str!(
            "../tests/fixtures/ytdlp-js-runtime/challenge-failed.stderr"
        ));
        let node = warnings(include_str!(
            "../tests/fixtures/ytdlp-js-runtime/with-node.stderr"
        ));
        assert_eq!((none.len(), unsolved.len(), node.len()), (1, 3, 1));
        // Its own words say it, whatever MDM found.
        assert!(missing_js_runtime(&none, false));
        assert!(missing_js_runtime(&none, true));
        // A challenge left unsolved is a runtime's want only when this computer has none: with
        // one, the solver script is what failed, and installing Node.js wouldn't help.
        assert!(missing_js_runtime(&unsolved, false));
        assert!(!missing_js_runtime(&unsolved, true));
        // A look-up that had one says nothing of the kind.
        assert!(!missing_js_runtime(&node, false));
        assert!(!missing_js_runtime(&[], false));
    }

    /// #508: a helper's PATH keeps its folders in their order and leaves out those that can't be
    /// opened (missing, a file); a plain PATH comes through unchanged.
    #[test]
    fn a_helpers_path_leaves_out_the_folders_it_cant_open() {
        let root = tempfile::tempdir().unwrap();
        let (a, b, gone, file) = (
            root.path().join("a"),
            root.path().join("b"),
            root.path().join("gone"),
            root.path().join("file"),
        );
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(&file, b"").unwrap();
        let join = |dirs: &[&PathBuf]| std::env::join_paths(dirs).unwrap();
        // What the helper gets: those, then (on Linux and macOS) the package folders (#659).
        let given = |dirs: &[&PathBuf]| {
            #[allow(unused_mut)]
            let mut all: Vec<PathBuf> = dirs.iter().map(|d| d.to_path_buf()).collect();
            #[cfg(unix)]
            all.extend(
                crate::system::PACKAGE_DIRS
                    .map(PathBuf::from)
                    .into_iter()
                    .filter(|d| d.is_dir()),
            );
            std::env::join_paths(all).unwrap()
        };

        let plain = join(&[&b, &a]);
        assert_eq!(child_path(&plain).unwrap(), given(&[&b, &a]));
        assert_eq!(
            child_path(&join(&[&gone, &b, &file, &a])).unwrap(),
            given(&[&b, &a])
        );

        let mut cmd = Command::new("x");
        cmd.env("PATH", join(&[&a, &gone]));
        with_usable_path(&mut cmd);
        let got: Vec<_> = cmd.get_envs().filter(|(k, _)| *k == "PATH").collect();
        let want = given(&[&a]);
        assert_eq!(
            got,
            [(std::ffi::OsStr::new("PATH"), Some(want.as_os_str()))]
        );
    }

    /// #659: MDM opened from Finder has no Homebrew on its PATH, yet finds yt-dlp there; the
    /// helper gets the package folders after its own, once each, so yt-dlp finds Deno and
    /// ffmpeg where Homebrew put them.
    #[cfg(unix)]
    #[test]
    fn a_helper_gets_the_package_folders_after_its_path() {
        let root = tempfile::tempdir().unwrap();
        let own = root.path().join("own");
        std::fs::create_dir_all(&own).unwrap();
        let usr_bin = PathBuf::from("/usr/bin");

        let dirs = usable_dirs(&std::env::join_paths([&own]).unwrap());
        assert_eq!(dirs[0], own, "the PATH's own folders first");
        assert!(dirs.contains(&usr_bin), "{dirs:?}");
        for d in crate::system::PACKAGE_DIRS.map(PathBuf::from) {
            assert_eq!(dirs.contains(&d), d.is_dir(), "{d:?} in {dirs:?}");
        }

        // Already on the PATH: kept where it was, not added again.
        let dirs = usable_dirs(&std::env::join_paths([&usr_bin, &own]).unwrap());
        assert_eq!(dirs[..2], [usr_bin.clone(), own]);
        assert_eq!(dirs.iter().filter(|d| **d == usr_bin).count(), 1);
    }

    /// #508 as it happened: Windows refuses a junction to a process held to Redirection Guard
    /// ("[WinError 448] ... untrusted mount point"). A stand-in for yt-dlp (PowerShell turning
    /// the guard on for itself, then opening each PATH folder) fails on a PATH through a
    /// junction made here, and passes once started as MDM starts helpers: given the junction's
    /// real folder, in the same place.
    #[cfg(windows)]
    #[test]
    fn a_helper_held_to_redirection_guard_walks_its_path() {
        let root = tempfile::tempdir().unwrap();
        let (real, junction) = (root.path().join("real"), root.path().join("bin"));
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("tool.txt"), b"").unwrap();
        let made = Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&junction)
            .arg(&real)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");

        let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        let path = std::env::join_paths([&system, &junction]).unwrap();
        let usable = child_path(&path).unwrap();
        let mut dirs: Vec<PathBuf> = std::env::split_paths(&usable).collect();
        assert_eq!(dirs.len(), 2);
        let found = dirs.pop().unwrap();
        assert!(
            std::fs::canonicalize(&found).unwrap() == std::fs::canonicalize(&real).unwrap()
                && !found.to_string_lossy().starts_with(r"\\?\")
                && !found.ancestors().any(reparse_point),
            "{found:?}"
        );
        assert_eq!(dirs, std::slice::from_ref(&system));

        let script = root.path().join("standin.ps1");
        std::fs::write(
            &script,
            r#"
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices;
public static class Guard { [DllImport("kernel32.dll", SetLastError=true)]
public static extern bool SetProcessMitigationPolicy(int policy, ref uint flags, UIntPtr size); }'
[uint32]$enforce = 1
if (-not [Guard]::SetProcessMitigationPolicy(16, [ref]$enforce, [UIntPtr]::new([uint32]4))) {
  'no guard here'; exit 0
}
foreach ($d in $env:PATH.Split(';')) {
  if ($d) {
    try { [void][IO.Directory]::GetFiles($d, 'tool.txt') }
    catch { [Console]::Error.WriteLine('ERROR: WinError ' + ($_.Exception.InnerException.HResult -band 0xFFFF) + ' ' + $d); exit 1 }
  }
}
'walked'
"#,
        )
        .unwrap();
        let standin = || {
            let mut cmd = Command::new(system.join(r"WindowsPowerShell\v1.0\powershell.exe"));
            cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&script)
                .env("PATH", &path);
            cmd
        };
        let raw = standin().output().unwrap();
        let said = String::from_utf8_lossy(&raw.stdout).trim().to_owned();
        if said == "no guard here" {
            eprintln!("this Windows has no Redirection Guard: the stand-in can't be held to it");
            return;
        }
        if raw.status.success() && said == "walked" {
            // A junction an administrator made is trusted (CI runs as one): nothing to refuse.
            eprintln!(
                "this junction is trusted here (made as an administrator): nothing to refuse"
            );
            return;
        }
        let refused = String::from_utf8_lossy(&raw.stderr);
        assert!(
            !raw.status.success() && refused.contains("WinError 448"),
            "the junction is refused as it was on #508: {said} {refused}"
        );
        let out = output(standin(), "stand-in", Duration::from_secs(60)).unwrap();
        assert_eq!(String::from_utf8_lossy(&out).trim(), "walked");
    }

    /// #352: the second look-up has no cookies of any kind, and everything else the first had.
    #[test]
    fn a_look_up_without_cookies_keeps_the_rest() {
        let ours: Vec<String> = [
            "-J",
            "--cookies",
            "/tmp/c.txt",
            "--referer",
            "https://p.test/",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            without_cookies(&ours),
            ["-J", "--referer", "https://p.test/", NO_COOKIES]
        );
        let firefox = BrowserCookies::new("firefox", "").unwrap();
        assert!(with_cookies(&ours, None));
        assert!(with_cookies(&["-J".to_owned()], Some(&firefox)));
        assert!(!with_cookies(&["-J".to_owned()], None));
        // Told so, the chosen browser's cookies aren't read either.
        assert_eq!(yt(Some(&firefox), &["-J", NO_COOKIES]), ["-J", NO_COOKIES]);
    }

    /// #229: why the browser's cookies didn't help stays on the failure's message.
    #[test]
    fn the_cookie_trouble_is_kept_on_the_failure() {
        let tail: VecDeque<String> = [
            "[youtube] Extracting URL: https://www.youtube.com/watch?v=x",
            "Extracted 3 cookies from chrome (41 could not be decrypted)",
            "ERROR: [youtube] x: Sign in to confirm you're not a bot.",
        ]
        .map(String::from)
        .into();
        let warnings = vec!["unknown cookie version: \"b'v20'\"".to_owned()];
        assert_eq!(
            with_cookie_trouble(
                "[youtube] x: Sign in to confirm you're not a bot.".into(),
                &warnings,
                &tail
            ),
            "[youtube] x: Sign in to confirm you're not a bot. (unknown cookie version: \"b'v20'\")"
        );
        let copy: VecDeque<String> = [
            "ERROR: Could not copy Chrome cookie database. See  https://github.com/yt-dlp/yt-dlp/issues/7271  for more info",
            "ERROR: failed to load cookies",
        ]
        .map(String::from)
        .into();
        assert!(
            with_cookie_trouble("failed to load cookies".into(), &[], &copy)
                .contains("Could not copy Chrome cookie database")
        );
        // Nothing about cookies: the message as it was.
        assert_eq!(
            with_cookie_trouble(
                "Video unavailable".into(),
                &["a warning".into()],
                &VecDeque::new()
            ),
            "Video unavailable"
        );
    }

    #[test]
    fn hostile_text_never_reaches_the_argument_list() {
        let exe = Path::new("yt-dlp");
        assert!(ytdlp(exe, &[], &[], "https://x/\n--exec calc").is_err());
        assert!(ytdlp(exe, &["--format\0best".into()], &[], "https://x/").is_err());
    }

    #[test]
    fn fixed_arguments_come_last_and_the_url_after_the_separator() {
        let cmd = ytdlp(
            Path::new("yt-dlp"),
            &["--user".into()],
            &["-J".into()],
            "-https://x/",
        )
        .unwrap();
        let mut args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // This computer's Node.js or Bun, if it has one (#360); tested on its own below.
        if let Some(i) = args.iter().position(|a| a == "--js-runtimes") {
            args.drain(i..=i + 1);
        }
        assert_eq!(
            args,
            [
                "--user",
                "-J",
                "--ignore-config",
                "--no-exec",
                "--no-update",
                "--encoding",
                "utf-8",
                "--",
                "-https://x/"
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn output_and_failures_of_a_real_process() {
        let mut ok = Command::new("cmd");
        ok.args(["/C", "echo hello"]);
        let out = output(ok, "cmd", Duration::from_secs(10)).unwrap();
        assert_eq!(String::from_utf8_lossy(&out).trim(), "hello");
        let mut bad = Command::new("cmd");
        bad.args(["/C", "echo ERROR: no such video 1>&2 & exit 1"]);
        let err = output(bad, "cmd", Duration::from_secs(10)).unwrap_err();
        assert!(err.to_string().contains("no such video"), "{err}");
        let mut slow = Command::new("cmd");
        slow.args(["/C", "ping -n 30 127.0.0.1 >nul"]);
        let err = output(slow, "cmd", Duration::from_millis(500)).unwrap_err();
        assert!(matches!(err, ProcError::Timeout(_)), "{err}");
    }

    /// #834: a helper (here a stand-in that prints where it runs and its temporary folders)
    /// starts in the helpers' folder, not in MDM's own working folder, and has it as TEMP, TMP
    /// and TMPDIR.
    #[test]
    fn a_helper_runs_in_the_helpers_folder() {
        let dir = crate::workdir::folders()
            .dir
            .clone()
            .expect("a temporary folder that can be written");
        #[cfg(windows)]
        let cmd = {
            let mut c = Command::new("cmd");
            c.args(["/C", "cd& echo %TEMP%& echo %TMP%& echo %TMPDIR%"]);
            c
        };
        #[cfg(unix)]
        let cmd = {
            let mut c = Command::new("/bin/sh");
            c.args(["-c", r#"pwd -P; echo "$TEMP"; echo "$TMP"; echo "$TMPDIR""#]);
            c
        };
        let out = output(cmd, "stand-in", Duration::from_secs(10)).unwrap();
        let out = String::from_utf8_lossy(&out);
        let lines: Vec<&str> = out.lines().map(str::trim).collect();
        assert_eq!(lines.len(), 4, "{out}");
        let real = |p: &Path| std::fs::canonicalize(p).unwrap();
        assert_eq!(real(Path::new(lines[0])), real(&dir), "{out}");
        assert_ne!(
            real(Path::new(lines[0])),
            real(&std::env::current_dir().unwrap())
        );
        for temp in &lines[1..] {
            assert_eq!(Path::new(temp), dir, "{out}");
        }
    }

    /// #61: killing a helper ends what it started too (yt-dlp.exe is a launcher whose child
    /// downloads). Here a script starts a child that writes a line every second: once killed,
    /// the file stops growing.
    #[cfg(windows)]
    #[test]
    fn killing_a_helper_ends_its_children() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker.txt");
        let child = dir.path().join("child.cmd");
        let parent = dir.path().join("parent.cmd");
        std::fs::write(
            &child,
            format!(
                "@for /l %%i in (1,1,60) do @(echo x>>\"{}\" & ping -n 2 127.0.0.1 >nul)\r\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::write(
            &parent,
            format!(
                "@start \"\" /b cmd /c \"{}\"\r\n@ping -n 60 127.0.0.1 >nul\r\n",
                child.display()
            ),
        )
        .unwrap();
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(&parent);
        let mut s = spawn(cmd, "cmd").unwrap();
        let size = || std::fs::metadata(&marker).map_or(0, |m| m.len());
        std::thread::sleep(Duration::from_secs(3));
        assert!(size() > 0, "the child runs");
        s.kill();
        std::thread::sleep(Duration::from_millis(1500));
        let after_kill = size();
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(size(), after_kill, "the child ended with the helper");
    }
}
