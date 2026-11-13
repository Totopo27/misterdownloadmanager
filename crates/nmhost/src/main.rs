//! Mister Download Manager's browser native messaging host (Chrome, Edge, Brave, Firefox).
//!
//! - Started by the browser: reads native messages (u32 LE length + JSON) from stdin, hands each
//!   download to the running Mister Download Manager over mdm-ipc (starting Mister Download Manager if needed), answers
//!   `{"ok": …}`. A download Options says to leave to the browser (file type, excluded site or
//!   address, browser, integration off: `mdm_ipc::capture`) is answered `{"ok": false}` without
//!   starting anything, and the extension lets the browser have it. `{"rules": true}` asks for
//!   the file types to offer: `{"ok": true, "types": "EXE ZIP …"}`.
//! - The download button over videos: `{"panel": true, "url": page}` asks whether it may show on
//!   that page (`{"ok": …}`, Options' rules); `{"video": true, "url": page}` opens the Download
//!   video window for it (`src`: the video's own file, or its stream's manifest; with
//!   `explicit`, the toolbar button's list of the tab's media asked, wherever the button shows);
//!   `{"hideSite": host}` keeps it off that site from now on;
//!   `{"openOptions": true}` opens Options where it is set up.
//! - The button's quality menu (announced by `"videoLookup": true` in the answer to `hello` and
//!   `rules`): `{"videoLookup": {page, src, title, referrer, storeId}}` has the running app look
//!   the video up, `{"videoPoll": {token}}` asks how it goes, `{"videoPick": {token, choice,
//!   storeId}}` downloads the choice made. Each is answered `{"ok": true, "lookup": {state, …}}`
//!   (`mdm_ipc::api::VideoLookupAnswer`), never with a `why`; none of them starts the app
//!   (`use-window`: the button's click then opens the Download video window as before). A
//!   question waits here while the look-up runs, up to 1.5 s ([`POLL_WAIT`]).
//! - `misterdownloadmanager-nmhost --register` / `--unregister`: writes / removes the host
//!   manifests (Chromium's names the extension's origin, Firefox's its add-on ID). Windows: beside
//!   this exe, with the per-user (HKCU) registry entries the browsers look up; with `--all-users`
//!   (the installer, installing for everyone, elevated) under HKLM instead. Linux and macOS: in
//!   each installed browser's own host folder (the app does it at every start, as there is no
//!   installer step), or the system-wide ones with `--all-users`. A browser that reads a
//!   system-wide manifest naming this same program (the .deb's or .rpm's) gets none in the home
//!   folder: the package manager removes the package's with it (#266).
//! - `misterdownloadmanager-nmhost --safari` (macOS): the same host for Safari, started by the
//!   app's Safari extension (tools/safari), which passes each message on; nothing to register.
//! - `misterdownloadmanager-nmhost --package-manifests <root>` (Linux, the .deb and .rpm build):
//!   writes the system-wide manifests under `<root>`, naming the packaged helper
//!   (`/usr/bin/misterdownloadmanager-nmhost`). Nothing else.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod win32;

const HOST_NAME: &str = "com.misterdownloadmanager.host";
use mdm_ipc::extension::{FIREFOX_ID, STORE_ID, chromium_ids, store_id};
/// Where each Chromium browser looks for native messaging hosts on Windows, under HKCU (one
/// user) or HKLM (everyone). Chromium-branded builds (Chromium, Helium, Thorium, and Brave and
/// Brave Origin with every channel) read Chromium's first, then Chrome's, as most other forks do
/// (Chromium's launch_context_win.cc). Brave's own key is kept for builds that read it.
#[cfg(windows)]
const REGISTRY_KEYS: [&str; 4] = [
    r"Software\Google\Chrome\NativeMessagingHosts",
    r"Software\Chromium\NativeMessagingHosts",
    r"Software\Microsoft\Edge\NativeMessagingHosts",
    r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
];
/// Where Firefox looks.
#[cfg(windows)]
const FIREFOX_KEY: &str = r"Software\Mozilla\NativeMessagingHosts";
/// Chrome's external extensions: an entry naming the store's extension makes Chrome offer it at
/// its next start ("New extension added"), the user turning it on there. HKLM's is read from the
/// 32-bit view on 64-bit Windows, hence /reg:32 (HKCU has one view). Helium reads it too, but
/// asks Google's address without the browser's version, and Google answers "no update": nothing
/// is offered there (#685), the app's window opens the store's page in Helium instead.
#[cfg(windows)]
const CHROME_EXTERNAL_KEY: &str = r"Software\Google\Chrome\Extensions";

/// A message from the extension.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
struct Message {
    /// Only asking for the capture rules (the file types to offer). `hello` (the extension
    /// installed, or the browser started) asks the same and tells the app it is there.
    rules: bool,
    hello: bool,
    /// The extension's version.
    v: Option<String>,
    url: String,
    referrer: Option<String>,
    cookies: Option<String>,
    /// The same cookies with their domains and flags (`mdm_ipc::Command::cookie_jar`); an older
    /// extension sends none.
    cookie_jar: Vec<mdm_ipc::BrowserCookie>,
    user_agent: Option<String>,
    /// `navigator.userAgentData` brands: Brave and Opera name themselves there, not in the
    /// User-Agent; Firefox's forks add the name `runtime.getBrowserInfo` gives ("Zen").
    brands: Vec<String>,
    filename: Option<String>,
    /// Where the browser's redirects from `url` ended.
    final_url: Option<String>,
    /// The browser's own download: its size (bytes, when known) and type.
    size: Option<u64>,
    mime: Option<String>,
    /// A form's download: the body to post again, and its Content-Type.
    post: Option<String>,
    post_type: Option<String>,
    /// From a private window.
    private: bool,
    /// The browser's proxy for `url` (#42): its address, or its auto-config script, and whether it
    /// asked for a login (see mdm_ipc::Command).
    proxy: Option<String>,
    proxy_pac: Option<String>,
    proxy_login: bool,
    /// The user asked for this download (right-click → Download with Mister Download Manager):
    /// Options' automatic-capture rules don't apply, as IDM's "Download with IDM" takes any link.
    explicit: bool,
    /// May the download button show over videos on page `url`? With `referrer`: the tab's
    /// address, when `url` is a frame embedded from another page (its site counts too, #532).
    panel: bool,
    /// The download button was clicked: `url` is the video's page, `src` the video's own
    /// address when the page plays a plain file (not a stream assembled in the page), or the
    /// manifest of the stream its player loaded. With `explicit`, the toolbar button's list of
    /// the tab's media asked for it (#38).
    video: bool,
    src: Option<String>,
    /// The page's title (`video`).
    title: Option<String>,
    /// The button over a plain file (`src`, #703): it is downloaded as any file, at once.
    direct: bool,
    /// The button's "Show": the main window with the newest download of this address selected.
    show_download: Option<String>,
    /// The button's "Turn on": show it on this site (a host name) again.
    show_site: Option<String>,
    /// The button's menu: keep it off this site (a host name).
    hide_site: Option<String>,
    /// The button's menu: open its settings in Options (`page`: the toolbar button's popup asks
    /// for `general`).
    open_options: bool,
    page: Option<String>,
    /// The right-click menu: these links of a page or its selection.
    links: Vec<String>,
    /// The toolbar button: leave this site (a host name) to the browser.
    exclude_site: Option<String>,
    /// The toolbar button: bring Mister Download Manager's window up.
    open: bool,
    /// The extension left a download to the browser by Options' rules (it has them from
    /// `rules`) without asking about it: for MDM's log. Under its own key, with no `url`: an
    /// older host takes it for an address it can't download, and hands nothing over.
    declined: Option<Notice>,
    /// The video button's quality menu: look this video up (with the message's `cookies`,
    /// `cookieJar`, `userAgent` and `private`), ask how it goes, download the choice made. Under
    /// their own keys, with no `url`: an older host refuses them without effect.
    video_lookup: Option<LookupAsk>,
    video_poll: Option<PollAsk>,
    video_pick: Option<PickAsk>,
    /// With `video` and `videoLookup`: what the browser sent with `src` as its page's player
    /// asked for it (#900), sent again with it and each of its pieces. An older extension sends
    /// none: the page is the stream's Referer then, as before.
    fetched_with: FetchedWith,
}

/// The headers the browser sent with a stream (`Message::fetched_with`): its Referer and Origin.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct FetchedWith {
    referer: Option<String>,
    origin: Option<String>,
}

impl FetchedWith {
    /// The Referer for `src` (some) fetched from `page`: the one the browser sent, else the page.
    /// None without `src` (the page itself is looked up).
    fn referer(&self, src: Option<&str>, page: &str) -> Option<String> {
        src?;
        Some(
            web_header(self.referer.as_deref())
                .unwrap_or(page)
                .to_owned(),
        )
    }

    /// The Origin the browser sent with `src`, if any (only with `src`).
    fn origin(&self, src: Option<&str>) -> Option<String> {
        src?;
        web_header(self.origin.as_deref()).map(str::to_owned)
    }
}

/// `v` when it is a web address fit for a header (http or https, no space or control character,
/// at most 8 KB): what the extension takes from the browser's own request.
fn web_header(v: Option<&str>) -> Option<&str> {
    v.map(str::trim).filter(|v| {
        (v.starts_with("http://") || v.starts_with("https://"))
            && v.len() <= 8192
            && !v.chars().any(|c| c.is_whitespace() || c.is_control())
    })
}

/// The quality menu's look-up: the video's page (as the button's click sends it), the video's
/// own file or its stream's manifest when the page plays one (`src`), the page's title, the
/// page's referrer, and the cookie store the tab is in (Firefox's containers).
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
struct LookupAsk {
    page: String,
    src: Option<String>,
    title: Option<String>,
    referrer: Option<String>,
    store_id: Option<String>,
}

/// The quality menu: how is the look-up of `token` going?
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct PollAsk {
    token: String,
}

/// The quality menu: the choice at `choice` of look-up `token`, made in a tab of cookie store
/// `store_id` (the message's `private` says whether in a private window).
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
struct PickAsk {
    token: String,
    choice: usize,
    store_id: Option<String>,
}

/// A download the extension left to the browser: why (a `Declined` code, or `site-access`:
/// the browser doesn't let it read the site), its site and file name.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct Notice {
    why: String,
    site: String,
    name: String,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Why it failed, for the extension to say and to link the help page's section
    /// (docs/browser-help.md): `not-running`, `too-large`, `refused`, `options`.
    #[serde(skip_serializing_if = "Option::is_none")]
    why: Option<&'static str>,
    /// Mister Download Manager's version (this host comes with it).
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'static str>,
    /// For `rules`: Options → File types.
    #[serde(skip_serializing_if = "Option::is_none")]
    types: Option<String>,
    /// For `rules`: whether this browser's downloads are taken at all (integration on, the
    /// browser captured), and the sites and addresses left to the browser; the extension leaves
    /// those alone without asking (the host checks again at each hand-off).
    #[serde(skip_serializing_if = "Option::is_none")]
    capture: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    excluded_sites: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    excluded_addresses: Option<Vec<String>>,
    /// For `rules`: also take listed types the browser would open itself (a PDF, an MP3).
    #[serde(skip_serializing_if = "Option::is_none")]
    opened: Option<bool>,
    /// For `rules`: files smaller than this (KB) stay with the browser.
    #[serde(skip_serializing_if = "Option::is_none")]
    min_kb: Option<u64>,
    /// For `rules` when `capture` is false: why (`integration-off`, `browser`).
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_off: Option<&'static str>,
    /// A download left to the browser by Options: which rule (a `Declined` code), for the
    /// extension to say (its popup).
    #[serde(skip_serializing_if = "Option::is_none")]
    declined: Option<&'static str>,
    /// For `hello` and `rules`: this host and its app have the button's quality menu (they are
    /// installed together); without it the extension never asks for one.
    #[serde(skip_serializing_if = "Option::is_none")]
    video_lookup: Option<bool>,
    /// The quality menu's answer: where the look-up is, or what became of the choice. Its
    /// states are never a `why`: they say nothing of whether MDM is reachable.
    #[serde(skip_serializing_if = "Option::is_none")]
    lookup: Option<mdm_ipc::api::VideoLookupAnswer>,
}

