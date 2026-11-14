//! App settings, saved with the download list (store.rs). Defaults are IDM 6.42's
//! (notes/ui-spec/options-dialog.md, scheduler-and-misc.md).

use mdm_ipc::capture::host_matches;
use serde::{Deserialize, Serialize};

/// A debrid service account (#44), as Options → Accounts keeps it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DebridAccount {
    /// Stable, for the order and for errors that name the account.
    pub id: u64,
    pub service: mdm_debrid::ServiceKind,
    pub enabled: bool,
    /// The account's name at the service, and its state as last read ("premium, 143 days").
    pub user: String,
    pub state: String,
    /// The credential's JSON (`mdm_debrid::Credential`); on disk its keychain reference, or sealed
    /// (`sealed`, secrets.rs). Never shown, logged or sent anywhere but the service.
    #[serde(with = "sealed")]
    pub secret: String,
}

impl Default for DebridAccount {
    fn default() -> Self {
        DebridAccount {
            id: 0,
            service: Default::default(),
            enabled: true,
            user: String::new(),
            state: String::new(),
            secret: String::new(),
        }
    }
}

/// Never the secret, even in a debug dump.
impl std::fmt::Debug for DebridAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebridAccount")
            .field("id", &self.id)
            .field("service", &self.service)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

impl DebridAccount {
    /// For `mdm_debrid`: None when its secret can't be read (signed out, or sealed for another
    /// Windows account).
    pub fn account(&self) -> Option<mdm_debrid::Account> {
        let credential = serde_json::from_str(&self.secret).ok()?;
        Some(mdm_debrid::Account {
            id: self.id,
            service: self.service,
            credential,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Downloads → Speed Limiter → Turn on / Turn off.
    pub speed_limit_on: bool,
    /// "Maximum download speed for one file", KBytes/sec (IDM default 10).
    pub speed_limit_kb: u64,
    /// "Always turn on Speed Limiter on … startup".
    pub limiter_on_startup: bool,
    /// Beyond IDM (D-012): one limit for all downloads together, optionally only while the
    /// user is at the computer. Off by default.
    pub global_limit_on: bool,
    pub global_limit_kb: u64,
    pub global_only_active: bool,
    /// Options → Connection → "Default max. conn. number" (IDM default 8).
    pub max_connections: u32,
    /// Beyond IDM (issue #12): "Allow more than 32 connections" raises the ceiling of that
    /// number and of the exceptions from `MAX_CONNECTIONS` to `MAX_CONNECTIONS_UNLOCKED`.
    pub many_connections: bool,
    /// Show the "Download complete" dialog (Options → Downloads).
    pub show_complete: bool,
    /// Download complete snoozed until then (unix seconds; its window's Snooze menu, #741):
    /// `show_complete` stays as it is. A snooze until MDM restarts is not saved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complete_snoozed_until: Option<i64>,
    /// List right-click → On Double click: Open (true) or Properties (IDM default).
    pub double_click_open: bool,
    /// Ask before deleting downloads ("Don't show this dialog again" clears it).
    pub confirm_delete: bool,
    /// The confirmation's "Also delete the files from disk", as last answered (IDM keeps it):
    /// its first state next time, and what happens when it no longer asks.
    pub delete_files_too: bool,
    /// Categories pane frame width (IDM: 143, frame x 6..149); dragged with the splitter.
    pub categories_width: u32,
    /// The main window as last left: position and size (x, y, width, height, in points at 100 %
    /// interface size: the system's own logical pixels), and maximized.
    pub window: Option<[i32; 4]>,
    pub window_maximized: bool,
    /// The Categories pane shown, its branches open (All, Unfinished, Finished, Queues),
    /// and the node last shown.
    pub show_categories: bool,
    pub branches: [bool; 4],
    pub view: Option<crate::app::View>,
    /// View → Dark Mode support (IDM 6.37): follow Windows' dark app mode; off = always light.
    pub dark_mode_support: bool,
    /// View → Always dark (#60): dark whatever the system says (a desktop that reports no
    /// preference, or a dark MDM on a light desktop).
    pub always_dark: bool,
    /// The look (#45): a built-in theme's id ("classic", the default) or a theme file's name in
    /// the data folder's `themes` folder (crate::themefile).
    pub theme: String,
    /// View → Draw with (#89): OpenGL or Direct3D (Windows only), used from the next start.
    /// None: MDM's default (`renderer::DEFAULT`), which may change.
    pub renderer: Option<crate::renderer::Api>,
    /// View → Interface size (#262), percent: one of `crate::scale::STEPS` (100 to 200). Every
    /// window drawn that much larger than the display's own scale makes it.
    pub interface_size: u16,
    /// View → Text (Windows): smooth text as Windows' modern apps draw it, or the classic
    /// controls' hinted, whole-pixel look, the default (`crate::theme::set_smooth_text`): text
    /// doesn't change look on an update; a dense screen can choose Smooth.
    pub smooth_text: bool,
    /// Scheduler → Download limits: "Download no more than `limits_mb` MBytes every
    /// `limits_hours` hours" (IDM defaults 200 / 5), with a warning before stopping.
    pub limits_on: bool,
    pub limits_mb: u32,
    pub limits_hours: u32,
    pub limits_warn: bool,

    // ---- Options (notes/ui-spec/options-dialog.md; IDM 6.42 defaults unless noted) ----
    /// General: start with Windows. IDM's default is on; ours is off until the user asks
    /// (a download manager shouldn't register itself silently).
    pub launch_on_startup: bool,
    /// View → Language: a catalog's code (`ar`, `pt-br`…); `None` follows Windows.
    pub language: Option<String>,
    /// The language has been chosen once (in the installer's wizard or the first start's
    /// picker); until then the first start asks (langpick.rs).
    pub language_asked: bool,
    /// The video helpers (yt-dlp, ffmpeg) have been offered once, at a start (helpers.rs).
    pub helpers_asked: bool,
    /// The browser extension is offered at each start until a browser has it, unless "Don't ask
    /// again" was pressed (extension.rs).
    pub extension_never_ask: bool,
    /// General: offer to download (Download File Info) links of the listed file types copied
    /// to the clipboard. On by default: without the browser extension, copying a link is the
    /// quickest way in.
    pub clipboard_autostart: bool,
    /// General: Minimize hides the main window, and a download's progress window, to the
    /// notification area, as closing the main window does (#63). On by default (the
    /// maintainer's choice), for existing lists too. Only where a tray shows MDM's icon.
    pub minimize_to_tray: bool,
    /// General: closing the main window keeps MDM running, hidden in the notification area (else
    /// closing quits). On by default, for existing lists too, in every copy. Only where a tray
    /// shows MDM's icon.
    pub close_to_tray: bool,
    pub mv2_extensions: bool,
    pub advanced_integration: bool,
    /// General: "Capture downloads from the following browsers" (name, on).
    pub browsers: Vec<(String, bool)>,
    /// File types: extensions taken over automatically (space separated).
    pub auto_types: String,
    /// File types: sites (space separated) and addresses never taken over.
    pub excluded_sites: String,
    pub excluded_addresses: Vec<String>,
    /// General → Download panels: the download button over videos in the browsers, and the
    /// sites it stays off (space separated; the native host reads both, mdm_ipc::capture).
    pub video_panel: bool,
    pub video_panel_sites: String,
    /// General: also take the listed types the browser would open itself (a PDF in its viewer,
    /// an MP3 in its player); holding Alt while clicking leaves one to the browser.
    pub capture_opened: bool,
    /// File types: files smaller than this (KB) stay with the browser; 0 takes any size.
    pub capture_min_kb: u64,
    /// Options → Regional: how dates and times are written.
    pub regional: crate::regional::Regional,
    /// Save to: a folder per category (category label → folder); missing = the default.
    pub category_dirs: Vec<(String, String)>,
    pub change_folder_on_last: bool,
    /// The addresses typed into Add URL and the folders downloads were saved to, newest first
    /// (the Address and Save As drop lists, as IDM's UrlHistory.txt).
    pub url_history: Vec<String>,
    pub folder_history: Vec<String>,
    pub set_creation_date: bool,
    /// Downloads → "Mark downloaded files as coming from the internet" (#813): Windows' Mark of
    /// the Web, macOS' quarantine, Linux's origin attributes (`webmark`).
    pub mark_downloads: bool,
    /// Downloads.
    pub show_start_dialog: bool,
    pub only_add_to_queue: bool,
    pub start_immediately: bool,
    pub queue_panel_later: bool,
    pub queue_panel_batch: bool,
    pub ignore_mtime: bool,
    /// "If a duplicate download link is added": 0 ask, 1 numbered name, 2 overwrite,
    /// 3 complete → show the complete dialog, else resume.
    pub duplicate_action: u8,
    /// "User-Agent for manually added downloads": the user's, or empty for the automatic one
    /// (the default browser's, #668: `mdm_engine::agent::automatic`). Saved before #668 as
    /// Internet Explorer 11's, the default then: read as empty (`own_user_agent`), which an older
    /// MDM reads as its default again.
    #[serde(deserialize_with = "automatic_agent")]
    pub user_agent: String,
    /// Connection: per-server connection limits (server, number).
    pub conn_exceptions: Vec<(String, u32)>,
    /// Proxy / Socks: 0 none, 1 system, 2 automatic script, 3 manual.
    pub proxy_from_browser: bool,
    pub proxy_mode: u8,
    pub proxy_script: String,
    pub proxy_host: String,
    pub proxy_port: String,
    pub proxy_user: String,
    /// Sealed on disk like Sites Logins' passwords (`sealed`); a plain one from before still reads.
    #[serde(with = "sealed")]
    pub proxy_pass: String,
    pub proxy_http: bool,
    pub proxy_https: bool,
    pub proxy_ftp: bool,
    /// Connection → DNS (#36): 0 the system's, 1 plain DNS, 2 DNS over TLS, 3 DNS over HTTPS.
    pub dns_mode: u8,
    /// A service of `mdm_engine::dns::PRESETS` by id, or "custom".
    pub dns_provider: String,
    /// Custom: the servers (plain, TLS) or the DoH server's addresses (optional), as typed.
    pub dns_servers: String,
    /// Custom DNS over TLS: the name the servers' certificate shows.
    pub dns_tls_name: String,
    /// Custom DNS over HTTPS: the address (`https://.../dns-query`).
    pub dns_doh_url: String,
    /// NextDNS: the configuration ID (six hex digits), or none.
    pub dns_nextdns_id: String,
    /// When the chosen DNS can't be reached, ask the system's (off: strict).
    pub dns_fallback: bool,
    /// Options → Proxy / Socks → Network adapter (#35): every connection goes through this
    /// adapter (its name as the system shows it, or one of its addresses), or none at all; ""
    /// lets the system choose.
    pub net_adapter: String,
    /// "Use FTP in PASV mode": passive data connections (the client connects; works behind
    /// NAT), else active. Named anew when FTP arrived: `ftp_pasv` was saved as false while
    /// the box was greyed.
    pub ftp_passive: bool,
    /// Sites Logins: (site/path, user, password). The passwords are written sealed for this
    /// Windows account (`sealed_logins`).
    #[serde(with = "sealed_logins")]
    pub site_logins: Vec<(String, String, String)>,
    /// Options → Accounts (#44): debrid service accounts, in the order they are asked.
    pub debrid_accounts: Vec<DebridAccount>,
    /// "Download supported file-host links through these accounts".
    pub debrid_on: bool,
    /// Options → Accounts (#44): magnet links and .torrent files go to the first account, whose
    /// service downloads them; their files then come over HTTP. No peer connections. Off: as
    /// before, through the torrent engine.
    pub debrid_torrents: bool,
    /// Sounds: download complete, download failed, queue started, queue stopped/finished.
    pub sounds: [(bool, String); 4],
    /// The local automation API (beyond IDM): status queries over the command channel. Off by
    /// default; `misterdownloadmanager-cli api on` turns it on.
    pub automation_api: bool,
    /// Options → Remote (#141, beyond IDM): paired phones and computers on the local network
    /// add and control downloads (crates/remote). Off by default.
    pub remote_on: bool,
    /// Its port.
    pub remote_port: u16,
    /// Where it listens: "" every local network address, `127.0.0.1` this computer only, or
    /// one address (`mdm_remote::addrs::chosen`).
    pub remote_listen: String,
    /// Torrents keep sharing after they finish (D-018: off; a finished torrent stops, like any
    /// download).
    pub torrent_keep_sharing: bool,
    /// Windows opens magnet links and .torrent files with MDM (D-018: opt-in, current user).
    pub torrent_associations: bool,
    /// Torrent settings → Network (#40, `torrent_config`): accept incoming connections, map
    /// the port with the router's UPnP, a fixed port (0: any, picked at each start), the DHT,
    /// finding peers on the local network.
    pub torrent_incoming: bool,
    pub torrent_upnp: bool,
    pub torrent_port: u16,
    pub torrent_dht: bool,
    pub torrent_lsd: bool,
    /// Torrents go through Proxy / Socks' SOCKS5 proxy (a manual `socks5://` one only).
    pub torrent_proxy: bool,
    /// A blocklist's address (empty: none), fetched again every `BLOCKLIST_DAYS`.
    pub torrent_blocklist: String,
    /// Stop the torrent engine when no torrent runs (it stops listening; new settings apply).
    pub torrent_stop_idle: bool,
    /// Torrent settings → "Limit uploading to [n] KBytes/sec" for all torrents together (#39;
    /// off by default: a torrent's own limit, in its progress window, still applies).
    pub torrent_upload_on: bool,
    pub torrent_upload_kb: u64,
    /// Torrent settings → seeding limits (#39), both off by default (D-018: sharing is the
    /// user's choice): stop sharing a finished torrent at a share ratio (in hundredths: 200 is
    /// 2.00), or after so many minutes of sharing.
    pub torrent_stop_ratio_on: bool,
    pub torrent_stop_ratio: u32,
    pub torrent_stop_minutes_on: bool,
    pub torrent_stop_minutes: u32,
    /// Help: look for a newer release once a week (D-020; its switch is in Check for updates:
    /// Options → General has no free row in IDM's layout).
    pub update_check: bool,
    /// When the weekly check last ran (Unix seconds; 0: never).
    pub update_checked: i64,
    /// The user's categories (D-021), after IDM's and Torrents.
    pub custom_categories: Vec<crate::model::CustomCategory>,
    /// The built-in categories the user renamed, gave other file types or deleted (#93); the
    /// others are as shipped. Their folders stay in `category_dirs`.
    pub builtin_categories: Vec<crate::model::BuiltinEdit>,
    /// Tasks → Show drop target: shown, and where (screen, logical points).
    pub drop_target: bool,
    pub drop_target_pos: Option<(i32, i32)>,
    /// Download video from a page: the subtitles last chosen (#68), offered again for the
    /// next video when it has that language (none at first).
    pub video_subs: mdm_tools::video::Subs,
    /// Download video from a page → Embed chapters (#827): a video's chapters go into its file
    /// (with ffmpeg). On; a settings file from before has none: on.
    pub video_chapters: bool,
    /// Options → Sites Logins → Cookies for video sites (#229): the browser whose cookies
    /// yt-dlp reads on this computer when the extension passed none (yt-dlp's name: `firefox`,
    /// `chrome`...; empty: off, the default), and its profile (a name or a folder; empty: the
    /// browser's last used). A settings file from before has neither: off.
    pub video_cookies: String,
    pub video_cookies_profile: String,
    /// Options → Archives (#37): extract an archive when its download completes (a Link grabber
    /// package says so itself). Off, as a browser leaves archives alone.
    pub extract_auto: bool,
    /// Then move the archive (every volume of it) to the Recycle Bin (the Trash). Off.
    pub extract_delete: bool,
    /// Into a folder named after the archive: 0 when it holds several things at its top (smart),
    /// 1 always, 2 never.
    pub extract_subfolder: u8,
    /// Link grabber: look each link up (name, size, online) when it arrives, as IDM's "Download
    /// all links" does. On.
    pub grabber_check: bool,
    /// Link grabber: web pages hidden (IDM's "Hide HTML files"). On.
    pub grabber_hide_pages: bool,
    /// Click'n'Load (#37): a page's button posts its links to 127.0.0.1:9666, shown in Link
    /// grabber. Off by default: any page may post there.
    pub cnl_on: bool,
    /// Click'n'Load: the sites asked about, each allowed (true) or blocked, by host.
    pub cnl_sites: Vec<(String, bool)>,
}

/// Options → File types defaults (IDM 6.42), shared with the browser host (mdm_ipc::capture).
pub use mdm_ipc::capture::{DEFAULT_AUTO_TYPES, DEFAULT_EXCLUDED_SITES};
/// The browsers IDM 6.42 lists, plus Firefox's forks MDM tells apart (#41; capture::browser_named),
/// all captured on.
pub const BROWSERS: [&str; 12] = [
    "Apple Safari",          // not translated: a product name
    "Brave Browser",         // not translated: a product name
    "Floorp",                // not translated: a product name
    "Google Chrome",         // not translated: a product name
    "Internet Explorer",     // not translated: a product name
    "LibreWolf",             // not translated: a product name
    "Microsoft Edge",        // not translated: a product name
    "Microsoft Edge Legacy", // not translated: a product name
    "Mozilla Firefox",       // not translated: a product name
    "Opera",                 // not translated: a product name
    "Waterfox",              // not translated: a product name
    "Zen Browser",           // not translated: a product name
];

impl Settings {
    /// A list saved before a browser was known gets it, captured (as an unlisted browser
    /// already is, capture::CaptureRules::captures), so Options shows it.
    pub fn list_known_browsers(&mut self) {
        for b in BROWSERS {
            if !self.browsers.iter().any(|(n, _)| n == b) {
                self.browsers.push((b.to_owned(), true));
            }
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            speed_limit_on: false,
            speed_limit_kb: 10,
            limiter_on_startup: false,
            global_limit_on: false,
            global_limit_kb: 500,
            global_only_active: false,
            max_connections: 8,
            many_connections: false,
            show_complete: true,
            complete_snoozed_until: None,
            double_click_open: false,
            confirm_delete: true,
            delete_files_too: false,
            categories_width: 143,
            window: None,
            window_maximized: false,
            show_categories: true,
            branches: [true, false, false, false],
            view: None,
            dark_mode_support: true,
            always_dark: false,
            theme: "classic".into(),
            renderer: None,
            interface_size: 100,
            smooth_text: false,
            limits_on: false,
            limits_mb: 200,
            limits_hours: 5,
            limits_warn: true,
            launch_on_startup: false,
            language: None,
            language_asked: false,
            helpers_asked: false,
            extension_never_ask: false,
            clipboard_autostart: true,
            minimize_to_tray: true,
            close_to_tray: true,
            mv2_extensions: true,
            advanced_integration: true,
            browsers: BROWSERS.iter().map(|b| (b.to_string(), true)).collect(),
            auto_types: DEFAULT_AUTO_TYPES.into(),
            excluded_sites: DEFAULT_EXCLUDED_SITES.into(),
            excluded_addresses: Vec::new(),
            video_panel: true,
            video_panel_sites: String::new(),
            capture_opened: true,
            capture_min_kb: 0,
            regional: crate::regional::Regional::default(),
            category_dirs: Vec::new(),
            change_folder_on_last: true,
            url_history: Vec::new(),
            folder_history: Vec::new(),
            set_creation_date: false,
            mark_downloads: true,
            show_start_dialog: true,
            only_add_to_queue: false,
            start_immediately: true,
            queue_panel_later: true,
            queue_panel_batch: true,
            ignore_mtime: false,
            duplicate_action: 0,
            user_agent: String::new(),
            conn_exceptions: Vec::new(),
            proxy_from_browser: true,
            proxy_mode: 0,
            proxy_script: String::new(),
            proxy_host: String::new(),
            proxy_port: String::new(),
            proxy_user: String::new(),
            proxy_pass: String::new(),
            proxy_http: false,
            proxy_https: false,
            proxy_ftp: false,
            dns_mode: 0,
            dns_provider: "cloudflare".into(),
            dns_servers: String::new(),
            dns_tls_name: String::new(),
            dns_doh_url: String::new(),
            dns_nextdns_id: String::new(),
            dns_fallback: false,
            net_adapter: String::new(),
            ftp_passive: true,
            site_logins: Vec::new(),
            debrid_accounts: Vec::new(),
            debrid_on: true,
            debrid_torrents: false,
            sounds: Default::default(),
            automation_api: false,
            remote_on: false,
            remote_port: mdm_remote::PORT,
            remote_listen: String::new(),
            torrent_keep_sharing: false,
            torrent_associations: false,
            torrent_incoming: true,
            torrent_upnp: true,
            torrent_port: 0,
            torrent_dht: true,
            torrent_lsd: true,
            torrent_proxy: false,
            torrent_blocklist: String::new(),
            torrent_stop_idle: true,
            torrent_upload_on: false,
            torrent_upload_kb: 100,
            torrent_stop_ratio_on: false,
            torrent_stop_ratio: 200,
            torrent_stop_minutes_on: false,
            torrent_stop_minutes: 1440,
            update_check: true,
            update_checked: 0,
            custom_categories: Vec::new(),
            builtin_categories: Vec::new(),
            drop_target: false,
            drop_target_pos: None,
            video_subs: Default::default(),
            video_chapters: true,
            video_cookies: String::new(),
            video_cookies_profile: String::new(),
            extract_auto: false,
            extract_delete: false,
            extract_subfolder: 0,
            grabber_check: true,
            grabber_hide_pages: true,
            cnl_on: false,
            cnl_sites: Vec::new(),
        }
    }
}

impl Settings {
    /// The settings as the list file writes them (#44, secrets.rs): each secret's field holds
    /// its keychain reference once the keychain holds it, else the secret sealed as before
    /// (and the keychain is asked to take it). Serialized inside `secrets::prepared`, where these
    /// fields go as they are. Also called at start: secrets from before go to the keychain then.
    pub fn for_disk(&self) -> Settings {
        let vault = crate::secrets::vault();
        let mut pass = vault.pass();
        let mut disk = self.clone();
        let mut keys = std::collections::HashSet::new();
        let mut kept = |key: String, label: &dyn Fn() -> String, secret: &str| {
            // A key once (two logins for the same site and user get one each).
            let mut k = key.clone();
            let mut n = 2;
            while !keys.insert(k.clone()) {
                k = format!("{key}-{n}");
                n += 1;
            }
            pass.secret(&k, label, secret)
                .unwrap_or_else(|| sealed::seal(secret))
        };
        // Their names in the system's keychain (Credential Manager, the keyring).
        disk.proxy_pass = kept(
            "proxy".into(),
            &|| {
                format!(
                    "Mister Download Manager: {} {}:{}",
                    // i18n: the proxy's password in the system's keychain (Credential Manager, the keyring): its name there, followed by the proxy's address
                    crate::i18n::t("proxy server"),
                    self.proxy_host,
                    self.proxy_port
                )
            },
            &self.proxy_pass,
        );
        for (a, d) in self.debrid_accounts.iter().zip(&mut disk.debrid_accounts) {
            d.secret = kept(
                format!("debrid/{}", a.id),
                &|| format!("Mister Download Manager: {} ({})", a.service.name(), a.user),
                &a.secret,
            );
        }
        for ((site, user, pw), d) in self.site_logins.iter().zip(&mut disk.site_logins) {
            d.2 = kept(
                format!(
                    "login/{:016x}",
                    fnv(&format!(
                        "{site}
{user}"
                    ))
                ),
                &|| format!("Mister Download Manager: {site} ({user})"),
                pw,
            );
        }
        pass.finish();
        disk
    }

    /// Options → Archives' folder choice, for mdm-extract.
    pub fn extract_into(&self) -> mdm_extract::Subfolder {
        match self.extract_subfolder {
            1 => mdm_extract::Subfolder::Always,
            2 => mdm_extract::Subfolder::Never,
            _ => mdm_extract::Subfolder::Smart,
        }
    }
}

/// FNV-1a: a stable hash (std's may change between builds), for keychain keys.
pub(crate) fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The most entries a drop list keeps.
pub const HISTORY: usize = 20;

/// Put `value` first in a drop list's history: once, newest first, at most `HISTORY`.
pub fn remember(list: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    list.retain(|v| v != value);
    list.insert(0, value.to_owned());
    list.truncate(HISTORY);
}

/// A saved "User-Agent for manually added downloads": MDM's former default (Internet Explorer
/// 11's, before #668) or nothing is the automatic one (empty).
fn automatic_agent<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let saved = String::deserialize(d)?;
    Ok(if saved.trim() == mdm_engine::agent::IE11_USER_AGENT {
        String::new()
    } else {
        saved
    })
}

impl Settings {
    /// The User-Agent the user set for links added by hand; None: the automatic one (the
    /// default browser's, #668).
    pub fn own_user_agent(&self) -> Option<&str> {
        let ua = self.user_agent.trim();
        (!ua.is_empty() && ua != mdm_engine::agent::IE11_USER_AGENT).then_some(ua)
    }

    /// `live` with the settings a dialog changed (`from` → `to`) put in, field by field: what
    /// changed elsewhere while it was open (the toolbar's Speed Limiter, Torrent settings, the
    /// Categories pane) is kept, not reverted to the copy the dialog started from.
    pub fn merge_changes(live: &Settings, from: &Settings, to: &Settings) -> Settings {
        let value = |s: &Settings| serde_json::to_value(s).unwrap_or_default();
        let (mut out, before, after) = (value(live), value(from), value(to));
        if let (Some(out), Some(before), Some(after)) =
            (out.as_object_mut(), before.as_object(), after.as_object())
        {
            for (k, v) in after {
                if before.get(k) != Some(v) {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
        serde_json::from_value(out).unwrap_or_else(|_| to.clone())
    }

    /// Options → Save to: the category's folder (its own choice, else `Downloads\<Category>`).
    pub fn category_dir(
        &self,
        cat: crate::model::Category,
        downloads: &std::path::Path,
    ) -> std::path::PathBuf {
        crate::model::category_folder(cat, downloads, &self.category_dirs)
    }

    /// Options → Proxy / Socks for the engine. An automatic configuration script (PAC) is run
    /// once it has been fetched (pacfetch.rs); until then, the system's settings.
    /// The SOCKS5 proxy torrents may use: Proxy / Socks' manual one when its address is a
    /// `socks5://` (or `socks5h://`) one, with its login; librqbit takes no other kind.
    pub fn socks5_proxy(&self) -> Option<String> {
        let host = self.proxy_host.trim();
        let (scheme, rest) = host.split_once("://")?;
        if self.proxy_mode != 3
            || !matches!(scheme.to_ascii_lowercase().as_str(), "socks5" | "socks5h")
        {
            return None;
        }
        let mut url = url::Url::parse(&format!("{}://{rest}", scheme.to_ascii_lowercase())).ok()?;
        if let Ok(port) = self.proxy_port.trim().parse::<u16>() {
            let _ = url.set_port(Some(port));
        }
        if !self.proxy_user.is_empty() {
            let _ = url.set_username(&self.proxy_user);
            let _ = url.set_password(Some(&self.proxy_pass));
        }
        Some(url.as_str().trim_end_matches('/').to_owned())
    }

    /// The torrent engine as Torrent settings say (#40), its data in `state_dir`. None when they
    /// say to use the proxy and Proxy / Socks has no SOCKS5 one: torrents then don't start, never
    /// going around the proxy unasked.
    pub fn torrent_config(&self, state_dir: std::path::PathBuf) -> Option<mdm_torrent::Config> {
        let proxy = match (self.torrent_proxy, self.socks5_proxy()) {
            (true, None) => return None,
            (true, proxy) => proxy,
            (false, _) => None,
        };
        let blocklist = (!self.torrent_blocklist.trim().is_empty())
            .then(|| state_dir.join(crate::torrents::BLOCKLIST_FILE));
        Some(mdm_torrent::Config {
            listen: self.torrent_incoming,
            upnp: self.torrent_upnp,
            port: (self.torrent_port != 0).then_some(self.torrent_port),
            dht: self.torrent_dht,
            lsd: self.torrent_lsd,
            proxy,
            blocklist,
            // The chosen network adapter (#35, #143); the session takes it again when it starts.
            bind: crate::torrents::route_bind(&own_route()).ok().flatten(),
            ..mdm_torrent::Config::new(state_dir)
        })
    }

    pub fn proxy(&self) -> mdm_engine::Proxy {
        use mdm_engine::Proxy;
        match self.proxy_mode {
            1 => Proxy::System,
            2 => crate::pacfetch::script(&self.proxy_script)
                .map_or(Proxy::System, |script| Proxy::Pac { script }),
            3 if !self.proxy_host.trim().is_empty() => {
                let host = self.proxy_host.trim();
                let url = if host.contains("://") {
                    host.to_owned()
                } else {
                    format!("http://{host}")
                };
                let url = match self.proxy_port.trim() {
                    "" => url,
                    port => format!("{url}:{port}"),
                };
                Proxy::Manual {
                    url,
                    login: (!self.proxy_user.is_empty())
                        .then(|| (self.proxy_user.clone(), self.proxy_pass.clone())),
                    http: self.proxy_http,
                    https: self.proxy_https,
                }
            }
            // "Use proxy/socks from a browser's request": browsers use Windows' proxy settings,
            // so with no proxy of our own so do we (none set there: direct, as before).
            _ if self.proxy_from_browser => Proxy::System,
            _ => Proxy::None,
        }
    }

    /// The proxy for a download that came with `context` (#42): the browser's own, when it used
    /// one for it (a VPN or proxy extension, or its settings) and "Use proxy/socks from a
    /// browser's request" is on; else [`Self::proxy`]. A proxy the extension logs in to is
    /// refused (`browser_proxy_refused`), never replaced by a direct connection.
    pub fn proxy_for(&self, context: &mdm_ipc::Command) -> mdm_engine::Proxy {
        use mdm_engine::Proxy;
        if self.proxy_from_browser {
            if let Some(url) = &context.proxy {
                return Proxy::Manual {
                    url: url.clone(),
                    login: None,
                    http: true,
                    https: true,
                };
            }
            if let Some(script) = &context.proxy_pac {
                return Proxy::Pac {
                    script: std::sync::Arc::from(script.as_str()),
                };
            }
        }
        self.proxy()
    }

    /// Why a download from the browser can't start (#42): its proxy asks for a login that the
    /// VPN or proxy extension gives, which MDM can't have. Going around it would show the
    /// user's own address, so the download stops and says so.
    pub fn browser_proxy_refused(&self, context: &mdm_ipc::Command) -> Option<String> {
        (self.proxy_from_browser && context.proxy_login).then(|| {
            // i18n: an error: the browser's VPN or proxy extension logs in to its proxy itself, and MDM can't; Options → Proxy / Socks → Network adapter is where an adapter can be chosen
            crate::i18n::t(
                "the browser's proxy (a VPN or proxy extension) asks for a login only that extension has: use the VPN's own app, or choose its network adapter in Options → Proxy / Socks",
            )
            .to_owned()
        })
    }

    /// Options → Connection → DNS for the engine. A setting that can't work (no server typed,
    /// an address that isn't one) gives the system's DNS; Options won't take one with OK.
    pub fn dns(&self) -> mdm_engine::DnsChoice {
        dns_choice(
            self.dns_mode,
            &self.dns_provider,
            &self.dns_servers,
            &self.dns_tls_name,
            &self.dns_doh_url,
            &self.dns_nextdns_id,
            self.dns_fallback,
        )
        .unwrap_or_default()
    }

    /// The most connections Options → Connection lets a download use: 32, or 128 once more are
    /// allowed.
    pub fn connection_limit(&self) -> u32 {
        if self.many_connections {
            mdm_engine::download::MAX_CONNECTIONS_UNLOCKED
        } else {
            mdm_engine::download::MAX_CONNECTIONS
        }
    }

    /// Connections for a download from `url`: a server exception (Options → Connection), else
    /// the default; never more than `connection_limit` (a hand-edited file can hold anything).
    pub fn connections_for(&self, url: &url::Url) -> u32 {
        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        self.conn_exceptions
            .iter()
            .find(|(server, _)| host_matches(&host, server))
            .map_or(self.max_connections, |(_, n)| *n)
            .clamp(1, self.connection_limit())
    }

    /// The Sites Logins entry for `url`: its site/path is a prefix of the address (without the
    /// scheme); `*.` matches subdomains.
    pub fn site_login(&self, url: &url::Url) -> Option<(String, String)> {
        let host = url.host_str()?.to_ascii_lowercase();
        let path = url.path();
        self.site_logins
            .iter()
            .find(|(site, _, _)| {
                let site = site.trim();
                let site = site.split_once("://").map_or(site, |(_, rest)| rest);
                let (site_host, site_path) = site.split_once('/').unwrap_or((site, ""));
                host_matches(&host, site_host)
                    && path
                        .trim_start_matches('/')
                        .starts_with(site_path.trim_start_matches('/'))
            })
            .map(|(_, user, pass)| (user.clone(), pass.clone()))
    }

    /// Should a copied link named `name` be offered? Fewer types than a browser hands over
    /// (`mdm_ipc::capture::clipboard_type`).
    pub fn clipboard_type(&self, name: &str) -> bool {
        mdm_ipc::capture::clipboard_type(&self.auto_types, name)
    }

    /// Set (or forget, with `None`) the category's own folder.
    pub fn set_category_dir(&mut self, cat: crate::model::Category, dir: Option<String>) {
        if let crate::model::Category::Custom(id) = cat {
            if let Some(c) = self.custom_categories.iter_mut().find(|c| c.id == id)
                && c.folder != dir
            {
                c.folder = dir;
                crate::model::set_custom(&self.custom_categories);
            }
            return;
        }
        // Under the built-in's shipped name: a rename keeps its folder.
        let key = cat.key();
        self.category_dirs.retain(|(c, _)| *c != *key);
        if let Some(d) = dir {
            self.category_dirs.push((key.into_owned(), d));
        }
    }

    /// The folder just saved into becomes the category's, when it is another one (an unchanged
    /// folder stays "the default", following Downloads). True when the folder changed.
    pub fn keep_category_dir(
        &mut self,
        cat: crate::model::Category,
        dir: &std::path::Path,
        downloads: &std::path::Path,
    ) -> bool {
        if dir.as_os_str().is_empty() || dir == self.category_dir(cat, downloads) {
            return false;
        }
        self.set_category_dir(cat, Some(dir.display().to_string()));
        true
    }

    /// Both category registries (the user's and the built-in ones' edits) from these settings,
    /// for code that only has a `Category`.
    pub fn publish_categories(&self) {
        crate::model::set_custom(&self.custom_categories);
        crate::model::set_builtin(&self.builtin_categories);
    }

    /// The per-download limit new downloads start with, bytes/s.
    pub fn speed_limit(&self) -> Option<u64> {
        self.speed_limit_on
            .then_some(self.speed_limit_kb.max(1) * 1024)
    }

    /// Torrent settings' seeding limits: the share ratio and the seconds of sharing at which a
    /// finished torrent stops being shared (None: that limit is off).
    pub fn sharing_limits(&self) -> crate::torrents::SharingLimits {
        crate::torrents::SharingLimits {
            ratio: self
                .torrent_stop_ratio_on
                .then(|| f64::from(self.torrent_stop_ratio.max(1)) / 100.0),
            secs: self
                .torrent_stop_minutes_on
                .then(|| u64::from(self.torrent_stop_minutes.max(1)) * 60),
        }
    }

    /// Torrent settings' upload limit for all torrents together, bytes/s (None: off).
    pub fn torrent_upload_limit(&self) -> Option<u64> {
        self.torrent_upload_on
            .then_some(self.torrent_upload_kb.max(1) * 1024)
    }
}

/// DNS mode numbers (`Settings::dns_mode`) as the engine's.
pub fn dns_mode(n: u8) -> Option<mdm_engine::dns::Mode> {
    use mdm_engine::dns::Mode;
    match n {
        1 => Some(Mode::Plain),
        2 => Some(Mode::Tls),
        3 => Some(Mode::Https),
        _ => None,
    }
}

/// The engine's DNS choice from Options → DNS's fields; `Err` says what is wrong.
pub fn dns_choice(
    mode: u8,
    provider: &str,
    servers: &str,
    tls_name: &str,
    doh_url: &str,
    nextdns_id: &str,
    fallback: bool,
) -> Result<mdm_engine::DnsChoice, String> {
    use mdm_engine::dns::{Dns, Mode, Preset, parse_servers, valid_nextdns_profile};
    let Some(m) = dns_mode(mode) else {
        return Ok(mdm_engine::DnsChoice::default());
    };
    let how = match m {
        // i18n: a way of asking a DNS, in its name in messages: "Cloudflare (plain DNS)"
        Mode::Plain => crate::i18n::t("plain DNS"),
        // i18n: a way of asking a DNS, in its name in messages: "Cloudflare (DNS over TLS)"
        Mode::Tls => crate::i18n::t("DNS over TLS"),
        // i18n: a way of asking a DNS, in its name in messages: "Cloudflare (DNS over HTTPS)"
        Mode::Https => crate::i18n::t("DNS over HTTPS"),
    };
    if let Some(p) = Preset::by_id(provider) {
        if !p.offers(m) {
            return Err(crate::i18n::fmt(
                // i18n: Options → DNS: this service can't be asked this way; {how} is e.g. "plain DNS"
                crate::i18n::t("{name} isn't offered as {how}."),
                &[("name", p.name), ("how", how)],
            ));
        }
        if !valid_nextdns_profile(nextdns_id) {
            return Err(
                // i18n: Options → DNS: the NextDNS configuration ID typed isn't one
                crate::i18n::t("A NextDNS configuration ID is six letters and digits (0-9, a-f).")
                    .to_owned(),
            );
        }
        return Ok(p.choice(m, nextdns_id, fallback, format!("{} ({how})", p.name)));
    }
    // i18n: Options → DNS: the name of a DNS the user typed in, in messages: "your DNS (DNS over TLS)"
    let label = crate::i18n::fmt(crate::i18n::t("your DNS ({how})"), &[("how", how)]);
    let bad = |what: String| {
        crate::i18n::fmt(
            // i18n: Options → DNS: a server address typed isn't an IP address; {what} is what was typed
            crate::i18n::t("\"{what}\" isn't an IP address."),
            &[("what", &what)],
        )
    };
    // i18n: Options → DNS: no server address was typed
    let none = || crate::i18n::t("Type at least one server address.").to_owned();
    let dns = match m {
        Mode::Plain => {
            let s = parse_servers(servers, 53).map_err(bad)?;
            if s.is_empty() {
                return Err(none());
            }
            Dns::Plain(s)
        }
        Mode::Tls => {
            let s = parse_servers(servers, 853).map_err(bad)?;
            if s.is_empty() {
                return Err(none());
            }
            let name = tls_name.trim();
            if name.is_empty() {
                return Err(
                    // i18n: Options → DNS over TLS: the server's name (as its certificate shows it) is missing
                    crate::i18n::t("Type the name the server's certificate shows.").to_owned(),
                );
            }
            Dns::Tls {
                name: name.to_owned(),
                servers: s,
            }
        }
        Mode::Https => {
            let url = doh_url.trim();
            if !url.starts_with("https://") || url::Url::parse(url).is_err() {
                // i18n: Options → DNS over HTTPS: the address typed isn't a valid https:// address
                return Err(crate::i18n::t("Type the server's https:// address.").to_owned());
            }
            Dns::Https {
                url: url.to_owned(),
                bootstrap: parse_servers(servers, 0)
                    .map_err(bad)?
                    .into_iter()
                    .map(|a| a.ip())
                    .collect(),
            }
        }
    };
    Ok(mdm_engine::DnsChoice {
        dns,
        fallback,
        label,
    })
}

/// The proxy MDM's own downloads go through (its helpers, its updates, a crash report's
/// symbols): Options → Proxy / Socks, kept in step by the app ([`share_proxy`]); Windows' own
/// settings in a process without them (the crash monitor).
static OWN_PROXY: std::sync::RwLock<Option<mdm_engine::Proxy>> = std::sync::RwLock::new(None);
/// The same for Options → Connection → DNS ([`share_dns`]).
static OWN_DNS: std::sync::RwLock<Option<mdm_engine::DnsChoice>> = std::sync::RwLock::new(None);

/// Keep [`own_net`] on the settings' proxy (cheap when it didn't change).
pub fn share_proxy(proxy: mdm_engine::Proxy) {
    // yt-dlp and gallery-dl go through it too (#363).
    mdm_tools::proc::set_proxy(proxy.clone());
    if OWN_PROXY.read().unwrap().as_ref() != Some(&proxy) {
        *OWN_PROXY.write().unwrap() = Some(proxy);
    }
}

/// Keep [`own_net`] on the settings' DNS (cheap when it didn't change), and the helpers' relay
/// (#35: the servers of video pages and galleries are looked up with it while an adapter is
/// chosen).
pub fn share_dns(dns: mdm_engine::DnsChoice) {
    if OWN_DNS.read().unwrap().as_ref() != Some(&dns) {
        *OWN_DNS.write().unwrap() = Some(dns.clone());
    }
    mdm_tools::proc::set_dns(dns);
}

static OWN_ROUTE: std::sync::RwLock<mdm_engine::Route> =
    std::sync::RwLock::new(mdm_engine::Route::Any);

/// Keep [`own_net`], yt-dlp and the media-server checks on the chosen network adapter as it is
/// now (#35; the adapter watch in `netwatch`).
pub fn share_route(route: mdm_engine::Route) {
    if *OWN_ROUTE.read().unwrap() != route {
        *OWN_ROUTE.write().unwrap() = route.clone();
    }
    mdm_tools::proc::set_route(route);
}

impl Settings {
    /// Options → Sites Logins → Cookies for video sites, as yt-dlp is given it (None: off, or
    /// a browser yt-dlp doesn't read).
    pub fn video_cookies(&self) -> Option<mdm_tools::proc::BrowserCookies> {
        mdm_tools::proc::BrowserCookies::new(&self.video_cookies, &self.video_cookies_profile)
    }
}

/// Keep every yt-dlp run on Options' cookies for video sites (cheap when they didn't change),
/// and every video started on Embed chapters (#827).
pub fn share_video_settings(s: &Settings) {
    mdm_tools::proc::set_browser_cookies(s.video_cookies());
    mdm_tools::video::set_chapters(s.video_chapters);
}

/// The network adapter MDM's connections go through now (#35).
pub fn own_route() -> mdm_engine::Route {
    OWN_ROUTE.read().unwrap().clone()
}

/// How MDM downloads its own files: its engine, as itself, through the user's proxy, DNS and
/// network adapter.
pub fn own_net() -> mdm_engine::Options {
    let proxy = OWN_PROXY
        .read()
        .unwrap()
        .clone()
        .unwrap_or(mdm_engine::Proxy::System);
    mdm_engine::Options {
        dns: OWN_DNS.read().unwrap().clone().unwrap_or_default(),
        route: own_route(),
        ..mdm_tools::net(proxy)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// #668: the User-Agent for links added by hand is the default browser's unless one is set.
    /// Saved before, MDM's former default (Internet Explorer 11's) or nothing reads as automatic;
    /// automatic is saved empty, which an older MDM reads as its own default again.
    #[test]
    fn the_user_agent_is_automatic_unless_one_is_set() {
        use super::Settings;
        let ie11 = mdm_engine::agent::IE11_USER_AGENT;
        assert_eq!(Settings::default().user_agent, "");
        assert_eq!(Settings::default().own_user_agent(), None);
        for saved in [
            serde_json::json!({ "user_agent": ie11 }),
            serde_json::json!({ "user_agent": format!(" {ie11} ") }),
            serde_json::json!({ "user_agent": "" }),
            serde_json::json!({ "user_agent": "   " }),
            serde_json::json!({}),
        ] {
            let s: Settings = serde_json::from_value(saved.clone()).unwrap();
            assert_eq!(s.own_user_agent(), None, "{saved}");
            let again = serde_json::to_value(&s).unwrap();
            assert_eq!(
                again["user_agent"].as_str().map(str::trim),
                Some(""),
                "{saved}"
            );
        }
        let own: Settings =
            serde_json::from_value(serde_json::json!({ "user_agent": " MyAgent/1.0 " })).unwrap();
        assert_eq!(own.own_user_agent(), Some("MyAgent/1.0"));
        let again: Settings = serde_json::from_value(serde_json::to_value(&own).unwrap()).unwrap();
        assert_eq!(again.own_user_agent(), Some("MyAgent/1.0"));
    }

    /// #475: a folder chosen for a video is the Video category's next time; the category's own
    /// folder stays "the default", following Downloads, and other categories stay as they were.
    #[test]
    fn a_folder_saved_into_becomes_the_categorys() {
        use crate::model::Category;
        let root = std::env::temp_dir();
        let downloads = root.join("Downloads");
        let downloads = downloads.as_path();
        let mut s = super::Settings::default();
        let default_video = s.category_dir(Category::Video, downloads);
        assert!(!s.keep_category_dir(Category::Video, &default_video, downloads));
        assert!(s.category_dirs.is_empty(), "the default stays the default");
        assert!(!s.keep_category_dir(Category::Video, Path::new(""), downloads));
        let chosen = root.join("clips");
        let chosen = chosen.as_path();
        assert!(s.keep_category_dir(Category::Video, chosen, downloads));
        assert_eq!(s.category_dir(Category::Video, downloads), chosen);
        assert_ne!(s.category_dir(Category::Music, downloads), chosen);
        assert!(!s.keep_category_dir(Category::Video, chosen, downloads));
    }

    #[test]
    fn a_dialog_keeps_what_changed_elsewhere_meanwhile() {
        let from = super::Settings::default();
        let mut live = from.clone();
        live.speed_limit_on = !from.speed_limit_on; // the toolbar, while Options was open
        let mut to = from.clone();
        to.max_connections = 3; // Options
        let out = super::Settings::merge_changes(&live, &from, &to);
        assert_eq!(out.speed_limit_on, live.speed_limit_on);
        assert_eq!(out.max_connections, 3);
    }

    use super::*;

    /// #40: torrents use only a manual SOCKS5 proxy, with its login; asked for one and given
    /// none, they don't start (no config) rather than go around it.
    #[test]
    fn torrents_use_a_socks5_proxy_or_none() {
        let dir = std::path::PathBuf::from("state");
        let mut s = Settings {
            proxy_mode: 3,
            proxy_host: "socks5://127.0.0.1".into(),
            proxy_port: "1080".into(),
            proxy_user: "ann".into(),
            proxy_pass: "p@ss".into(),
            ..Settings::default()
        };
        assert_eq!(
            s.socks5_proxy().as_deref(),
            Some("socks5://ann:p%40ss@127.0.0.1:1080")
        );
        assert_eq!(
            s.torrent_config(dir.clone()).unwrap().proxy,
            None,
            "not asked"
        );
        s.torrent_proxy = true;
        assert!(s.torrent_config(dir.clone()).unwrap().proxy.is_some());
        s.proxy_host = "http://127.0.0.1".into();
        assert_eq!(s.socks5_proxy(), None, "an HTTP proxy isn't for torrents");
        assert!(
            s.torrent_config(dir.clone()).is_none(),
            "asked, none there: not started"
        );
        s.torrent_port = 51413;
        s.torrent_proxy = false;
        let c = s.torrent_config(dir).unwrap();
        assert_eq!(
            (c.port, c.listen, c.dht, c.lsd),
            (Some(51413), true, true, true)
        );
    }

    /// #41: a list saved before Zen was known gets it, captured; a choice made is kept.
    #[test]
    fn saved_browser_lists_get_new_browsers() {
        let mut s = Settings {
            browsers: vec![("Google Chrome".into(), false)],
            ..Settings::default()
        };
        s.list_known_browsers();
        assert_eq!(s.browsers[0], ("Google Chrome".into(), false));
        assert!(s.browsers.contains(&("Zen Browser".into(), true)));
        assert_eq!(s.browsers.len(), BROWSERS.len());
        s.list_known_browsers();
        assert_eq!(s.browsers.len(), BROWSERS.len(), "each once");
    }

    #[test]
    fn history_keeps_each_entry_once_newest_first() {
        let mut h = Vec::new();
        for v in ["a", "b", "a", "  ", "c"] {
            remember(&mut h, v);
        }
        assert_eq!(h, ["c", "a", "b"]);
        for i in 0..30 {
            remember(&mut h, &i.to_string());
        }
        assert_eq!(h.len(), HISTORY);
        assert_eq!(h[0], "29");
    }

    #[test]
    fn site_logins_match_by_host_and_path() {
        let s = Settings {
            site_logins: vec![
                ("files.example.com/private".into(), "a".into(), "1".into()),
                ("*.corp.net".into(), "b".into(), "2".into()),
            ],
            ..Settings::default()
        };
        let url = |u: &str| url::Url::parse(u).unwrap();
        assert_eq!(
            s.site_login(&url("https://files.example.com/private/x.zip")),
            Some(("a".into(), "1".into()))
        );
        assert_eq!(
            s.site_login(&url("https://files.example.com/public/x.zip")),
            None
        );
        assert_eq!(
            s.site_login(&url("http://dl.corp.net/y")),
            Some(("b".into(), "2".into()))
        );
    }

    #[test]
    fn listed_file_types() {
        let s = Settings::default();
        let listed = |name| mdm_ipc::capture::type_listed(&s.auto_types, name);
        assert!(listed("setup.EXE") && listed("a.r01") && listed("x.zip"));
        assert!(!listed("page.html") && !listed("noext"));
    }

    #[test]
    fn connection_exceptions_and_proxy() {
        let mut s = Settings {
            conn_exceptions: vec![("ftp.slow.org".into(), 1)],
            ..Settings::default()
        };
        let url = |u: &str| url::Url::parse(u).unwrap();
        assert_eq!(s.connections_for(&url("ftp://ftp.slow.org/a")), 1);
        assert_eq!(s.connections_for(&url("https://fast.org/a")), 8);
        // Beyond 32 only once allowed (issue #12), and never beyond 128.
        s.max_connections = 100;
        s.conn_exceptions.push(("big.org".into(), 500));
        assert_eq!(s.connections_for(&url("https://fast.org/a")), 32);
        assert_eq!(s.connections_for(&url("https://big.org/a")), 32);
        s.many_connections = true;
        assert_eq!(s.connections_for(&url("https://fast.org/a")), 100);
        assert_eq!(s.connections_for(&url("https://big.org/a")), 128);
        // An older settings file has no switch: locked.
        let old: Settings = serde_json::from_str(r#"{"max_connections": 64}"#).unwrap();
        assert!(!old.many_connections);
        // Nor cookies for video sites (#229): off.
        assert_eq!(old.video_cookies(), None);
        assert_eq!(Settings::default().video_cookies(), None);
        assert_eq!(old.connections_for(&url("https://fast.org/a")), 32);
        // IDM's default: no proxy, but "use the browser's proxy" ticked — the browser's is
        // Windows' (none set there: direct).
        assert_eq!(s.proxy(), mdm_engine::Proxy::System);
        s.proxy_from_browser = false;
        assert_eq!(s.proxy(), mdm_engine::Proxy::None);
        s.proxy_mode = 3;
        s.proxy_host = "10.0.0.1".into();
        s.proxy_port = "3128".into();
        s.proxy_https = true;
        assert!(matches!(
            s.proxy(),
            mdm_engine::Proxy::Manual { ref url, https: true, http: false, .. }
                if url == "http://10.0.0.1:3128"
        ));
    }
}

/// Sites Logins' passwords on disk: in the system's keychain once it holds them (secrets.rs),
/// else sealed as `sealed` seals them. A list from before reads as before, and a password that
/// can't be opened (the list copied to another account) reads as none.
mod sealed_logins {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::sealed::{open, seal};

    pub fn serialize<S: Serializer>(
        v: &[(String, String, String)],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        if crate::secrets::is_prepared() {
            return v.serialize(s);
        }
        v.iter()
            .map(|(site, user, pw)| (site, user, seal(pw)))
            .collect::<Vec<_>>()
            .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Vec<(String, String, String)>, D::Error> {
        let v = Vec::<(String, String, String)>::deserialize(d)?;
        Ok(v.into_iter()
            .map(|(site, user, pw)| (site, user, open(&pw)))
            .collect())
    }
}

/// One secret on disk (the proxy's password, a debrid account's credential, #44). In the
/// system's keychain once it holds it: the settings then keep its reference (secrets.rs,
/// `keychain:…`, written by [`Settings::for_disk`]). Until then, and where no keychain is used:
/// Windows seals it with DPAPI, for this account on this computer; Linux and macOS keep it as it
/// is, in the data folder only its owner may read (mdm_ipc::private_dir). A secret that can't be
/// opened (the file copied to another account) reads as none: sign in again.
pub(crate) mod sealed {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::secrets::REF;

    #[cfg(windows)]
    const SEALED: &str = "dpapi:";
    /// Before a secret kept as it is that would read as a reference (`keychain:…`), or as one
    /// of these.
    const PLAIN: &str = "plain:";

    pub fn serialize<S: Serializer>(v: &str, s: S) -> Result<S::Ok, S::Error> {
        if crate::secrets::is_prepared() {
            return s.serialize_str(v);
        }
        s.serialize_str(&seal(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        Ok(open(&String::deserialize(d)?))
    }

    pub(crate) fn seal(pw: &str) -> String {
        #[cfg(windows)]
        if !pw.is_empty()
            && let Some(sealed) = crate::win32::protect(pw)
        {
            use base64::Engine as _;
            return format!(
                "{SEALED}{}",
                base64::engine::general_purpose::STANDARD.encode(sealed)
            );
        }
        if pw.starts_with(REF) || escaped(pw).is_some() {
            return format!("{PLAIN}{pw}");
        }
        pw.to_owned()
    }

    /// What `seal` escaped: `plain:` before a reference or another `plain:` (a plain secret from
    /// before that starts with `plain:` alone reads as it is).
    fn escaped(stored: &str) -> Option<&str> {
        stored
            .strip_prefix(PLAIN)
            .filter(|rest| rest.starts_with(REF) || rest.starts_with(PLAIN))
    }

    pub(crate) fn open(stored: &str) -> String {
        // Read as the disk has it (store::keep_generations only checks that a list reads).
        if crate::secrets::is_prepared() {
            return stored.to_owned();
        }
        if stored.starts_with(REF) {
            return crate::secrets::vault().open(stored);
        }
        if let Some(plain) = escaped(stored) {
            return plain.to_owned();
        }
        #[cfg(windows)]
        if let Some(b64) = stored.strip_prefix(SEALED) {
            use base64::Engine as _;
            return base64::engine::general_purpose::STANDARD
                .decode(b64)
                .ok()
                .and_then(|b| crate::win32::unprotect(&b))
                .unwrap_or_default();
        }
        stored.to_owned()
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn the_proxy_password_and_debrid_secrets_are_sealed_on_disk() {
            let s = crate::settings::Settings {
                proxy_pass: "proxy-s3cret".into(),
                debrid_accounts: vec![crate::settings::DebridAccount {
                    id: 1,
                    user: "me".into(),
                    secret: r#"{"kind":"token","token":"rd-s3cret"}"#.into(),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let json = serde_json::to_string(&s).unwrap();
            if cfg!(windows) {
                assert!(!json.contains("proxy-s3cret") && !json.contains("rd-s3cret"));
            }
            let back: crate::settings::Settings = serde_json::from_str(&json).unwrap();
            assert_eq!(back.proxy_pass, "proxy-s3cret");
            assert!(back.debrid_accounts[0].account().is_some());
            // Never in a debug dump either.
            assert!(!format!("{:?}", back.debrid_accounts).contains("rd-s3cret"));
            // A plain password from before still reads; a secret sealed elsewhere reads as none
            // (signed out).
            let old: crate::settings::Settings =
                serde_json::from_value(serde_json::json!({ "proxy_pass": "plain" })).unwrap();
            assert_eq!(old.proxy_pass, "plain");
            if cfg!(windows) {
                let foreign =
                    serde_json::json!({ "debrid_accounts": [{ "id": 2, "secret": "dpapi:AAAA" }] });
                let s: crate::settings::Settings = serde_json::from_value(foreign).unwrap();
                assert!(s.debrid_accounts[0].account().is_none());
            }
        }

        #[test]
        fn passwords_are_sealed_on_disk_and_read_back() {
            let s = crate::settings::Settings {
                site_logins: vec![("example.com".into(), "me".into(), "s3cret".into())],
                ..Default::default()
            };
            let json = serde_json::to_string(&s).unwrap();
            if cfg!(windows) {
                assert!(!json.contains("s3cret") && json.contains("dpapi:"));
            }
            let back: crate::settings::Settings = serde_json::from_str(&json).unwrap();
            assert_eq!(back.site_logins, s.site_logins);
            // A list from before (plain) still reads.
            let plain = serde_json::json!({ "site_logins": [["a.test", "u", "pw"]] });
            let old: crate::settings::Settings = serde_json::from_value(plain).unwrap();
            assert_eq!(old.site_logins[0].2, "pw");
        }

        #[test]
        fn the_close_button_keeps_mdm_running_unless_told_otherwise() {
            let s: crate::settings::Settings =
                serde_json::from_value(serde_json::json!({})).unwrap();
            assert!(
                s.close_to_tray,
                "a list from before the option goes on hiding"
            );
            let s: crate::settings::Settings =
                serde_json::from_value(serde_json::json!({ "close_to_tray": false })).unwrap();
            assert!(!s.close_to_tray);
        }
    }
}