impl Reply {
    fn done() -> Reply {
        Reply {
            ok: true,
            ..Default::default()
        }
    }

    /// The quality menu's answer.
    fn lookup(answer: mdm_ipc::api::VideoLookupAnswer) -> Reply {
        Reply {
            ok: true,
            lookup: Some(answer),
            ..Default::default()
        }
    }

    fn failed(why: &'static str, error: impl Into<String>) -> Reply {
        Reply {
            why: Some(why),
            error: Some(error.into()),
            ..Default::default()
        }
    }
}

/// The name a download will have: the browser's, else the address's last part.
fn file_name_of(m: &Message) -> String {
    let from_browser = m
        .filename
        .as_deref()
        .and_then(|f| f.rsplit(['\\', '/']).next())
        .filter(|f| !f.is_empty());
    match from_browser {
        Some(f) => f.to_owned(),
        None => m
            .url
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_owned(),
    }
}

/// The browser `m` comes from, by the names Options lists.
fn browser(m: &Message) -> &'static str {
    mdm_ipc::capture::browser_named(m.user_agent.as_deref().unwrap_or_default(), &m.brands)
}

/// The answer to one message, under `rules` (the app's saved Options).
fn answer(m: Message, rules: &mdm_ipc::capture::CaptureRules) -> Reply {
    let browser = browser(&m);
    if m.rules || m.hello {
        return Reply {
            ok: true,
            version: Some(env!("CARGO_PKG_VERSION")),
            types: Some(rules.auto_types.clone()),
            capture: Some(rules.captures(browser)),
            excluded_sites: Some(rules.excluded_sites.clone()),
            excluded_addresses: Some(rules.excluded_addresses.clone()),
            opened: Some(rules.capture_opened),
            min_kb: Some(rules.capture_min_kb),
            capture_off: rules.capture_off(browser).map(|d| d.code()),
            video_lookup: Some(true),
            ..Default::default()
        };
    }
    if let Some(n) = &m.declined {
        log_left(browser, &n.site, &n.name, notice_reason(&n.why), m.private);
        return Reply::done();
    }
    // The video button's quality menu: asked of a running app only, answered in `lookup`.
    if let Some(ask) = menu_ask(&m) {
        return quality_menu(ask, &m, rules, browser);
    }
    // The video button: asked about, clicked, its menu. What it sends goes to the app (started
    // if need be), which opens its window or keeps the setting.
    if m.panel {
        return Reply {
            ok: rules.panel_on(&m.url, m.referrer.as_deref(), browser),
            ..Default::default()
        };
    }
    let hide = m
        .hide_site
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty());
    let show_site = m
        .show_site
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty());
    let show_download = m
        .show_download
        .as_deref()
        .map(str::trim)
        .filter(|a| a.starts_with("http://") || a.starts_with("https://"));
    let panel_cmd = if let Some(host) = hide {
        Some(mdm_ipc::Command {
            panel_hide_site: Some(host.to_owned()),
            ..Default::default()
        })
    } else if let Some(host) = show_site {
        Some(mdm_ipc::Command {
            panel_show_site: Some(host.to_owned()),
            ..Default::default()
        })
    } else if let Some(address) = show_download {
        Some(mdm_ipc::Command {
            show_download: Some(address.to_owned()),
            ..Default::default()
        })
    } else if m.open_options {
        let page = match m.page.as_deref() {
            Some("general") => "options:general",
            _ => "options:panels",
        };
        Some(mdm_ipc::Command {
            show: Some(page.into()),
            ..Default::default()
        })
    } else if m.open {
        // An empty command: the running app comes to the front (a second launch's).
        Some(mdm_ipc::Command::default())
    } else if let Some(host) = m
        .exclude_site
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
    {
        Some(mdm_ipc::Command {
            exclude_site: Some(host.to_owned()),
            ..Default::default()
        })
    } else if !m.links.is_empty() {
        let links: Vec<String> = m
            .links
            .iter()
            .filter(|l| downloadable(l))
            .cloned()
            .collect();
        if links.is_empty() {
            return Reply::failed("refused", "no link MDM downloads");
        }
        Some(mdm_ipc::Command {
            links,
            referer: m.referrer.clone().filter(|r| !r.is_empty()),
            cookies: m.cookies.clone().filter(|c| !c.is_empty()),
            user_agent: m.user_agent.clone(),
            private: m.private,
            ..Default::default()
        })
    } else if m.video {
        match video_command(&m, rules, browser) {
            Ok(cmd) => Some(cmd),
            Err((why, error)) => return Reply::failed(why, error),
        }
    } else {
        None
    };
    if let Some(cmd) = panel_cmd {
        return delivered(&cmd);
    }
    if !downloadable(&m.url) {
        return Reply::failed("refused", "not an http(s) or ftp(s) URL");
    }
    if let Err(why) = verdict(&m, rules) {
        let site = mdm_ipc::capture::host_of(&m.url);
        log_left(browser, &site, &file_name_of(&m), why.reason(), m.private);
        return Reply {
            declined: Some(why.code()),
            ..Reply::failed(
                "options",
                format!("left to the browser by Options ({why:?})"),
            )
        };
    }
    delivered(&to_command(m))
}

/// Why the extension left a download alone, from its notice's code.
fn notice_reason(code: &str) -> String {
    match mdm_ipc::capture::Declined::from_code(code) {
        Some(d) => d.reason().to_owned(),
        None if code == "site-access" => {
            "the browser doesn't let the extension read this site (its site access is limited)"
                .to_owned()
        }
        None => format!("{:?}", clean(code, 32)),
    }
}

/// `s` for one line of the log: no line breaks or other control characters, at most `max`
/// characters.
fn clean(s: &str, max: usize) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect()
}

/// The log line for a download left to the browser. A private window's site and file name are
/// left out: the log outlives the window.
fn left_line(browser: &str, site: &str, name: &str, reason: &str, private: bool) -> String {
    let browser = if browser.is_empty() {
        "a browser"
    } else {
        browser
    };
    let what = if private {
        "a download from a private window".to_owned()
    } else {
        match (clean(name, 200), clean(site, 200)) {
            (n, s) if n.trim().is_empty() && s.trim().is_empty() => "a download".to_owned(),
            (n, s) if s.trim().is_empty() => n,
            (n, s) if n.trim().is_empty() => format!("a download from {s}"),
            (n, s) => format!("{n} from {s}"),
        }
    };
    format!("browser integration: {browser} keeps {what}: {reason}")
}

fn log_left(browser: &str, site: &str, name: &str, reason: impl AsRef<str>, private: bool) {
    log_line(&left_line(browser, site, name, reason.as_ref(), private));
}

/// A line in MDM's log (`<data>/logs/mdm.log`, crates/crash/src/logs.rs), dated as the app's
/// own lines: the host runs apart from the app, which may not be running at all. Appended
/// whole, so it never splits a line the app writes at the same time.
fn log_line(message: &str) {
    // Unit tests run without a data folder of their own: never into the real MDM's log
    // (tests/host.rs runs the program with its own and reads what it wrote).
    if cfg!(test) {
        return;
    }
    let Some(dir) = mdm_ipc::data_dir().map(|d| d.join("logs")) else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let line = format!(
        "{}  INFO nmhost mdm_nmhost: {message}\n",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ")
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("mdm.log"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// The Download video window's command for a video message: the page, or the video's own file
/// or its stream's manifest (`src`) fetched as from its page; or why it is refused (the reply's
/// `why` and `error`).
fn video_command(
    m: &Message,
    rules: &mdm_ipc::capture::CaptureRules,
    browser: &str,
) -> Result<mdm_ipc::Command, (&'static str, &'static str)> {
    if !(m.url.starts_with("http://") || m.url.starts_with("https://")) {
        return Err(("refused", "not an http(s) page"));
    }
    // The toolbar button's list of the tab's media (#38) asks for one explicitly: whether the
    // button over videos shows on this site doesn't matter then.
    if !m.explicit && !rules.panel_on(&m.url, m.referrer.as_deref(), browser) {
        return Err(("options", "the video button is off here"));
    }
    // The video clicked, not the page's first one (a feed plays many): its own file when there
    // is one, fetched as from its page.
    let src = m
        .src
        .clone()
        .filter(|s| s.starts_with("http://") || s.starts_with("https://"));
    // A stream or file of the page's, fetched as the browser fetched it (#900): with the Referer
    // and Origin its player sent, else from the page.
    let referer = match &src {
        Some(s) => m.fetched_with.referer(Some(s), &m.url),
        None => m.referrer.clone().filter(|r| !r.is_empty()),
    };
    let origin = m.fetched_with.origin(src.as_deref());
    // The button over a plain file (#703): that file, as a file.
    let direct = m.direct && src.is_some();
    Ok(mdm_ipc::Command {
        video_direct: direct,
        // The type the plain file's server said when the tab loaded it (#800).
        mime: m.mime.clone().filter(|t| direct && !t.trim().is_empty()),
        video_page: Some(src.unwrap_or_else(|| m.url.clone())),
        video_title: m.title.clone().filter(|t| !t.trim().is_empty()),
        referer,
        origin,
        cookies: m.cookies.clone().filter(|c| !c.is_empty()),
        cookie_jar: m.cookie_jar.clone(),
        user_agent: m.user_agent.clone(),
        private: m.private,
        ..Default::default()
    })
}

/// Which of the quality menu's messages: what an answer that isn't the app's means depends on
/// it ([`menu_answer`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAsk {
    Lookup,
    Poll,
    Pick,
}

/// The quality menu's message `m` is, if it is one.
fn menu_ask(m: &Message) -> Option<MenuAsk> {
    if m.video_lookup.is_some() {
        Some(MenuAsk::Lookup)
    } else if m.video_poll.is_some() {
        Some(MenuAsk::Poll)
    } else if m.video_pick.is_some() {
        Some(MenuAsk::Pick)
    } else {
        None
    }
}

/// The longest page title passed on (the extension cuts it there too).
const TITLE_MAX: usize = 300;
/// The longest look-up token taken (the app's are 64 hex digits).
const TOKEN_MAX: usize = 128;

/// The app's command for one of the quality menu's messages, or, without asking the app, its
/// answer's state: `use-window` for a page not on the web (or a choice without a token), `off`
/// where the button is off (Options' rules, as `panel` asks), `gone` for a question without a
/// token. Each command says `startup` too: an older app, which knows none of them, takes it for
/// its startup entry's and opens nothing (a bare command would bring its window up).
fn menu_command(
    ask: MenuAsk,
    m: &Message,
    rules: &mdm_ipc::capture::CaptureRules,
    browser: &str,
) -> Result<mdm_ipc::Command, &'static str> {
    use mdm_ipc::api::lookup;
    let token =
        |t: &str| Some(t.trim().to_owned()).filter(|t| !t.is_empty() && t.len() <= TOKEN_MAX);
    let store = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
    let cmd = match ask {
        MenuAsk::Lookup => {
            let l = m.video_lookup.as_ref().ok_or(lookup::USE_WINDOW)?;
            let web = |s: &str| s.starts_with("http://") || s.starts_with("https://");
            if !web(&l.page) {
                return Err(lookup::USE_WINDOW);
            }
            if !rules.panel_on(&l.page, l.referrer.as_deref(), browser) {
                return Err(lookup::OFF);
            }
            // As the button's click (`video_command`): the video clicked, its own file when
            // there is one, fetched as from its page, with the page's title (a video yt-dlp
            // knows only by its address is named by it, #345).
            let src = l.src.clone().filter(|s| web(s));
            let referer = match &src {
                Some(s) => m.fetched_with.referer(Some(s), &l.page),
                None => l.referrer.clone().filter(|r| !r.is_empty()),
            };
            let origin = m.fetched_with.origin(src.as_deref());
            let title = l
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| t.chars().take(TITLE_MAX).collect());
            mdm_ipc::Command {
                video_lookup: Some(mdm_ipc::VideoLookup {
                    address: src.unwrap_or_else(|| l.page.clone()),
                    title,
                    store_id: store(&l.store_id),
                }),
                referer,
                origin,
                cookies: m.cookies.clone().filter(|c| !c.is_empty()),
                cookie_jar: m.cookie_jar.clone(),
                user_agent: m.user_agent.clone(),
                private: m.private,
                ..Default::default()
            }
        }
        MenuAsk::Poll => {
            let p = m.video_poll.as_ref().ok_or(lookup::GONE)?;
            mdm_ipc::Command {
                video_poll: Some(token(&p.token).ok_or(lookup::GONE)?),
                ..Default::default()
            }
        }
        MenuAsk::Pick => {
            let p = m.video_pick.as_ref().ok_or(lookup::USE_WINDOW)?;
            mdm_ipc::Command {
                video_pick: Some(mdm_ipc::VideoPick {
                    token: token(&p.token).ok_or(lookup::USE_WINDOW)?,
                    choice: p.choice,
                    store_id: store(&p.store_id),
                }),
                private: m.private,
                ..Default::default()
            }
        }
    };
    Ok(mdm_ipc::Command {
        startup: true,
        ..cmd
    })
}

/// One of the quality menu's messages, asked of the running app (never started for it: a menu
/// over a video isn't worth starting MDM unseen; the button's click does that).
fn quality_menu(
    ask: MenuAsk,
    m: &Message,
    rules: &mdm_ipc::capture::CaptureRules,
    browser: &str,
) -> Reply {
    let cmd = match menu_command(ask, m, rules, browser) {
        Ok(cmd) => cmd,
        Err(state) => return Reply::lookup(mdm_ipc::api::VideoLookupAnswer::of(state)),
    };
    // The choice brings its download's progress window up: the app may bring it to the front
    // (Windows only, as in `deliver`: Linux and macOS have no foreground lock for a host to
    // lift). A look-up and the questions about it open nothing.
    #[cfg(windows)]
    if ask == MenuAsk::Pick
        && let Some(pid) = mdm_ipc::app_pid()
    {
        win32::allow_foreground(pid);
    }
    let token = cmd.video_poll.clone().unwrap_or_default();
    let ask_app = || menu_answer(ask, mdm_ipc::request_checked(&cmd), &token);
    let answer = match ask {
        MenuAsk::Poll => until_settled(ask_app, POLL_WAIT, POLL_EVERY),
        MenuAsk::Lookup | MenuAsk::Pick => ask_app(),
    };
    if let Some(l) = &cmd.video_lookup {
        log_line(&lookup_line(browser, &l.address, m.private, &answer.state));
    }
    Reply::lookup(answer)
}

/// How long a question about a look-up waits here while it runs, asking the app again every
/// [`POLL_EVERY`]: the browser then asks about once every 1.5 s while yt-dlp works (a host
/// started for each question), and hears of the answer within a tenth of a second of it.
const POLL_WAIT: Duration = Duration::from_millis(1500);
const POLL_EVERY: Duration = Duration::from_millis(100);

/// `ask`'s answer once it is no longer `pending`, asked again every `every`, for `wait` at most
/// (then `pending` as it is).
fn until_settled(
    ask: impl FnMut() -> mdm_ipc::api::VideoLookupAnswer,
    wait: Duration,
    every: Duration,
) -> mdm_ipc::api::VideoLookupAnswer {
    let start = std::time::Instant::now();
    until_settled_on(ask, wait, every, || start.elapsed(), std::thread::sleep)
}

/// [`until_settled`] on any clock: `elapsed` is the time since the first question, `sleep`
/// waits. The tests run it on a clock of their own, so how often it asks doesn't depend on how
/// busy the machine is (a loaded CI runner oversleeps and asks less often).
fn until_settled_on(
    mut ask: impl FnMut() -> mdm_ipc::api::VideoLookupAnswer,
    wait: Duration,
    every: Duration,
    elapsed: impl Fn() -> Duration,
    mut sleep: impl FnMut(Duration),
) -> mdm_ipc::api::VideoLookupAnswer {
    loop {
        let answer = ask();
        if answer.state != mdm_ipc::api::lookup::PENDING || elapsed() + every > wait {
            return answer;
        }
        sleep(every);
    }
}

/// The app's answer to the quality menu's `ask` (`token`: the look-up a question is about), or
/// what its absence means. No app running, no answer, a refusal (`denied`, `gone`,
/// `too-large`), or `ok` (an older app, which knows no quality menu): a look-up or a choice is
/// left to the Download video window (`use-window`). `timeout` is different: the app's port
/// says it only after handing the command over (`mdm_ipc`'s server queues it, then waits for
/// the answer), so the app has it and will act on it. A choice is then `sent` (downloading
/// soon; the Download video window would download it twice), a look-up `busy` (its yt-dlp is
/// running; another window would start another), and a question `pending` again. Anything
/// else a question gets is `gone`.
fn menu_answer(
    ask: MenuAsk,
    got: Result<String, mdm_ipc::SendError>,
    token: &str,
) -> mdm_ipc::api::VideoLookupAnswer {
    use mdm_ipc::api::{VideoLookupAnswer, lookup};
    if let Ok(line) = &got
        && let Ok(answer) = serde_json::from_str::<VideoLookupAnswer>(line)
        && !answer.state.is_empty()
    {
        return answer;
    }
    let timeout = matches!(&got, Ok(l) if l.trim() == "timeout");
    match ask {
        MenuAsk::Poll if timeout => VideoLookupAnswer::pending(token),
        MenuAsk::Poll => VideoLookupAnswer::of(lookup::GONE),
        MenuAsk::Pick if timeout => VideoLookupAnswer::of(lookup::SENT),
        MenuAsk::Lookup if timeout => VideoLookupAnswer::of(lookup::BUSY),
        MenuAsk::Lookup | MenuAsk::Pick => VideoLookupAnswer::of(lookup::USE_WINDOW),
    }
}

/// The log line for a look-up the quality menu asked for: the video's site (not its address,
/// which may carry a signed link's keys) and how it went. A private window's site is left out,
/// as in [`left_line`]: the log outlives the window.
fn lookup_line(browser: &str, address: &str, private: bool, state: &str) -> String {
    let browser = if browser.is_empty() {
        "a browser"
    } else {
        browser
    };
    let site = clean(&mdm_ipc::capture::host_of(address), 200);
    let what = if private {
        "a video in a private window".to_owned()
    } else if site.trim().is_empty() {
        "a video".to_owned()
    } else {
        format!("a video on {site}")
    };
    format!(
        "browser integration: {browser} looked up the qualities of {what}: {}",
        clean(state, 32)
    )
}

/// `cmd` handed to the app: the reply saying so, or why not.
fn delivered(cmd: &mdm_ipc::Command) -> Reply {
    match deliver(cmd) {
        Ok(()) => Reply::done(),
        Err(Undelivered::NotRunning(e)) => Reply::failed("not-running", e),
        Err(Undelivered::Refused(e)) if e.starts_with("too-large") => Reply::failed("too-large", e),
        Err(Undelivered::Refused(e)) => Reply::failed("refused", e),
    }
}

/// Whether Options let the browser hand `m` over; an explicit request always may. Both
/// addresses must pass when redirects led elsewhere (an excluded site sending to a CDN).
fn verdict(
    m: &Message,
    rules: &mdm_ipc::capture::CaptureRules,
) -> Result<(), mdm_ipc::capture::Declined> {
    if m.explicit {
        return Ok(());
    }
    let (name, browser) = (file_name_of(m), browser(m));
    rules.accepts(&m.url, &name, browser)?;
    if rules.too_small(m.size) {
        return Err(mdm_ipc::capture::Declined::Small);
    }
    match m.final_url.as_deref().filter(|f| *f != m.url) {
        Some(f) => rules.accepts(f, &name, browser),
        None => Ok(()),
    }
}

/// Read one native message; None at end of input.
/// Read one native message; None at end of input. `Err` inside: a message over the limit (its
/// size), read past and dropped, to be answered "too-large".
fn read_message(r: &mut impl Read) -> io::Result<Option<Result<Vec<u8>, usize>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    // Browsers send at most 4 GB, but a download request is small: refuse anything silly (the
    // app's channel takes this much, so whatever is read here reaches it). Read past it, so
    // the extension hears why instead of a closed host.
    if len > mdm_ipc::MAX_MESSAGE {
        io::copy(&mut r.take(len as u64), &mut io::sink())?;
        return Ok(Some(Err(len)));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(Ok(buf)))
}

fn write_message(w: &mut impl Write, json: &[u8]) -> io::Result<()> {
    w.write_all(&(json.len() as u32).to_le_bytes())?;
    w.write_all(json)?;
    w.flush()
}

fn to_command(m: Message) -> mdm_ipc::Command {
    let some = |s: Option<String>| s.filter(|s| !s.is_empty());
    // The browser's file name, without its folder.
    let name = some(m.filename).and_then(|f| {
        f.rsplit(['\\', '/'])
            .next()
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
    });
    mdm_ipc::Command {
        final_url: some(m.final_url).filter(|f| *f != m.url),
        url: Some(m.url),
        name,
        referer: some(m.referrer),
        cookies: some(m.cookies),
        cookie_jar: m.cookie_jar,
        user_agent: m.user_agent,
        size: m.size.filter(|s| *s > 0),
        mime: some(m.mime),
        // A page's media from the toolbar button's list (#800): its page's title names a file
        // whose address names nothing.
        video_title: m
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| t.chars().take(TITLE_MAX).collect()),
        post_type: m.post.as_ref().and(some(m.post_type)),
        post: m.post,
        private: m.private,
        proxy: m.proxy.as_deref().and_then(browser_proxy),
        proxy_pac: m
            .proxy_pac
            .filter(|s| s.len() <= PAC_MAX && s.contains("FindProxyForURL")),
        proxy_login: m.proxy_login,
        ..Default::default()
    }
}

/// The largest proxy auto-config script taken from the browser (#42).
const PAC_MAX: usize = 256 * 1024;

/// The browser's proxy (#42) if it is one the app can use: `scheme://host:port`, the scheme
/// http, https, socks4 or socks5(h), the host a name or an IP address (IPv6 in brackets), nothing
/// after the port. Anything else is dropped (the app then uses its own proxy settings).
fn browser_proxy(s: &str) -> Option<String> {
    let s = s.trim();
    let (scheme, rest) = s.split_once("://")?;
    if !matches!(scheme, "http" | "https" | "socks4" | "socks5" | "socks5h") {
        return None;
    }
    let (host, port) = rest.rsplit_once(':')?;
    let port: u16 = port.parse().ok().filter(|p| *p > 0)?;
    let ok_host = match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        Some(v6) => {
            !v6.is_empty()
                && v6
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        }
        None => {
            !host.is_empty()
                && host.len() <= 253
                && host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        }
    };
    ok_host.then(|| format!("{scheme}://{host}:{port}"))
}

/// Why a command didn't reach the app.
enum Undelivered {
    /// It isn't running and couldn't be started (in time).
    NotRunning(String),
    /// It is running and said no (or stopped answering): starting another can't help.
    Refused(String),
}

/// How long a Mister Download Manager just started gets to open its channel (a first start, a
/// slow disk or an antivirus scan can take a while); the browser's download waits meanwhile.
const START_WAIT: Duration = Duration::from_secs(20);

/// Started by Safari's app extension (`--safari`): inside its sandbox, which a program started from
/// here would inherit, so the app is started through LaunchServices (`open`) instead.
static SAFARI: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The app this helper belongs to, when it is the copy inside Safari's extension
/// (`X.app/Contents/PlugIns/Y.appex/Contents/MacOS/this`): its bundle.
fn bundle_of(exe: &std::path::Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(PathBuf::from)
}

/// Hand `cmd` to Mister Download Manager, starting it only if none is running.
fn deliver(cmd: &mdm_ipc::Command) -> Result<(), Undelivered> {
    // Whatever it asks may open a window: the app may bring it to the front (Windows only: Linux
    // and macOS have no foreground lock for a host to lift).
    #[cfg(windows)]
    if let Some(pid) = mdm_ipc::app_pid() {
        win32::allow_foreground(pid);
    }
    match mdm_ipc::send(cmd) {
        Ok(()) => return Ok(()),
        Err(mdm_ipc::SendError::Refused(e)) => return Err(Undelivered::Refused(e)),
        Err(mdm_ipc::SendError::NotRunning(_)) => {}
    }
    let dir = exe_dir().map_err(Undelivered::NotRunning)?;
    // Beside the app, or (an AppImage's host, copied out of its mount: unix.rs) the path
    // written next to this copy.
    let app = match std::fs::read_to_string(dir.join("app-path")) {
        Ok(path) if !path.trim().is_empty() => PathBuf::from(path.trim()),
        _ => dir.join(if cfg!(windows) {
            "misterdownloadmanager.exe"
        } else {
            "misterdownloadmanager"
        }),
    };
    let safari = SAFARI.load(std::sync::atomic::Ordering::Relaxed);
    let mut child = if safari {
        let exe = this_program().map_err(Undelivered::NotRunning)?;
        let bundle = bundle_of(&exe)
            .ok_or_else(|| Undelivered::NotRunning(format!("{} is in no app", exe.display())))?;
        // -g: the app opens behind Safari, as it does for the other browsers.
        std::process::Command::new("/usr/bin/open")
            .arg("-g")
            .arg(&bundle)
            .spawn()
            .map_err(|e| {
                Undelivered::NotRunning(format!("cannot start {}: {e}", bundle.display()))
            })?
    } else {
        start_detached(&app)
            .map_err(|e| Undelivered::NotRunning(format!("cannot start {}: {e}", app.display())))?
    };
    #[cfg(windows)]
    win32::allow_foreground(child.id());
    let started = std::time::Instant::now();
    while started.elapsed() < START_WAIT {
        std::thread::sleep(Duration::from_millis(100));
        match mdm_ipc::send(cmd) {
            Ok(()) => return Ok(()),
            Err(mdm_ipc::SendError::Refused(e)) => return Err(Undelivered::Refused(e)),
            Err(mdm_ipc::SendError::NotRunning(_)) => {}
        }
        // It quit (another copy took over, or it failed to start): that one may answer still.
        // (`open` returns at once, the app being another process.)
        if !safari
            && child.try_wait().is_ok_and(|s| s.is_some())
            && started.elapsed() > START_WAIT / 4
        {
            break;
        }
    }
    Err(Undelivered::NotRunning(
        "Mister Download Manager did not start".into(),
    ))
}

/// Start the app outside the browser's reach: browsers end a host's whole process tree (on
/// Windows a job object, elsewhere its process group) once it has answered, which would end an
/// app this host started along with it (MDN, "Native messaging": closing the native app).
fn start_detached(app: &std::path::Path) -> io::Result<std::process::Child> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(app);
    // The browser's message pipes are this host's standard streams: never the app's.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        // Leaving the job is refused when the job doesn't allow it: then without.
        if let Ok(child) = cmd
            .creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
        {
            return Ok(child);
        }
        cmd.creation_flags(flags);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
}

fn exe_dir() -> Result<PathBuf, String> {
    std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| "no exe dir".into())
}

fn serve() -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    while let Some(raw) = read_message(&mut stdin)? {
        let raw = match raw {
            Ok(raw) => raw,
            Err(len) => {
                let reply = Reply::failed("too-large", format!("a message of {len} bytes"));
                write_message(
                    &mut stdout,
                    &serde_json::to_vec(&reply).map_err(io::Error::other)?,
                )?;
                continue;
            }
        };
        let reply = match serde_json::from_slice::<Message>(&raw) {
            // The rules as saved now: a change in Options applies to the next download.
            Ok(m) => {
                // This browser has the extension: the app stops offering it (extension.rs).
                mdm_ipc::extension::mark_seen(browser(&m), m.v.as_deref().unwrap_or_default());
                answer(m, &mdm_ipc::capture::CaptureRules::load())
            }
            Err(e) => Reply::failed("refused", e.to_string()),
        };
        write_message(
            &mut stdout,
            &serde_json::to_vec(&reply).map_err(io::Error::other)?,
        )?;
    }
    Ok(())
}

/// This program, which the manifests name.
fn this_program() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string())
}

/// The manifest Chromium browsers read: the host (`program`) may serve our extension's origin
/// only.
fn chromium_manifest(program: &std::path::Path) -> serde_json::Value {
    serde_json::json!({
        "name": HOST_NAME,
        "description": "Mister Download Manager",
        "path": program,
        "type": "stdio",
        "allowed_origins": chromium_ids().into_iter().map(origin).collect::<Vec<_>>(),
    })
}

/// Firefox's manifest: the same host, allowed for our add-on ID instead of an origin.
fn firefox_manifest(program: &std::path::Path) -> serde_json::Value {
    serde_json::json!({
        "name": HOST_NAME,
        "description": "Mister Download Manager",
        "path": program,
        "type": "stdio",
        "allowed_extensions": [FIREFOX_ID],
    })
}

fn write_json(path: &std::path::Path, value: &serde_json::Value) -> Result<(), String> {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("{}: {e}", path.display()))
}

fn portable_refused() -> Result<(), String> {
    if mdm_ipc::portable_dir().is_some() {
        return Err(
            "portable mode: browser integration is not registered (nothing is written outside \
             its folder); install Mister Download Manager to use it"
                .into(),
        );
    }
    Ok(())
}

/// A Chromium extension's origin, as browsers name the caller.
fn origin(id: &str) -> String {
    format!("chrome-extension://{id}/")
}

/// Is this the caller the browsers start us for? Chromium passes the extension's origin first
/// (unpacked or from the store); Firefox passes the manifest's path, then the add-on ID.
fn our_caller(args: &[String]) -> bool {
    args.first()
        .is_some_and(|a| chromium_ids().iter().any(|id| *a == origin(id)))
        || args.get(1).is_some_and(|a| a == FIREFOX_ID)
}

/// Windows: the manifests beside this exe, and the registry entries naming them, per user
/// (HKCU) or for everyone (HKLM).
#[cfg(windows)]
fn register(all_users: bool) -> Result<(), String> {
    portable_refused()?;
    let root = if all_users { "HKLM" } else { "HKCU" };
    let path = exe_dir()?.join(format!("{HOST_NAME}.json"));
    write_json(&path, &chromium_manifest(&this_program()?))?;
    for key in REGISTRY_KEYS {
        reg(&[
            "add",
            &format!(r"{root}\{key}\{HOST_NAME}"),
            "/ve",
            "/t",
            "REG_SZ",
            "/d",
            &path.display().to_string(),
            "/f",
        ])?;
    }
    let ff_path = exe_dir()?.join(format!("{HOST_NAME}.firefox.json"));
    write_json(&ff_path, &firefox_manifest(&this_program()?))?;
    reg(&[
        "add",
        &format!(r"{root}\{FIREFOX_KEY}\{HOST_NAME}"),
        "/ve",
        "/t",
        "REG_SZ",
        "/d",
        &ff_path.display().to_string(),
        "/f",
    ])?;
    // Chrome offers the store's extension once it is listed (never a placeholder ID).
    if let Some(id) = store_id() {
        reg(&[
            "add",
            &format!(r"{root}\{CHROME_EXTERNAL_KEY}\{id}"),
            "/v",
            "update_url",
            "/t",
            "REG_SZ",
            "/d",
            mdm_ipc::extension::STORE_UPDATE_URL,
            "/f",
            "/reg:32",
        ])?;
    }
    println!(
        "registered {HOST_NAME} for Chrome, Edge, Brave, Helium and the other Chromium browsers \
         ({}), and for Firefox, Zen and its other forks ({})",
        path.display(),
        ff_path.display()
    );
    Ok(())
}

#[cfg(windows)]
fn unregister(all_users: bool) -> Result<(), String> {
    let root = if all_users { "HKLM" } else { "HKCU" };
    for key in REGISTRY_KEYS.into_iter().chain([FIREFOX_KEY]) {
        let _ = reg(&["delete", &format!(r"{root}\{key}\{HOST_NAME}"), "/f"]);
    }
    let _ = reg(&[
        "delete",
        &format!(r"{root}\{CHROME_EXTERNAL_KEY}\{STORE_ID}"),
        "/f",
        "/reg:32",
    ]);
    let _ = std::fs::remove_file(exe_dir()?.join(format!("{HOST_NAME}.json")));
    let _ = std::fs::remove_file(exe_dir()?.join(format!("{HOST_NAME}.firefox.json")));
    println!("unregistered {HOST_NAME}");
    Ok(())
}

#[cfg(windows)]
fn reg(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("reg")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

/// Linux and macOS: the folders the browsers there read host manifests from, for one user (in
/// the home folder) or for everyone, each with whether it is Firefox's (`mdm_ipc::browsers`).
/// With `host`, a user's folder is left out for a browser that reads a manifest for everyone
/// naming it (the package's, #266).
#[cfg(not(windows))]
fn host_dirs(all_users: bool, host: Option<&std::path::Path>) -> Vec<(PathBuf, bool)> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    mdm_ipc::browsers::host_dirs(&home, &config, cfg!(target_os = "macos"), all_users, host)
        .into_iter()
        .map(|(d, f)| (d, f == mdm_ipc::browsers::Family::Gecko))
        .collect()
}

/// Linux and macOS: the folders the browsers there read external extensions from (an
/// `<id>.json` naming the store's update service makes the browser offer that extension at its
/// next start), for those that are there, each with the service's address as that browser names
/// it (`mdm_ipc::browsers::external_dirs`: Chrome's, and Helium's own, #685). Linux Chrome reads
/// only the system's, so a per-user registration there leaves Chrome to the app's own offer
/// (extension.rs).
#[cfg(not(windows))]
fn external_dirs(all_users: bool) -> Vec<(PathBuf, &'static str)> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    mdm_ipc::browsers::external_dirs(&home, &config, cfg!(target_os = "macos"), all_users)
}

/// Linux: this program runs from inside an AppImage's mount (`APPDIR`), which is gone once the
/// AppImage quits: a manifest naming it would name a program that no longer exists (#253). The
/// app registers a copy outside it instead (unix.rs).
#[cfg(not(windows))]
fn mount_refused() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    match std::env::var_os("APPDIR").map(PathBuf::from) {
        Some(mount) if !mount.as_os_str().is_empty() && exe.starts_with(&mount) => Err(format!(
            "{} is inside the AppImage's mount, which is gone once it quits: register a copy \
             outside it",
            exe.display()
        )),
        _ => Ok(()),
    }
}

#[cfg(not(windows))]
fn register(all_users: bool) -> Result<(), String> {
    portable_refused()?;
    mount_refused()?;
    let program = this_program()?;
    let (chromium, firefox) = (chromium_manifest(&program), firefox_manifest(&program));
    let dirs = host_dirs(all_users, Some(&program));
    // A browser that reads the package's manifest for everyone, naming this program, needs none
    // in the home folder: one written there before is taken away, so uninstalling the package
    // leaves nothing behind (#266). The browser reads a user's manifest first, so one naming
    // another program (an earlier MDM, an AppImage's copy) would win over the package's: the MDM
    // registered last is the one the browsers start, as when each wrote its own.
    let mut covered = 0;
    if !all_users {
        for (dir, _) in host_dirs(false, None) {
            if dirs.iter().any(|(d, _)| *d == dir) {
                continue;
            }
            covered += 1;
            let manifest = dir.join(format!("{HOST_NAME}.json"));
            if std::fs::remove_file(&manifest).is_ok() {
                println!(
                    "removed {} (the browser reads the one for everyone)",
                    manifest.display()
                );
            }
        }
    }
    if covered > 0 {
        println!(
            "registered {HOST_NAME} for everyone by the package: {covered} of this user's \
             browser folders need nothing"
        );
    }
    // One browser's folder that can't be written doesn't keep the others from theirs.
    let mut failed = Vec::new();
    for (dir, is_firefox) in &dirs {
        let manifest = if *is_firefox { &firefox } else { &chromium };
        let written = std::fs::create_dir_all(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))
            .and_then(|()| write_json(&dir.join(format!("{HOST_NAME}.json")), manifest));
        match written {
            Ok(()) => println!("registered {HOST_NAME} in {}", dir.display()),
            Err(e) => failed.push(e),
        }
    }
    if dirs.is_empty() && covered == 0 {
        println!("registered {HOST_NAME} for no browser: none found");
    }
    if !failed.is_empty() {
        return Err(failed.join("; "));
    }
    // Linux, the .deb's or .rpm's helper for one user: no offer in the home folder, which the
    // package manager couldn't take away with the package (#266); the app's window offers it.
    #[cfg(target_os = "linux")]
    let offer = all_users || program != std::path::Path::new(PACKAGED_HOST);
    #[cfg(not(target_os = "linux"))]
    let offer = true;
    if let Some(id) = store_id().filter(|_| offer) {
        for (dir, update_url) in external_dirs(all_users) {
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            write_json(
                &dir.join(format!("{id}.json")),
                &serde_json::json!({ "external_update_url": update_url }),
            )?;
            println!("offered the extension in {}", dir.display());
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn unregister(all_users: bool) -> Result<(), String> {
    for (dir, _) in host_dirs(all_users, None) {
        let _ = std::fs::remove_file(dir.join(format!("{HOST_NAME}.json")));
    }
    for (dir, _) in external_dirs(all_users) {
        let _ = std::fs::remove_file(dir.join(format!("{STORE_ID}.json")));
    }
    println!("unregistered {HOST_NAME}");
    Ok(())
}

/// Linux: where the .deb and .rpm install this program, which their manifests name.
#[cfg(target_os = "linux")]
const PACKAGED_HOST: &str = "/usr/bin/misterdownloadmanager-nmhost";

/// Linux (the .deb and .rpm build, tools/installer/build-linux.sh): the manifests for everyone,
/// in each folder the browsers read them from (`mdm_ipc::browsers::LINUX_SYSTEM_HOST_DIRS`),
/// under `root`, naming the packaged helper. The package owns them, so the package manager
/// removes them with it (#266).
#[cfg(target_os = "linux")]
fn package_manifests(root: Option<&str>) -> Result<(), String> {
    let root = std::path::Path::new(root.ok_or("--package-manifests needs a folder")?);
    let program = std::path::Path::new(PACKAGED_HOST);
    for (dir, family) in mdm_ipc::browsers::LINUX_SYSTEM_HOST_DIRS {
        let dir = root.join(dir.trim_start_matches('/'));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let manifest = match family {
            mdm_ipc::browsers::Family::Chromium => chromium_manifest(program),
            mdm_ipc::browsers::Family::Gecko => firefox_manifest(program),
        };
        let path = dir.join(format!("{HOST_NAME}.json"));
        write_json(&path, &manifest)?;
        println!("{}", path.display());
    }
    Ok(())
}

/// The system-wide manifests are packaged for Linux only (macOS's and Windows' installers
/// register with `--register --all-users`).
#[cfg(not(target_os = "linux"))]
fn package_manifests(_: Option<&str>) -> Result<(), String> {
    Err("--package-manifests: the .deb and .rpm are Linux's".into())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let all_users = args.iter().any(|a| a == "--all-users");
    let result = match args.first().map(String::as_str).unwrap_or_default() {
        "--register" => register(all_users),
        "--unregister" => unregister(all_users),
        "--package-manifests" => package_manifests(args.get(1).map(String::as_str)),
        // Safari's extension has no host manifest: its app extension (tools/safari) starts this
        // program from inside the app, which is the only caller there is.
        "--safari" => {
            SAFARI.store(true, std::sync::atomic::Ordering::Relaxed);
            serve().map_err(|e| e.to_string())
        }
        // Browsers start the host naming the caller: serve only our own extension (the
        // manifests' allowed_origins / allowed_extensions say the same; this is a second check).
        _ if our_caller(&args) => serve().map_err(|e| e.to_string()),
        _ => Err(format!(
            "refusing caller {args:?}: not the Mister Download Manager extension"
        )),
    };
    if let Err(e) = result {
        eprintln!("misterdownloadmanager-nmhost: {e}");
        std::process::exit(1);
    }
}

/// The addresses MDM downloads (mdm_engine::supported): http(s), and ftp links sent from a page
/// (browsers no longer download ftp themselves).
fn downloadable(url: &str) -> bool {
    let scheme = url.split_once("://").map_or("", |(s, _)| s);
    ["http", "https", "ftp", "ftps", "ftpes"]
        .iter()
        .any(|k| scheme.eq_ignore_ascii_case(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Safari's copy of the helper, inside the app's extension, finds the app it opens.
    #[test]
    fn safari_helper_finds_its_app() {
        let exe = std::path::Path::new(
            "/Applications/Mister Download Manager.app/Contents/PlugIns/Mister Download Manager.appex/Contents/MacOS/misterdownloadmanager-nmhost",
        );
        assert_eq!(
            bundle_of(exe),
            Some(PathBuf::from("/Applications/Mister Download Manager.app"))
        );
        assert_eq!(bundle_of(std::path::Path::new("/usr/bin/nmhost")), None);
    }

    /// The Chrome Web Store's copy and the app folder's (unpacked) both connect, and both are
    /// served: the manifest's allowed_origins and the host's own caller check agree.
    #[test]
    fn store_and_unpacked_extensions_both_allowed() {
        let manifest = chromium_manifest(&this_program().unwrap());
        for id in [
            "ejoleppdfgjdndddcjoehopknmlglojp",
            "oogbclbbninliongijffeeggalppliok",
        ] {
            let o = format!("chrome-extension://{id}/");
            assert!(
                manifest["allowed_origins"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!(o)),
                "{manifest}"
            );
            assert!(our_caller(&[o]));
        }
        assert!(!our_caller(&[
            "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/".into()
        ]));
    }

    #[test]
    fn web_and_ftp_links_are_downloadable() {
        assert!(downloadable("https://x.org/a.zip") && downloadable("FTP://x.org/a.iso"));
        assert!(downloadable("ftps://x/a") && downloadable("ftpes://x/a"));
        assert!(!downloadable("file:///C:/a") && !downloadable("javascript:alert(1)"));
    }

    #[test]
    fn only_our_extension_in_either_browser() {
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        for id in chromium_ids() {
            assert!(our_caller(&s(&[&format!("chrome-extension://{id}/")])));
        }
        assert!(our_caller(&s(&[r"C:\x\host.firefox.json", FIREFOX_ID])));
        assert!(!our_caller(&s(&["chrome-extension://someoneelse/"])));
        assert!(!our_caller(&s(&[r"C:\x\host.json", "evil@example.com"])));
        assert!(!our_caller(&[]));
    }

    #[test]
    fn options_decide_what_is_taken() {
        let mut rules = mdm_ipc::capture::CaptureRules::default();
        let ua = "Mozilla/5.0 (Windows NT 10.0) Chrome/141.0.0.0 Safari/537.36";
        let msg = |url: &str, name: &str| Message {
            url: url.into(),
            filename: Some(name.into()),
            user_agent: Some(ua.into()),
            ..Default::default()
        };
        // Not a listed type: left to the browser, nothing delivered.
        let r = answer(msg("https://example.com/page.html", "page.html"), &rules);
        assert!(!r.ok && r.error.unwrap().contains("FileType"));
        // An excluded site.
        let r = answer(
            msg("https://dl.update.microsoft.com/kb.exe", "kb.exe"),
            &rules,
        );
        assert!(!r.ok && r.error.unwrap().contains("Site"));
        // The browser off in Options.
        rules.browsers = vec![("Google Chrome".into(), false)];
        let r = answer(
            msg(
                "https://example.com/a.zip",
                "C:\\Users\\u\\Downloads\\a.zip",
            ),
            &rules,
        );
        assert!(!r.ok && r.error.unwrap().contains("Browser"));
        // The rules themselves.
        let r = answer(
            Message {
                rules: true,
                ..Default::default()
            },
            &rules,
        );
        assert!(r.ok && r.types.unwrap().contains("ZIP"));
        assert_eq!(file_name_of(&msg("https://x.org/f/b.rar?x=1", "")), "b.rar");
        // Right-click → Download with Mister Download Manager: any link, whatever Options say.
        let mut page = msg("https://example.com/page.html", "");
        assert!(verdict(&page, &rules).is_err());
        page.explicit = true;
        assert!(verdict(&page, &rules).is_ok());
        rules.advanced_integration = false;
        assert!(verdict(&page, &rules).is_ok());
    }

    #[test]
    fn the_video_button_asks_the_rules() {
        let ua = "Mozilla/5.0 (Windows NT 10.0) Chrome/141.0.0.0 Safari/537.36";
        let mut rules = mdm_ipc::capture::CaptureRules {
            video_panel_sites: "*.quiet.test".into(),
            ..Default::default()
        };
        let ask = |page: &str| Message {
            panel: true,
            url: page.into(),
            user_agent: Some(ua.into()),
            ..Default::default()
        };
        assert!(answer(ask("https://videos.test/watch"), &rules).ok);
        assert!(!answer(ask("https://www.quiet.test/watch"), &rules).ok);
        // #532: a player embedded from another site, in a tab on a listed site.
        let embedded = Message {
            referrer: Some("https://www.quiet.test/post".into()),
            ..ask("https://videos.test/embed/1")
        };
        assert!(!answer(embedded, &rules).ok);
        rules.video_panel = false;
        assert!(!answer(ask("https://videos.test/watch"), &rules).ok);
        // Clicked where it's off (the page was open before the change): refused, nothing sent.
        let click = Message {
            video: true,
            url: "https://videos.test/watch".into(),
            user_agent: Some(ua.into()),
            ..Default::default()
        };
        let r = answer(click, &rules);
        assert!(!r.ok && r.error.unwrap().contains("off"));
        // The toolbar button's media list asks for a stream explicitly (#38): taken wherever the
        // button is off, the manifest fetched as from its page with the cookies its server gets.
        let listed = Message {
            video: true,
            explicit: true,
            url: "https://www.quiet.test/watch".into(),
            src: Some("https://cdn.test/v/master.m3u8?t=1".into()),
            title: Some("A talk about download managers".into()),
            cookies: Some("cdn=1".into()),
            cookie_jar: vec![mdm_ipc::BrowserCookie {
                name: "cdn".into(),
                value: "1".into(),
                domain: ".cdn.test".into(),
                path: "/".into(),
                ..Default::default()
            }],
            user_agent: Some(ua.into()),
            ..Default::default()
        };
        let cmd = video_command(&listed, &rules, browser(&listed)).unwrap();
        assert_eq!(
            cmd.video_page.as_deref(),
            Some("https://cdn.test/v/master.m3u8?t=1")
        );
        assert_eq!(cmd.referer.as_deref(), Some("https://www.quiet.test/watch"));
        assert_eq!(cmd.cookies.as_deref(), Some("cdn=1"));
        assert_eq!(
            cmd.cookie_jar, listed.cookie_jar,
            "each cookie with its domain"
        );
        assert_eq!(cmd.user_agent.as_deref(), Some(ua));
        // The page's title goes with it (#345): the app names a video yt-dlp knows only by its
        // address (here `master`) after it.
        assert_eq!(
            cmd.video_title.as_deref(),
            Some("A talk about download managers")
        );
        // With a page too: the app keeps yt-dlp's title unless it is only the address's.
        let page_only = Message {
            video: true,
            explicit: true,
            url: "https://www.quiet.test/watch".into(),
            title: Some("A talk about download managers".into()),
            ..Default::default()
        };
        let cmd = video_command(&page_only, &rules, browser(&page_only)).unwrap();
        assert_eq!(
            cmd.video_page.as_deref(),
            Some("https://www.quiet.test/watch")
        );
        assert_eq!(
            cmd.video_title.as_deref(),
            Some("A talk about download managers")
        );
        // A blank title is none.
        let blank = Message {
            video: true,
            explicit: true,
            url: "https://www.quiet.test/watch".into(),
            src: Some("https://cdn.test/v/master.m3u8".into()),
            title: Some("   ".into()),
            ..Default::default()
        };
        let cmd = video_command(&blank, &rules, browser(&blank)).unwrap();
        assert_eq!(cmd.video_title, None);
        // From the wire: the extension's `title`.
        let m: Message = serde_json::from_str(
            r#"{"video":true,"explicit":true,"url":"https://a.test/w","src":"https://cdn.test/hls/index.m3u8","title":"Clip"}"#,
        )
        .unwrap();
        assert_eq!(
            video_command(&m, &rules, browser(&m))
                .unwrap()
                .video_title
                .as_deref(),
            Some("Clip")
        );
        // Never a page that isn't on the web.
        let local = Message {
            url: "file:///C:/x.html".into(),
            ..listed
        };
        assert!(video_command(&local, &rules, browser(&local)).is_err());
        // From the wire: the extension's camelCase names.
        let m: Message =
            serde_json::from_str(r#"{"hideSite":"tube.test","openOptions":false}"#).unwrap();
        assert_eq!(m.hide_site.as_deref(), Some("tube.test"));
    }

    #[test]
    fn the_buttons_file_show_and_turn_on_reach_the_app() {
        let ua = "Mozilla/5.0 (Windows NT 10.0) Chrome/141.0.0.0 Safari/537.36";
        let rules = mdm_ipc::capture::CaptureRules::default();
        // #703: a plain file is downloaded as one, its page the referrer.
        let m: Message = serde_json::from_str(&format!(
            r#"{{"video":true,"url":"https://videos.test/p","src":"https://cdn.test/a.mp4","direct":true,"userAgent":"{ua}"}}"#
        ))
        .unwrap();
        let cmd = video_command(&m, &rules, browser(&m)).unwrap();
        assert!(cmd.video_direct);
        assert_eq!(cmd.video_page.as_deref(), Some("https://cdn.test/a.mp4"));
        assert_eq!(cmd.referer.as_deref(), Some("https://videos.test/p"));
        assert_eq!(cmd.mime, None);
        // #800: with the type its server said, which names a script's answer at once.
        let m: Message = serde_json::from_str(&format!(
            r#"{{"video":true,"url":"https://videos.test/p","src":"https://cdn.test/stream.php?id=1","direct":true,"mime":"video/mp4","title":"Talk","userAgent":"{ua}"}}"#
        ))
        .unwrap();
        let cmd = video_command(&m, &rules, browser(&m)).unwrap();
        assert_eq!(
            (cmd.mime.as_deref(), cmd.video_title.as_deref()),
            (Some("video/mp4"), Some("Talk"))
        );
        // `direct` without a file of its own is the page, as before.
        let m: Message = serde_json::from_str(&format!(
            r#"{{"video":true,"url":"https://videos.test/p","direct":true,"userAgent":"{ua}"}}"#
        ))
        .unwrap();
        assert!(!video_command(&m, &rules, browser(&m)).unwrap().video_direct);
        // "Show" and "Turn on": no `url`, so an older host refuses them and does nothing.
        let m: Message = serde_json::from_str(
            r#"{"showDownload":"https://videos.test/watch?v=1","showSite":"videos.test"}"#,
        )
        .unwrap();
        assert!(m.url.is_empty() && !m.video);
        assert_eq!(m.show_site.as_deref(), Some("videos.test"));
        assert_eq!(
            m.show_download.as_deref(),
            Some("https://videos.test/watch?v=1")
        );
    }

    /// `hello` and `rules` say the host and its app have the quality menu; nothing else does.
    #[test]
    fn the_quality_menu_is_announced() {
        let rules = mdm_ipc::capture::CaptureRules::default();
        let ua = Some("Mozilla/5.0 (Windows NT 10.0) Chrome/141.0.0.0 Safari/537.36".into());
        for m in [
            Message {
                hello: true,
                user_agent: ua.clone(),
                ..Default::default()
            },
            Message {
                rules: true,
                user_agent: ua.clone(),
                ..Default::default()
            },
        ] {
            let json = serde_json::to_string(&answer(m, &rules)).unwrap();
            assert!(json.contains(r#""videoLookup":true"#), "{json}");
        }
        let asked = answer(
            Message {
                panel: true,
                url: "https://videos.test/watch".into(),
                user_agent: ua,
                ..Default::default()
            },
            &rules,
        );
        let json = serde_json::to_string(&asked).unwrap();
        assert!(
            !json.contains("videoLookup") && !json.contains("lookup"),
            "{json}"
        );
    }

    /// #900: a stream goes with the Referer and Origin its page's player sent for it (the
    /// extension's `fetchedWith`), in the menu's look-up and the window's click alike; without
    /// them (an older extension), or with what isn't a web address, the page is its Referer and
    /// no Origin goes. Only for a stream or file: a page looked up keeps its own referrer.
    #[test]
    fn a_stream_goes_with_the_referer_and_origin_the_browser_sent() {
        let rules = mdm_ipc::capture::CaptureRules::default();
        let page = "https://anime.test/watch/12";
        let src = "https://cdn.video.test/manifest/x.m3u8?sec=abc";
        let fetched = r#""fetchedWith":{"referer":"https://player.video.test/","origin":"https://player.video.test"}"#;
        let lookup = |extra: &str, src: &str| -> mdm_ipc::Command {
            let m: Message = serde_json::from_str(&format!(
                r#"{{"videoLookup":{{"page":"{page}","src":"{src}","referrer":"https://search.test/"}}{extra}}}"#
            ))
            .unwrap();
            menu_command(MenuAsk::Lookup, &m, &rules, "").unwrap()
        };
        let click = |extra: &str, src: &str| -> mdm_ipc::Command {
            let m: Message = serde_json::from_str(&format!(
                r#"{{"video":true,"url":"{page}","src":"{src}","referrer":"https://search.test/"{extra}}}"#
            ))
            .unwrap();
            video_command(&m, &rules, "").unwrap()
        };
        for cmd in [
            lookup(&format!(",{fetched}"), src),
            click(&format!(",{fetched}"), src),
        ] {
            assert_eq!(cmd.referer.as_deref(), Some("https://player.video.test/"));
            assert_eq!(cmd.origin.as_deref(), Some("https://player.video.test"));
        }
        // An older extension: from the page, no Origin.
        for cmd in [lookup("", src), click("", src)] {
            assert_eq!(cmd.referer.as_deref(), Some(page));
            assert_eq!(cmd.origin, None);
        }
        // Not web addresses (a header's line break, the browser's "null" origin): left out.
        let odd = r#","fetchedWith":{"referer":"https://x.test/\r\nX-Evil: 1","origin":"null"}"#;
        for cmd in [lookup(odd, src), click(odd, src)] {
            assert_eq!(cmd.referer.as_deref(), Some(page));
            assert_eq!(cmd.origin, None);
        }
        // No stream (a player playing from blob:): the page is looked up, with its own referrer.
        for cmd in [
            lookup(&format!(",{fetched}"), "blob:https://anime.test/1"),
            click(&format!(",{fetched}"), "blob:https://anime.test/1"),
        ] {
            assert_eq!(cmd.referer.as_deref(), Some("https://search.test/"));
            assert_eq!(cmd.origin, None);
        }
    }

    /// The quality menu's three messages, as the extension sends them (camelCase, no `url` of
    /// their own), become the app's questions, each with `startup` (an older app opens nothing).
    #[test]
    fn the_quality_menus_messages_become_the_apps_questions() {
        let rules = mdm_ipc::capture::CaptureRules::default();
        let m: Message = serde_json::from_str(
            r#"{"videoLookup":{"page":"https://videos.test/watch?v=1","src":"https://cdn.test/v/master.m3u8?sig=1",
                "title":"  A talk about download managers  ","referrer":"https://search.test/","storeId":"firefox-container-2"},
              "cookies":"a=1","cookieJar":[{"name":"a","value":"1","domain":".cdn.test","path":"/","hostOnly":false}],
              "private":true,"userAgent":"Mozilla/5.0 Firefox/145.0","brands":[],"v":"0.4.0"}"#,
        )
        .unwrap();
        assert_eq!(m.url, "", "no address of its own: an older host refuses it");
        assert_eq!(menu_ask(&m), Some(MenuAsk::Lookup));
        let cmd = menu_command(MenuAsk::Lookup, &m, &rules, browser(&m)).unwrap();
        assert!(cmd.startup && cmd.wants_reply());
        assert_eq!(
            cmd.video_lookup,
            Some(mdm_ipc::VideoLookup {
                address: "https://cdn.test/v/master.m3u8?sig=1".into(),
                title: Some("A talk about download managers".into()),
                store_id: Some("firefox-container-2".into()),
            }),
            "the video's own stream, with its page's title"
        );
        assert_eq!(
            cmd.referer.as_deref(),
            Some("https://videos.test/watch?v=1"),
            "fetched as from its page"
        );
        assert_eq!(cmd.cookies.as_deref(), Some("a=1"));
        assert_eq!(cmd.cookie_jar.len(), 1);
        assert_eq!(cmd.cookie_jar[0].domain, ".cdn.test");
        assert_eq!(cmd.user_agent.as_deref(), Some("Mozilla/5.0 Firefox/145.0"));
        assert!(cmd.private);
        assert!(
            cmd.video_page.is_none() && cmd.url.is_none() && cmd.video_title.is_none(),
            "never the window's click nor a download"
        );
        // A page alone: yt-dlp's to look up, from the page's own referrer; its title goes too
        // (the app keeps yt-dlp's unless it is only the address's).
        let page: Message = serde_json::from_str(
            r#"{"videoLookup":{"page":"https://videos.test/watch?v=1","src":"blob:https://videos.test/x",
                "title":"A talk","referrer":"https://search.test/"}}"#,
        )
        .unwrap();
        let cmd = menu_command(MenuAsk::Lookup, &page, &rules, browser(&page)).unwrap();
        let ask = cmd.video_lookup.unwrap();
        assert_eq!(ask.address, "https://videos.test/watch?v=1");
        assert_eq!(ask.title.as_deref(), Some("A talk"));
        assert_eq!(ask.store_id, None);
        assert_eq!(cmd.referer.as_deref(), Some("https://search.test/"));
        // A long title is cut.
        let long = Message {
            video_lookup: Some(LookupAsk {
                page: "https://videos.test/w".into(),
                src: Some("https://cdn.test/v.mp4".into()),
                title: Some("x".repeat(1000)),
                ..Default::default()
            }),
            ..Default::default()
        };
        let cmd = menu_command(MenuAsk::Lookup, &long, &rules, "").unwrap();
        assert_eq!(cmd.video_lookup.unwrap().title.unwrap().len(), TITLE_MAX);
        // How it goes.
        let poll: Message = serde_json::from_str(r#"{"videoPoll":{"token":"abc"}}"#).unwrap();
        assert_eq!(menu_ask(&poll), Some(MenuAsk::Poll));
        let cmd = menu_command(MenuAsk::Poll, &poll, &rules, "").unwrap();
        assert_eq!(cmd.video_poll.as_deref(), Some("abc"));
        assert!(cmd.startup && cmd.wants_reply());
        // The choice, with the tab's private window and cookie store.
        let pick: Message = serde_json::from_str(
            r#"{"videoPick":{"token":"abc","choice":2,"storeId":"firefox-private"},"private":true}"#,
        )
        .unwrap();
        assert_eq!(menu_ask(&pick), Some(MenuAsk::Pick));
        let cmd = menu_command(MenuAsk::Pick, &pick, &rules, "").unwrap();
        assert_eq!(
            cmd.video_pick,
            Some(mdm_ipc::VideoPick {
                token: "abc".into(),
                choice: 2,
                store_id: Some("firefox-private".into()),
            })
        );
        assert!(cmd.private && cmd.startup && cmd.wants_reply());
        // No token: answered without asking the app.
        let blank: Message = serde_json::from_str(r#"{"videoPoll":{"token":" "}}"#).unwrap();
        assert_eq!(
            menu_command(MenuAsk::Poll, &blank, &rules, "").err(),
            Some(mdm_ipc::api::lookup::GONE)
        );
        let blank: Message = serde_json::from_str(r#"{"videoPick":{"choice":1}}"#).unwrap();
        assert_eq!(
            menu_command(MenuAsk::Pick, &blank, &rules, "").err(),
            Some(mdm_ipc::api::lookup::USE_WINDOW)
        );
        assert_eq!(menu_ask(&Message::default()), None);
    }

    /// Where the button is off, or for a page not on the web, the menu is answered at once
    /// (`off`, `use-window`), in `lookup` and never as a `why`.
    #[test]
    fn the_quality_menu_follows_options() {
        let ua = "Mozilla/5.0 (Windows NT 10.0) Chrome/141.0.0.0 Safari/537.36";
        let mut rules = mdm_ipc::capture::CaptureRules {
            video_panel_sites: "*.quiet.test".into(),
            ..Default::default()
        };
        let look = |page: &str| Message {
            video_lookup: Some(LookupAsk {
                page: page.into(),
                ..Default::default()
            }),
            user_agent: Some(ua.into()),
            ..Default::default()
        };
        let state = |r: Reply| {
            assert!(r.ok && r.why.is_none() && r.error.is_none());
            r.lookup.unwrap().state
        };
        assert_eq!(
            state(answer(look("https://www.quiet.test/w"), &rules)),
            "off"
        );
        assert_eq!(
            state(answer(look("file:///C:/x.html"), &rules)),
            "use-window"
        );
        assert_eq!(state(answer(look(""), &rules)), "use-window");
        rules.video_panel = false;
        assert_eq!(state(answer(look("https://videos.test/w"), &rules)), "off");
        let json = serde_json::to_string(&answer(look("https://videos.test/w"), &rules)).unwrap();
        assert_eq!(json, r#"{"ok":true,"lookup":{"state":"off"}}"#);
    }

    /// The app's answer passes through; its absence, or a line that isn't one, says what the
    /// contract says: the window for a look-up or a choice, `pending` or `gone` for a question.
    #[test]
    fn answers_that_are_not_the_apps() {
        use mdm_ipc::SendError;
        use mdm_ipc::api::lookup;
        let line = |s: &str| Ok::<String, SendError>(s.to_owned());
        let a = menu_answer(
            MenuAsk::Lookup,
            line(r#"{"state":"pending","token":"t1"}"#),
            "",
        );
        assert_eq!(
            (a.state.as_str(), a.token.as_deref()),
            ("pending", Some("t1"))
        );
        let a = menu_answer(
            MenuAsk::Poll,
            line(
                r#"{"state":"ready","token":"t1","title":"A talk","choices":[{"label":"720p mp4","ext":"mp4","size":null}]}"#,
            ),
            "t1",
        );
        assert_eq!(a.state, lookup::READY);
        assert_eq!(a.choices[0].ext, "mp4");
        assert_eq!(
            menu_answer(MenuAsk::Pick, line(r#"{"state":"queued"}"#), "").state,
            lookup::QUEUED
        );
        let not_answers = [
            "ok",
            "denied",
            "gone",
            "too-large",
            "",
            r#"{"error":"the automation API is off","code":"api-off"}"#,
        ];
        for got in not_answers {
            for ask in [MenuAsk::Lookup, MenuAsk::Pick] {
                assert_eq!(
                    menu_answer(ask, line(got), "").state,
                    lookup::USE_WINDOW,
                    "{ask:?} {got}"
                );
            }
        }
        // `timeout`: the app has the command and will act on it. The choice downloads soon (the
        // Download video window would download it again), the look-up runs (another window
        // would run a second yt-dlp).
        assert_eq!(
            menu_answer(MenuAsk::Pick, line("timeout"), "").state,
            lookup::SENT
        );
        assert_eq!(
            menu_answer(MenuAsk::Pick, line("timeout\n"), "").state,
            lookup::SENT
        );
        assert_eq!(
            menu_answer(MenuAsk::Lookup, line("timeout"), "").state,
            lookup::BUSY
        );
        for err in [
            SendError::NotRunning("no data dir".into()),
            SendError::Refused("connection reset".into()),
        ] {
            assert_eq!(
                menu_answer(MenuAsk::Lookup, Err(err), "").state,
                lookup::USE_WINDOW
            );
        }
        assert_eq!(
            menu_answer(MenuAsk::Pick, Err(SendError::NotRunning("x".into())), "").state,
            lookup::USE_WINDOW
        );
        // A question: the app only too busy is asked again; anything else, the look-up is gone.
        let busy = menu_answer(MenuAsk::Poll, line("timeout"), "t1");
        assert_eq!(
            (busy.state.as_str(), busy.token.as_deref()),
            ("pending", Some("t1"))
        );
        for got in ["ok", "denied", "gone", "too-large", ""] {
            assert_eq!(
                menu_answer(MenuAsk::Poll, line(got), "t1").state,
                lookup::GONE
            );
        }
        assert_eq!(
            menu_answer(MenuAsk::Poll, Err(SendError::NotRunning("x".into())), "t1").state,
            lookup::GONE
        );
        // As the extension reads it.
        let json =
            serde_json::to_string(&Reply::lookup(menu_answer(MenuAsk::Lookup, line("ok"), "")))
                .unwrap();
        assert_eq!(json, r#"{"ok":true,"lookup":{"state":"use-window"}}"#);
    }

    /// A question waits while the look-up runs, asking the app every 100 ms, and answers as soon
    /// as it is ready, or `pending` after 1.5 s; whether a choice is sound alone reaches the
    /// browser as the app says it.
    #[test]
    fn a_question_waits_for_the_look_up() {
        use mdm_ipc::SendError;
        use mdm_ipc::api::lookup;
        let ready = r#"{"state":"ready","token":"t1","title":"A talk","choices":[{"label":"720p MP4","ext":"mp4","size":null,"audio":false},{"label":"Audio only MP3 320 kbps","ext":"mp3","size":null,"audio":true}]}"#;
        // A clock of the test's own: asking every 20 ms, the answer is ready at the third
        // question, 40 ms in.
        let now = std::cell::Cell::new(Duration::ZERO);
        let elapsed = || now.get();
        let sleep = |d: Duration| now.set(now.get() + d);
        let mut asked = 0;
        let answer = until_settled_on(
            || {
                asked += 1;
                let got = if asked < 3 {
                    r#"{"state":"pending","token":"t1"}"#
                } else {
                    ready
                };
                menu_answer(MenuAsk::Poll, Ok::<String, SendError>(got.into()), "t1")
            },
            POLL_WAIT,
            Duration::from_millis(20),
            elapsed,
            sleep,
        );
        assert_eq!((answer.state.as_str(), asked), (lookup::READY, 3));
        assert_eq!(now.get(), Duration::from_millis(40));
        let json = serde_json::to_string(&Reply::lookup(answer)).unwrap();
        assert!(
            json.contains(r#""ext":"mp4","size":null,"audio":false"#)
                && json.contains(r#""ext":"mp3","size":null,"audio":true"#),
            "{json}"
        );
        // Still running after the wait: `pending`, having asked every tenth of a second (at 0,
        // 100, 200 and 300 ms) and never waited past the 300 ms.
        now.set(Duration::ZERO);
        let mut asked = 0;
        let answer = until_settled_on(
            || {
                asked += 1;
                mdm_ipc::api::VideoLookupAnswer::pending("t1")
            },
            Duration::from_millis(300),
            Duration::from_millis(100),
            elapsed,
            sleep,
        );
        assert_eq!(answer.state, lookup::PENDING);
        assert_eq!(asked, 4);
        assert_eq!(now.get(), Duration::from_millis(300));
        // Anything else at once, without waiting.
        now.set(Duration::ZERO);
        let mut asked = 0;
        let gone = until_settled_on(
            || {
                asked += 1;
                mdm_ipc::api::VideoLookupAnswer::of(lookup::GONE)
            },
            POLL_WAIT,
            POLL_EVERY,
            elapsed,
            sleep,
        );
        assert_eq!(now.get(), Duration::ZERO);
        assert_eq!((gone.state.as_str(), asked), (lookup::GONE, 1));
    }

    #[test]
    fn look_up_log_lines_keep_private_windows_out() {
        let open = lookup_line(
            "Mozilla Firefox",
            "https://cdn.test/v/master.m3u8?sig=secret",
            false,
            "pending",
        );
        assert_eq!(
            open,
            "browser integration: Mozilla Firefox looked up the qualities of a video on cdn.test: pending"
        );
        let private = lookup_line("", "https://secret.test/v.mp4", true, "use-window");
        assert!(!private.contains("secret") && private.contains("private window"));
        assert!(private.starts_with("browser integration: a browser "));
        assert!(!lookup_line("", "https://x.test/\n", false, "a\nb").contains('\n'));
    }

    #[test]
    fn the_browsers_whole_context_reaches_the_app() {
        let m: Message = serde_json::from_str(
            r#"{"url":"https://x.test/get?id=1","finalUrl":"https://cdn.test/f/abc","filename":"C:\\dl\\report.pdf",
                "size":12345,"mime":"application/pdf","post":"token=abc","postType":"application/x-www-form-urlencoded",
                "private":true,"userAgent":"UA","brands":["Brave"],"v":"0.2.5"}"#,
        )
        .unwrap();
        assert_eq!(browser(&m), "Brave Browser");
        let c = to_command(m);
        assert_eq!(
            c.url.as_deref(),
            Some("https://x.test/get?id=1"),
            "the address asked for"
        );
        assert_eq!(c.final_url.as_deref(), Some("https://cdn.test/f/abc"));
        assert_eq!(
            c.name.as_deref(),
            Some("report.pdf"),
            "without the browser's folder"
        );
        assert_eq!(
            (c.size, c.mime.as_deref()),
            (Some(12345), Some("application/pdf"))
        );
        assert_eq!(c.post.as_deref(), Some("token=abc"));
        assert_eq!(
            c.post_type.as_deref(),
            Some("application/x-www-form-urlencoded")
        );
        assert!(c.private);
        // No form: no Content-Type for one.
        let m: Message = serde_json::from_str(
            r#"{"url":"https://x.test/a.zip","postType":"text/plain","size":0}"#,
        )
        .unwrap();
        let c = to_command(m);
        assert_eq!((c.post_type, c.size, c.final_url), (None, None, None));
    }

    #[test]
    fn redirects_are_checked_at_both_ends() {
        let rules = mdm_ipc::capture::CaptureRules::default();
        let ua = "Mozilla/5.0 (Windows NT 10.0) Chrome/141.0.0.0 Safari/537.36";
        let mut m = Message {
            url: "https://example.com/get/a.zip".into(),
            final_url: Some("https://download.windowsupdate.com/a.zip".into()),
            filename: Some("a.zip".into()),
            user_agent: Some(ua.into()),
            ..Default::default()
        };
        assert_eq!(verdict(&m, &rules), Err(mdm_ipc::capture::Declined::Site));
        m.final_url = Some("https://cdn.example.com/a.zip".into());
        assert_eq!(verdict(&m, &rules), Ok(()));
    }

    #[test]
    fn rules_say_everything_the_extension_needs() {
        let mut rules = mdm_ipc::capture::CaptureRules::default();
        let hello = || Message {
            hello: true,
            user_agent: Some("Mozilla/5.0 Firefox/145.0".into()),
            ..Default::default()
        };
        let r = answer(hello(), &rules);
        assert!(r.ok && r.capture == Some(true) && r.opened == Some(true));
        assert_eq!(r.version, Some(env!("CARGO_PKG_VERSION")));
        assert!(r.excluded_sites.unwrap().contains("windowsupdate"));
        rules.browsers = vec![("Mozilla Firefox".into(), false)];
        assert_eq!(
            answer(hello(), &rules).capture,
            Some(false),
            "this browser off"
        );
        // The wire names.
        let json = serde_json::to_string(&answer(hello(), &rules)).unwrap();
        assert!(json.contains("\"excludedSites\"") && json.contains("\"capture\":false"));
        // Links that aren't downloads are refused without anything sent.
        let r = answer(
            Message {
                links: vec!["javascript:void(0)".into(), "mailto:a@b".into()],
                ..Default::default()
            },
            &rules,
        );
        assert!(!r.ok && r.why == Some("refused"));
    }

    #[test]
    fn a_refusal_says_which_rule() {
        let rules = mdm_ipc::capture::CaptureRules {
            browsers: vec![("Microsoft Edge".into(), false)],
            ..Default::default()
        };
        let ua = "Mozilla/5.0 (Windows NT 10.0) Chrome/141.0.0.0 Safari/537.36";
        let msg = |url: &str, name: &str, ua: &str| Message {
            url: url.into(),
            filename: Some(name.into()),
            user_agent: Some(ua.into()),
            ..Default::default()
        };
        let r = answer(msg("https://x.test/p.png", "p.png", ua), &rules);
        assert_eq!(
            (r.ok, r.why, r.declined),
            (false, Some("options"), Some("file-type"))
        );
        // The words older extensions read stay as they were.
        assert_eq!(
            r.error.as_deref(),
            Some("left to the browser by Options (FileType)")
        );
        let edge = "Mozilla/5.0 (Windows NT 10.0) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0";
        let r = answer(msg("https://x.test/a.zip", "a.zip", edge), &rules);
        assert_eq!(r.declined, Some("browser"));
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""declined":"browser""#), "{json}");
        // The rules say why nothing is taken from this browser.
        let hello = Message {
            hello: true,
            user_agent: Some(edge.into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&answer(hello, &rules)).unwrap();
        assert!(json.contains(r#""capture":false"#) && json.contains(r#""captureOff":"browser""#));
    }

    #[test]
    fn the_extensions_own_refusals_are_only_noted() {
        let rules = mdm_ipc::capture::CaptureRules::default();
        let raw = r#"{"declined":{"why":"file-type","site":"x.test","name":"p.png"},"userAgent":"UA","brands":[],"v":"0.3.0"}"#;
        let m: Message = serde_json::from_str(raw).unwrap();
        assert_eq!(
            m.declined,
            Some(Notice {
                why: "file-type".into(),
                site: "x.test".into(),
                name: "p.png".into()
            })
        );
        // No address of its own: an older host, which knows no `declined`, refuses it as an
        // address it can't download, and hands nothing over.
        assert!(!downloadable(&m.url));
        let r = answer(m, &rules);
        assert!(r.ok && r.error.is_none(), "noted, nothing delivered");
        assert_eq!(
            notice_reason("site"),
            mdm_ipc::capture::Declined::Site.reason()
        );
        assert!(notice_reason("site-access").contains("site access"));
        assert_eq!(notice_reason("x\ny"), r#""x y""#);
    }

    #[test]
    fn log_lines_name_what_was_left_and_why() {
        let reason = mdm_ipc::capture::Declined::FileType.reason();
        assert_eq!(
            left_line("Waterfox", "x.test", "p.png", reason, false),
            "browser integration: Waterfox keeps p.png from x.test: its type isn't in Options > File types"
        );
        // A private window's download is never named; nor can a name break the line.
        let private = left_line("Google Chrome", "secret.test", "s.zip", reason, true);
        assert!(!private.contains("secret") && private.contains("private window"));
        assert!(!left_line("", "x.test", "a\nb.zip", reason, false).contains('\n'));
        assert!(
            left_line("", "", "", reason, false)
                .starts_with("browser integration: a browser keeps a download:")
        );
    }

    #[test]
    fn older_extensions_messages_still_work() {
        // As the Chrome Web Store's 0.2.4 extension sends a download (no browser name from
        // getBrowserInfo, no notices), and as 0.2.4's own extension did (no brands, no version).
        let rules = mdm_ipc::capture::CaptureRules::default();
        for raw in [
            r#"{"url":"https://x.test/a.zip","finalUrl":"","referrer":"https://x.test/","cookies":"","filename":"a.zip","size":1048576,"mime":"application/zip","userAgent":"Mozilla/5.0 Chrome/141.0.0.0 Safari/537.36","brands":["Not:A-Brand","Chromium","Google Chrome"],"v":"0.2.4"}"#,
            r#"{"url":"https://x.test/a.zip","referrer":"https://x.test/","cookies":"","userAgent":"Mozilla/5.0 Chrome/141.0.0.0 Safari/537.36","filename":"a.zip"}"#,
        ] {
            let m: Message = serde_json::from_str(raw).unwrap();
            assert_eq!(verdict(&m, &rules), Ok(()), "{raw}");
            assert_eq!(to_command(m).name.as_deref(), Some("a.zip"));
        }
    }

    #[test]
    fn framing_round_trip() {
        let mut buf = Vec::new();
        write_message(&mut buf, br#"{"url":"https://x/a.zip"}"#).unwrap();
        assert_eq!(&buf[..4], &25u32.to_le_bytes());
        let mut r = &buf[..];
        assert_eq!(
            read_message(&mut r).unwrap().unwrap().unwrap(),
            br#"{"url":"https://x/a.zip"}"#
        );
        assert!(
            read_message(&mut r).unwrap().is_none(),
            "clean end of input"
        );
    }

    #[test]
    fn huge_messages_are_read_past_and_said() {
        let big = 2usize << 20;
        let mut input = (big as u32).to_le_bytes().to_vec();
        input.extend(vec![b'x'; big]);
        write_message(&mut input, br#"{"rules":true}"#).unwrap();
        let mut r = &input[..];
        assert_eq!(read_message(&mut r).unwrap(), Some(Err(big)));
        assert!(
            matches!(read_message(&mut r).unwrap(), Some(Ok(_))),
            "the next one reads"
        );
    }

    /// The extension's cookies with their domains, as `cookies.getAll` names them (its other
    /// fields ignored), reach the app; an older extension's message has none.
    #[test]
    fn cookies_keep_their_domains() {
        let m: Message = serde_json::from_str(
            r#"{"url":"https://music.youtube.com/watch?v=x","cookies":"SID=1; pref=2","userAgent":"UA",
              "cookieJar":[{"name":"SID","value":"1","domain":".youtube.com","hostOnly":false,"path":"/","secure":true,"httpOnly":true,"sameSite":"lax","session":false,"expirationDate":1893456000.5,"storeId":"0"},
                           {"name":"pref","value":"2","domain":"music.youtube.com","hostOnly":true,"path":"/","secure":false,"session":true}]}"#,
        )
        .unwrap();
        let c = to_command(m);
        assert_eq!(c.cookies.as_deref(), Some("SID=1; pref=2"));
        assert_eq!(
            c.cookie_jar,
            [
                mdm_ipc::BrowserCookie {
                    name: "SID".into(),
                    value: "1".into(),
                    domain: ".youtube.com".into(),
                    host_only: false,
                    path: "/".into(),
                    secure: true,
                    expires: Some(1893456000.5),
                },
                mdm_ipc::BrowserCookie {
                    name: "pref".into(),
                    value: "2".into(),
                    domain: "music.youtube.com".into(),
                    host_only: true,
                    path: "/".into(),
                    secure: false,
                    expires: None,
                },
            ]
        );
        // To the app and back (its command line): the same.
        let line = serde_json::to_string(&c).unwrap();
        let back: mdm_ipc::Command = serde_json::from_str(&line).unwrap();
        assert_eq!(back.cookie_jar, c.cookie_jar);
        let old: Message =
            serde_json::from_str(r#"{"url":"https://x/a.zip","cookies":"a=1"}"#).unwrap();
        let c = to_command(old);
        assert!(c.cookie_jar.is_empty());
        assert!(!serde_json::to_string(&c).unwrap().contains("cookie_jar"));
    }

    #[test]
    fn message_to_command() {
        let m: Message = serde_json::from_str(
            r#"{"url":"https://x/a.zip","referrer":"https://x/","cookies":"a=1","userAgent":"UA","filename":""}"#,
        )
        .unwrap();
        let c = to_command(m);
        assert_eq!(c.url.as_deref(), Some("https://x/a.zip"));
        assert_eq!(c.referer.as_deref(), Some("https://x/"));
        assert_eq!(c.cookies.as_deref(), Some("a=1"));
        assert_eq!(c.user_agent.as_deref(), Some("UA"));
        assert_eq!(
            c.name, None,
            "empty filename means: let Mister Download Manager decide"
        );
        assert!(!c.later && !c.silent);
        assert_eq!((c.proxy, c.proxy_pac, c.proxy_login), (None, None, false));
        assert_eq!(c.video_title, None);
    }

    /// #800: a page's media from the toolbar button's list goes with its page's title (for a
    /// file whose address names nothing), trimmed and cut; a blank one is none.
    #[test]
    fn a_media_files_page_title_goes_with_it() {
        let m: Message = serde_json::from_str(
            r#"{"url":"https://x/remote_control.php?id=1","mime":"video/mp4","title":"  A talk  "}"#,
        )
        .unwrap();
        let c = to_command(m);
        assert_eq!(c.video_title.as_deref(), Some("A talk"));
        assert_eq!(c.mime.as_deref(), Some("video/mp4"));
        let long = format!(r#"{{"url":"https://x/a","title":"{}"}}"#, "t".repeat(400));
        let c = to_command(serde_json::from_str(&long).unwrap());
        assert_eq!(c.video_title.unwrap().len(), TITLE_MAX);
        let blank: Message = serde_json::from_str(r#"{"url":"https://x/a","title":" "}"#).unwrap();
        assert_eq!(to_command(blank).video_title, None);
    }

    #[test]
    fn the_browsers_proxy_goes_with_the_download() {
        let m: Message = serde_json::from_str(
            r#"{"url":"https://x/a.zip","proxy":"socks5h://vpn.example:1080","proxyLogin":true}"#,
        )
        .unwrap();
        let c = to_command(m);
        assert_eq!(c.proxy.as_deref(), Some("socks5h://vpn.example:1080"));
        assert!(c.proxy_login);
        let pac = r#"{"url":"https://x/a.zip","proxyPac":"function FindProxyForURL(u, h) { return \"PROXY p:8080\"; }"}"#;
        assert!(
            to_command(serde_json::from_str(pac).unwrap())
                .proxy_pac
                .is_some()
        );
        // Only proxies the app can use, written as expected.
        for good in [
            "http://10.0.0.1:3128",
            "https://p.example:443",
            "socks4://[::1]:1080",
        ] {
            assert_eq!(browser_proxy(good).as_deref(), Some(good));
        }
        for bad in [
            "ftp://p:21",
            "http://p",
            "http://p:0",
            "http://user:pw@p:8080",
            "http://p:8080/path",
            "socks5://bad host:1080",
        ] {
            assert_eq!(browser_proxy(bad), None, "{bad}");
        }
        let not_pac = r#"{"url":"https://x/a.zip","proxyPac":"alert(1)"}"#;
        assert!(
            to_command(serde_json::from_str(not_pac).unwrap())
                .proxy_pac
                .is_none()
        );
    }
}
