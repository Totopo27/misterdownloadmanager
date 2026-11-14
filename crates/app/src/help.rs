//! The Help menu (D-020): the user guide and the home page on the website (docs/ made into
//! pages, in the interface language), problem reports on GitHub (pre-filled), update checks
//! against GitHub Releases (on demand, and at every start and once a day while running), About,
//! Keyboard shortcuts and Tell a friend.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{pos2, vec2};

use crate::dialogs::{Outcome, window};
use crate::winui::{self, ButtonKind, r};

/// The repository: problem reports, releases, source, licences.
pub const REPO: &str = "https://github.com/anefzaoui/misterdownloadmanager";
/// The creator's GitHub Sponsors page: Help → Support MDM, About's last line. The website, the
/// extension's popup and the repository's Sponsor button (.github/FUNDING.yml) point there too.
pub const SPONSORS: &str = "https://github.com/sponsors/anefzaoui";
const LATEST: &str = "https://api.github.com/repos/anefzaoui/misterdownloadmanager/releases/latest";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT: &str = env!("MDM_COMMIT");
/// While MDM runs, the automatic check comes again this often (it also checks at every start).
pub const CHECK_EVERY: i64 = 24 * 3600;
/// The check at start waits this long (the window, the list and the downloads first).
pub const CHECK_AFTER_START: std::time::Duration = std::time::Duration::from_secs(5);

/// The official website: the home page and the user guide, docs/*.md made into pages by
/// tools/website/build.py. GitHub (REPO) keeps what is GitHub's: issues, releases, the source.
pub const SITE: &str = "https://misterdownloadmanager.com";

/// The website's languages besides English, its folders (`/de/help/`): tools/website/sitei18n.py's
/// LANGS, which a test keeps this list equal to. English is at the root.
const SITE_LANGS: &[&str] = &[
    "es", "pt-br", "pt", "fr", "de", "ru", "ar", "zh-hans", "ja", "ko", "tr", "it", "id", "vi",
    "hi", "fa", "pl", "uk", "nl", "th", "bn",
];

/// The website's folder for the interface language `code` (a catalog's code: `de`, `pt-br`,
/// `es-mx`…): the language itself, else its base language (`es-mx` reads `/es/`, `fr-ca`
/// `/fr/`), else none, English. `zh-hant` stays English: the site has Simplified Chinese only.
fn site_lang(code: &str) -> Option<&'static str> {
    let code = code.to_ascii_lowercase();
    let base = code.split(['-', '_']).next().unwrap_or_default();
    SITE_LANGS
        .iter()
        .find(|l| **l == code)
        .or_else(|| SITE_LANGS.iter().find(|l| **l == base))
        .copied()
}

/// The website's address of a user guide page in the language `lang` (an interface language
/// code): `doc` is the page's file in docs/ with an optional section (`"video.md"`,
/// `"README.md#tutorials"`; the guide's contents are README.md), `""` the home page. The mapping
/// is build.py's: docs/README.md is /help/, docs/<page>.md /help/<page>.html.
pub fn page_url(doc: &str, lang: &str) -> String {
    let (file, anchor) = match doc.split_once('#') {
        Some((file, anchor)) => (file, format!("#{anchor}")),
        None => (doc, String::new()),
    };
    let path = match file {
        "" => "/".to_owned(),
        "README.md" => "/help/".to_owned(),
        page => format!("/help/{}.html", page.strip_suffix(".md").unwrap_or(page)),
    };
    let folder = site_lang(lang).map(|l| format!("/{l}")).unwrap_or_default();
    format!("{SITE}{folder}{path}{anchor}")
}

/// Open a page of the user guide on the website, in the interface language when the site has
/// it (`doc` as page_url takes it: `"video.md"`, `"README.md#tutorials"`, `""` the home page).
/// Never in an off-screen test run (MDM_OFFSCREEN), whose clicks come from scripts, not from a
/// person at the browser.
pub fn open_page(doc: &str) {
    if !crate::offscreen() {
        crate::launch::open(page_url(doc, crate::i18n::code()));
    }
}

/// Help → Support MDM and About's link: the Sponsors page in the browser. Asked for only: MDM
/// never brings it up by itself, and never in an off-screen test run (MDM_OFFSCREEN), whose
/// clicks come from scripts, not from a person at the browser.
pub fn support() {
    if !crate::offscreen() {
        crate::launch::open(SPONSORS);
    }
}

/// A new GitHub issue on the Bug report form (.github/ISSUE_TEMPLATE/bug.yml), filled with
/// what MDM knows: its version, the system and its version, how it was installed, the
/// language. The parameters are the form's field ids (all text fields: GitHub's page drops a
/// dropdown's value from the address).
pub fn report_problem() {
    // On a thread of its own: the fields ask the system (macOS: `sw_vers`), which the window's
    // thread doesn't wait for (#222).
    let _ = std::thread::Builder::new()
        .name("report-problem".into())
        .spawn(|| {
            let enc =
                |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
            let url = report_fields().iter().fold(
                format!("{REPO}/issues/new?template=bug.yml"),
                |url, (k, v)| format!("{url}&{k}={}", enc(v)),
            );
            let _ = open::that_detached(url);
        });
}

/// The Bug report form's fields MDM fills in: (field id, value).
fn report_fields() -> Vec<(&'static str, String)> {
    #[cfg(windows)]
    let os_version = crate::win32::windows_version();
    #[cfg(unix)]
    let os_version = crate::unix::os_version();
    let os = if cfg!(windows) {
        if os_version.contains("Windows 10") {
            "Windows 10"
        } else {
            "Windows 11"
        }
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "macOS (Apple Silicon)"
        } else {
            "macOS (Intel)"
        }
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        "Other"
    };
    let mut fields = vec![
        ("os", os.to_owned()),
        ("os-version", os_version),
        ("version", format!("{VERSION} ({COMMIT})")),
        ("language", crate::i18n::code().to_owned()),
    ];
    fields.push((
        "install",
        mdm_tools::update::Install::detect().label().to_owned(),
    ));
    fields
}

// ---------------------------------------------------------------------------------------------
// Updates

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub notes: String,
    /// The release's page (its files are there too).
    pub page: String,
    /// Its files: (name, download address); Update now takes this copy's from them.
    pub assets: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    Latest,
    Newer(Release),
}

pub type Slot = Arc<Mutex<Option<Result<Found, String>>>>;

/// Ask GitHub for the latest release in the background; the answer lands in the slot.
pub fn check(rt: &tokio::runtime::Handle, ctx: &egui::Context) -> Slot {
    let slot: Slot = Arc::new(Mutex::new(None));
    let (out, ctx) = (slot.clone(), ctx.clone());
    rt.spawn(async move {
        let res = latest().await;
        *out.lock().unwrap() = Some(res);
        ctx.request_repaint();
    });
    slot
}

async fn latest() -> Result<Found, String> {
    // MDM_UPDATE_FEED: another address for the latest release (tests of Update now, as
    // MDM_DATA_DIR is for a separate list).
    let feed = std::env::var("MDM_UPDATE_FEED").unwrap_or_else(|_| LATEST.to_owned());
    let net = crate::settings::own_net();
    // The chosen DNS and network adapter, as downloads have them (#36, #35).
    let client = mdm_engine::net::client_builder(&net.dns, &net.route, &feed)
        .map_err(|e| e.to_string())?
        .user_agent(concat!("MisterDownloadManager/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(feed)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    // No release published yet: nothing newer than this.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Found::Latest);
    }
    let text = resp
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    parse_release(&text, VERSION)
}

/// GitHub's latest-release JSON against the running `current` version.
fn parse_release(json: &str, current: &str) -> Result<Found, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().unwrap_or("").trim();
    let version = tag.trim_start_matches(['v', 'V']);
    if version.is_empty() {
        // i18n: why updates could not be checked: the newest release on the site has no number
        return Err(crate::i18n::t("the release has no version").to_owned());
    }
    if !newer(version, current) {
        return Ok(Found::Latest);
    }
    let s = |k: &str| v[k].as_str().unwrap_or("").to_owned();
    Ok(Found::Newer(Release {
        version: version.to_owned(),
        date: s("published_at").chars().take(10).collect(),
        notes: s("body"),
        page: s("html_url"),
        assets: v["assets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        Some((
                            x["name"].as_str()?.to_owned(),
                            x["browser_download_url"].as_str()?.to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }))
}

/// `a` is a later version than `b` (numeric parts; a pre-release `-…` counts before its
/// release).
pub fn newer(a: &str, b: &str) -> bool {
    fn key(v: &str) -> (Vec<u64>, bool) {
        let (main, pre) = v.split_once('-').map_or((v, false), |(m, _)| (m, true));
        (
            main.split('.')
                .map(|p| p.trim().parse().unwrap_or(0))
                .collect(),
            !pre,
        )
    }
    let (mut ka, mut kb) = (key(a), key(b));
    let n = ka.0.len().max(kb.0.len());
    ka.0.resize(n, 0);
    kb.0.resize(n, 0);
    ka > kb
}

/// Help → Check for updates... (and the automatic check when it finds one).
pub struct Updates {
    slot: Option<Slot>,
    result: Option<Result<Found, String>>,
    /// Update now: the release file on its way (crate::update).
    job: Option<crate::update::Job>,
    /// What Update now came to, when it didn't close MDM.
    status: Option<UpdateStatus>,
    /// Update now pressed by a test (`/show update-now`, feature `test-update`).
    press: bool,
}

#[derive(Clone, Debug)]
enum UpdateStatus {
    Failed(String),
    /// A .deb or .rpm, opened in the system's package installer.
    Opened,
}

impl Updates {
    /// Update now, as if pressed (`/show update-now`: Update now's end-to-end tests).
    #[cfg(feature = "test-update")]
    pub fn press_update_now(&mut self) {
        self.press = true;
    }

    pub fn check(rt: &tokio::runtime::Handle, ctx: &egui::Context) -> Self {
        Updates {
            slot: Some(check(rt, ctx)),
            result: None,
            job: None,
            status: None,
            press: false,
        }
    }

    /// The automatic check found `release`.
    pub fn found(release: Release) -> Self {
        Updates {
            slot: None,
            result: Some(Ok(Found::Newer(release))),
            job: None,
            status: None,
            press: false,
        }
    }

    /// `weekly`: the automatic check (its switch is here, under the answer). `Done(true)`:
    /// the update is going in place, MDM closes for it (and starts again).
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        rt: &tokio::runtime::Handle,
        weekly: &mut bool,
    ) -> Outcome<bool> {
        if let Some(r) = self.slot.as_ref().and_then(|s| s.lock().unwrap().take()) {
            self.result = Some(r);
            self.slot = None;
        }
        let install = mdm_tools::update::Install::detect();
        // The release file arrived (checked) and is in place (on the job's thread): MDM closes
        // once the new copy (or the installer) has started.
        if let Some(res) = self.job.as_ref().and_then(crate::update::Job::result) {
            self.job = None;
            match res {
                Ok(crate::update::Next::Restart) => return Outcome::Done(true),
                Ok(crate::update::Next::Opened) => self.status = Some(UpdateStatus::Opened),
                Err(e) => self.status = Some(UpdateStatus::Failed(e)),
            }
        }
        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let result = self.result.clone();
        // Compact unless there are release notes to show (then a window of its own size).
        let notes = matches!(result, Some(Ok(Found::Newer(_))));
        // One window that grows when notes arrive (a second id made a new window flash up).
        let (id, h) = ("updates", if notes { 250.0 } else { 96.0 });
        let bottom = h - 32.0;
        window(
            ctx,
            id,
            "Check for updates",
            [440.0, h],
            false,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let ink = winui::TEXT();
                let right = o.x + 429.0;
                let mut download = None;
                match &result {
                    None => {
                        winui::text(
                            &p,
                            o.x + 11.0,
                            o.y + 24.0,
                            crate::i18n::t("Checking for updates..."),
                            ink,
                        );
                    }
                    Some(Ok(Found::Latest)) => {
                        let line = crate::i18n::fmt(
                            crate::i18n::t(
                                "You have the latest version of Mister Download Manager ({version}).",
                            ),
                            &[("version", VERSION)],
                        );
                        winui::paragraph(&p, o.x + 11.0, o.y + 24.0, &line, ink, right, 13.0);
                    }
                    Some(Err(e)) => {
                        let line = crate::i18n::fmt(
                            crate::i18n::t("Updates could not be checked: {error}"),
                            &[("error", e)],
                        );
                        let (rect, cut) =
                            winui::text_fit(&p, o.x + 11.0, o.y + 24.0, &line, ink, right);
                        if cut {
                            winui::full_on_hover(ui, rect, "update-error", &line);
                        }
                    }
                    Some(Ok(Found::Newer(rel))) => {
                        let line = crate::i18n::fmt(
                            crate::i18n::t(
                                "Mister Download Manager {version} is available (released {date}).",
                            ),
                            &[("version", &rel.version), ("date", &rel.date)],
                        );
                        winui::fit(
                            ui,
                            &p,
                            o.x + 11.0,
                            o.y + 24.0,
                            &line,
                            ink,
                            right,
                            "update-line",
                        );
                        // A package of another package manager (pacman, …): that one updates it.
                        let have = if install == mdm_tools::update::Install::System {
                            crate::i18n::fmt(
                                // i18n: Check for updates, under the new version's line, when MDM was installed by the Linux system's package manager (pacman, ...), which MDM can't update itself
                                crate::i18n::t(
                                    "You have version {version}, from your system's package manager: update it there.",
                                ),
                                &[("version", VERSION)],
                            )
                        } else {
                            crate::i18n::fmt(
                                crate::i18n::t("You have version {version}."),
                                &[("version", VERSION)],
                            )
                        };
                        winui::fit(
                            ui,
                            &p,
                            o.x + 11.0,
                            o.y + 42.0,
                            &have,
                            winui::MUTED(),
                            right,
                            "update-have",
                        );
                        winui::text(
                            &p,
                            o.x + 11.0,
                            o.y + 66.0,
                            crate::i18n::t("What's new:"),
                            ink,
                        );
                        let notes = egui::Rect::from_min_max(
                            pos2(o.x + 11.0, o.y + 74.0),
                            pos2(right, o.y + 206.0),
                        );
                        // English, as the paragraph is laid out; made where it is on screen.
                        // Read-only text: the read-only field's look (it drew a square box of its own).
                        winui::field_frame(&p, winui::rect(notes), winui::FieldLook::ReadOnly);
                        // Release notes run longer than the box: it scrolls (issue #5).
                        winui::scroll_text(
                            ui,
                            egui::Id::unique("update-notes"),
                            notes,
                            &plain_notes(&rel.notes),
                            ink,
                            13.0,
                            true,
                        );
                        download = Some(rel.clone());
                    }
                }
                // Update now when this copy's file is in the signed release (and it hasn't just
                // failed); else Download opens the release's page.
                let asset = download.as_ref().and_then(|rel| {
                    crate::update::asset_for(install, &rel.version, &rel.assets)
                        .filter(|_| !matches!(self.status, Some(UpdateStatus::Failed(_))))
                });
                // Another package manager's copy: no Update now nor Download, the notes only.
                let managed = install == mdm_tools::update::Install::System;
                let close_label = if self.job.is_some() {
                    // i18n: button: stop downloading the update
                    crate::i18n::mark("Cancel")
                } else if download.is_some() && !managed {
                    // i18n: button: not now (download the update later)
                    crate::i18n::mark("Later")
                } else {
                    crate::i18n::mark("OK")
                };
                // The first button: Retry after a failed check, Update now or Download for a new
                // version.
                let first = if matches!(result, Some(Err(_))) {
                    Some("Retry")
                } else if download.is_some() && !managed {
                    Some(if asset.is_some() {
                        "Update now"
                    } else {
                        "Download"
                    })
                } else {
                    None
                };
                // The buttons as wide as their labels in this language (#192), the row ending
                // where it ends in English.
                let row: Vec<(&str, f32)> = first
                    .into_iter()
                    .chain([close_label])
                    .map(|l| (l, 73.0))
                    .collect();
                let placed = winui::buttons_to_right(&p, 429.0, 9.0, &row, 150.0);
                let (first_at, close_at) = (placed[0], placed[placed.len() - 1]);
                // Under the answer: how Update now is going, else the automatic check's switch,
                // up to 8 px before the buttons (as in English with two). In English, as the text
                // pens take it (a right-to-left window placed the status over its buttons when
                // given the mirrored rect).
                let status_right = (first_at.0 - 8.0).min(266.0);
                let status_rc = egui::Rect::from_min_size(
                    o + vec2(11.0, bottom),
                    vec2(status_right - 11.0, 21.0),
                );
                if let Some(job) = &self.job {
                    let installing = job.installing();
                    // Put in place: a bar that moves, no percent.
                    let fraction = job.progress.fraction().filter(|_| !installing);
                    let line = if installing {
                        // i18n: Check for updates, after Update now downloaded it: unpacking and putting the new version in place
                        crate::i18n::t("Installing the update...").to_owned()
                    } else {
                        crate::i18n::fmt(
                            // i18n: Check for updates, while Update now downloads; {percent} like "42%"
                            crate::i18n::t("Downloading the update... {percent}"),
                            &[(
                                "percent",
                                &fraction.map_or(String::new(), |f| {
                                    crate::i18n::percent(f64::from(f), 0)
                                }),
                            )],
                        )
                    };
                    winui::text(&p, status_rc.left(), status_rc.top() + 9.0, &line, ink);
                    winui::progress_bar(
                        &p,
                        winui::rect(egui::Rect::from_min_size(
                            pos2(status_rc.left(), status_rc.top() + 15.0),
                            vec2(status_rc.width(), 4.0),
                        )),
                        fraction,
                        ui.input(|i| i.time),
                    );
                } else if let Some(status) = &self.status {
                    let line = match status {
                        UpdateStatus::Failed(e) => crate::i18n::fmt(
                            crate::i18n::t("The update couldn't be installed: {error}"),
                            &[("error", e)],
                        ),
                        UpdateStatus::Opened => {
                            crate::i18n::t("The update is open in your system's package installer.")
                                .to_owned()
                        }
                    };
                    let (rect, cut) = winui::text_fit(
                        &p,
                        status_rc.left(),
                        status_rc.top() + 14.0,
                        &line,
                        ink,
                        status_rc.right(),
                    );
                    if cut {
                        winui::full_on_hover(ui, rect, "update-status", &line);
                    }
                } else {
                    // Its caption stops before the buttons (a longer translation ends in "...").
                    winui::checkbox_until(
                        ui,
                        o + vec2(11.0, bottom + 4.0),
                        weekly,
                        // i18n: check box in the update window: look for a new version every time MDM starts
                        "Check for updates at every start",
                        true,
                        Some(o.x + status_right),
                    );
                }
                // Enter presses the default button, as in the other dialogs.
                let mut enter = winui::enter_pressed(ui);
                // A check that failed can be tried again.
                let at = r(o, first_at.0, bottom, first_at.1, 21.0);
                if matches!(result, Some(Err(_))) {
                    // i18n: button: check for updates again
                    let retry = winui::button(ui, at, "Retry", ButtonKind::Default, false, None);
                    if retry.clicked() || enter {
                        self.slot = Some(check(rt, ui.ctx()));
                        self.result = None;
                        enter = false;
                    }
                }
                if let Some(rel) = download.as_ref().filter(|_| !managed) {
                    if let Some(asset) = asset {
                        let idle = self.job.is_none();
                        let kind = if idle {
                            ButtonKind::Default
                        } else {
                            ButtonKind::Disabled
                        };
                        // i18n: button: download and install the new version, then restart
                        let update = winui::button(ui, at, "Update now", kind, false, None);
                        if update.clicked()
                            || (enter && idle)
                            || (idle && std::mem::take(&mut self.press))
                        {
                            self.status = None;
                            self.job = Some(crate::update::Job::start(
                                rt,
                                ui.ctx(),
                                install,
                                rel.assets.clone(),
                                asset,
                            ));
                        }
                    } else if winui::button(ui, at, "Download", ButtonKind::Default, false, None)
                        .clicked()
                        || enter
                    {
                        crate::launch::open(&rel.page);
                        return Outcome::Done(false);
                    }
                }
                // Being put in place: nothing can stop that, so no Cancel (nor Esc) meanwhile.
                let installing = self
                    .job
                    .as_ref()
                    .is_some_and(crate::update::Job::installing);
                let kind = if installing {
                    ButtonKind::Disabled
                } else if download.is_some() || matches!(result, Some(Err(_))) {
                    ButtonKind::Normal
                } else {
                    ButtonKind::Default
                };
                let close = winui::button(
                    ui,
                    r(o, close_at.0, bottom, close_at.1, 21.0),
                    close_label,
                    kind,
                    false,
                    None,
                )
                .clicked();
                if installing {
                    Outcome::Open
                } else if close && self.job.is_some() {
                    // Cancel: the download stops, the window stays (Update now again, Later).
                    self.job = None;
                    Outcome::Open
                } else if close || winui::escape_pressed(ui) || (enter && download.is_none()) {
                    Outcome::Closed
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// Release notes (Markdown) as plain lines: headings, bullets, emphasis marks and link
/// addresses dropped (a link keeps its text). A paragraph or bullet wrapped over several lines
/// in the Markdown is one line again (the box wraps it to its own width).
fn plain_notes(md: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut joinable = false;
    for raw in md.lines() {
        let l = raw.trim();
        if l.is_empty() {
            out.push(String::new());
            joinable = false;
            continue;
        }
        let heading = l.starts_with('#');
        let bullet = l.strip_prefix("- ").or_else(|| l.strip_prefix("* "));
        let text = link_texts(l.trim_start_matches('#').trim_start())
            .replace("**", "")
            .replace('`', "");
        match (bullet, out.last_mut()) {
            (None, Some(last)) if joinable && !heading => {
                last.push(' ');
                last.push_str(&text);
            }
            (Some(b), _) => {
                let b = link_texts(b).replace("**", "").replace('`', "");
                out.push(format!("• {b}"));
            }
            _ => out.push(text),
        }
        joinable = !heading;
    }
    out.join("\n")
}

/// `[text](address)` becomes `text`.
fn link_texts(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some(close) = after.find("](") else { break };
        let Some(end) = after[close + 2..].find(')') else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&after[..close]);
        rest = &after[close + 2 + end + 1..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------------------------
// About

pub struct About;

impl About {
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        // A long translation pushes the links and OK down; the window grows with them.
        let extra = crate::dialogs::grow_extra(ctx, "about");
        window(
            ctx,
            "about",
            "About Mister Download Manager",
            [420.0, 236.0 + extra],
            false,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let ink = winui::TEXT();
                egui::Image::from_bytes(
                    "bytes://mdm-48.png",
                    include_bytes!("../assets/icons/app/mdm-48.png"),
                )
                .paint_at(ui, winui::r(o, 16.0, 16.0, 48.0, 48.0));
                winui::text(
                    &p,
                    o.x + 80.0,
                    o.y + 28.0,
                    crate::i18n::t("Mister Download Manager"),
                    ink,
                );
                let version = crate::i18n::fmt(
                    crate::i18n::t("Version {version} ({commit})"),
                    &[("version", VERSION), ("commit", COMMIT)],
                );
                // Copyable: bug reports ask for it.
                let rect = winui::text(&p, o.x + 80.0, o.y + 46.0, &version, winui::MUTED());
                winui::copyable(ui, rect, "about-version", &version, false);
                let lines = winui::paragraph(
                    &p,
                    o.x + 80.0,
                    o.y + 72.0,
                    crate::i18n::t(
                        "The download manager you already know, rebuilt to be faster. Free and open source under the GNU GPL, version 3 or later. Videos are found by yt-dlp; torrents run on librqbit.",
                    ),
                    ink,
                    o.x + 404.0,
                    13.0,
                );
                // The links in a row, 18 px apart; one that would pass the paragraph's right
                // edge (a long translation) starts the next row.
                let (mut x, mut y) = (o.x + 80.0, o.y + 72.0 + 13.0 * lines as f32 + 14.0);
                let mut place = |label: &str| {
                    let w = winui::text_width(&p, crate::i18n::t(label));
                    if x > o.x + 80.0 && x + w > o.x + 404.0 {
                        (x, y) = (o.x + 80.0, y + 16.0);
                    }
                    let at = (x, y);
                    x += w + 18.0;
                    at
                };
                let (home, source, credits) = (
                    place("Home page"),
                    place("Source code"),
                    place("Licenses and credits"),
                );
                if winui::link(ui, &p, home.0, home.1, "Home page", "home") {
                    open_page("");
                }
                if winui::link(ui, &p, source.0, source.1, "Source code", "source") {
                    crate::launch::open(REPO);
                }
                if winui::link(
                    ui,
                    &p,
                    credits.0,
                    credits.1,
                    "Licenses and credits",
                    "credits",
                ) {
                    crate::launch::open(format!("{REPO}/blob/main/ATTRIBUTION.md"));
                }
                // Its own row under them: the creator's Sponsors page (Help → Support MDM).
                y += 22.0;
                if winui::link(
                    ui,
                    &p,
                    o.x + 80.0,
                    y,
                    "Support MDM on GitHub Sponsors",
                    "support",
                ) {
                    support();
                }
                // The last row ends above OK (at 204 when it fits as planned).
                crate::dialogs::grow_px(ui.ctx(), "about", y + 12.0 - (o.y + 194.0));
                let ok = winui::button(
                    ui,
                    r(o, 336.0, 204.0 + extra, 73.0, 21.0),
                    "OK",
                    ButtonKind::Default,
                    false,
                    None,
                )
                .clicked();
                if ok || winui::escape_pressed(ui) || winui::enter_pressed(ui) {
                    Outcome::Closed
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

// ---------------------------------------------------------------------------------------------
// Keyboard shortcuts

/// Every shortcut the main window answers (keep in step with `app.rs` and `list.rs`).
const SHORTCUTS: [(&str, &str); 26] = [
    ("Ctrl+N", "Add a download (Add URL)"),
    (
        "Ctrl+B",
        "Download the links in the clipboard (Add batch download from clipboard)",
    ),
    ("Ctrl+F", "Find a download"),
    ("F3", "Find the next match"),
    ("Enter", "Open the selected download"),
    ("Alt+Enter", "Properties of the selected download"),
    ("Ctrl+M / F2", "Move or rename the selected download"),
    ("Delete", "Delete the selected downloads (asks first)"),
    (
        "Shift+Delete",
        "Delete the selected downloads and their files (asks first)",
    ),
    ("Ctrl+A", "Select all downloads"),
    ("Ctrl+click", "Select or unselect one more download"),
    ("Shift+click", "Select every download up to this one"),
    ("Drag on blank space", "Select the downloads in the box"),
    ("Up / Down", "Select the previous / next download"),
    (
        "Shift+Up / Shift+Down",
        "Make the selection longer or shorter",
    ),
    ("Ctrl+Up / Ctrl+Down", "Move without changing the selection"),
    (
        "Ctrl+Space",
        "Select or unselect the download with the dotted outline",
    ),
    ("Page Up / Page Down", "Move a page up / down"),
    ("Home / End", "Select the first / last download"),
    (
        "Type a name",
        "Jump to the download whose name starts with it",
    ),
    ("Shift+F10", "Open the selected download's menu"),
    ("Tab", "Move between the categories and the download list"),
    (
        "Shift+F10 / Menu",
        "In the categories: the selected category's menu",
    ),
    ("Esc", "Close a menu or dialog"),
    // View → Interface size (#262), in every window.
    (
        "Ctrl++ / Ctrl+-",
        "Make everything larger / smaller (View → Interface size)",
    ),
    ("Ctrl+0", "Everything at its normal size (100%)"),
];

pub struct Shortcuts;

impl Shortcuts {
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let rows = SHORTCUTS.len() as f32;
        window(
            ctx,
            "shortcuts",
            "Keyboard shortcuts",
            [520.0, 52.0 + 18.0 * rows + 20.0],
            false,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let ink = winui::TEXT();
                for (i, (keys, what)) in SHORTCUTS.iter().enumerate() {
                    let y = o.y + 24.0 + 18.0 * i as f32;
                    // As this system writes it: ⌘N on macOS, the translated key names elsewhere.
                    let keys = crate::chrome::key_label(keys);
                    winui::fit(
                        ui,
                        &p,
                        o.x + 16.0,
                        y,
                        &keys,
                        ink,
                        o.x + 174.0,
                        ("keys", y as i32),
                    );
                    let what = crate::i18n::t(what);
                    winui::fit(
                        ui,
                        &p,
                        o.x + 184.0,
                        y,
                        what,
                        ink,
                        o.x + 504.0,
                        ("what", y as i32),
                    );
                }
                let y = 24.0 + 18.0 * rows + 6.0;
                let ok = winui::button(
                    ui,
                    r(o, 436.0, y, 73.0, 21.0),
                    "OK",
                    ButtonKind::Default,
                    false,
                    None,
                )
                .clicked();
                if ok
                    || ui.input(|i| {
                        i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)
                    })
                {
                    Outcome::Closed
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

// ---------------------------------------------------------------------------------------------
// Tell a friend

pub struct TellFriend {
    /// What became of Copy or Email: the line under the message, and whether it is a failure.
    said: Option<(&'static str, bool)>,
}

impl TellFriend {
    pub fn new() -> Self {
        TellFriend { said: None }
    }

    fn message() -> String {
        crate::i18n::fmt(
            crate::i18n::t(
                "I download with Mister Download Manager: fast, free, and it looks like the download manager you already know. {link}",
            ),
            &[("link", SITE)],
        )
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let message = Self::message();
        // The message's box grows with a long translation, the window with it.
        let extra = crate::dialogs::grow_extra(ctx, "tell-a-friend");
        window(
            ctx,
            "tell-a-friend",
            "Tell a friend",
            [420.0, 168.0 + extra],
            false,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let ink = winui::TEXT();
                winui::text(
                    &p,
                    o.x + 11.0,
                    o.y + 21.0,
                    crate::i18n::t("Send this to a friend:"),
                    ink,
                );
                let rect = egui::Rect::from_min_max(
                    pos2(o.x + 11.0, o.y + 30.0),
                    pos2(o.x + 409.0, o.y + 106.0 + extra),
                );
                // English, as the paragraph is laid out; made where it is on screen.
                // Read-only text: the read-only field's look (it drew a square box of its own).
                winui::field_frame(&p, winui::rect(rect), winui::FieldLook::ReadOnly);
                let lines = winui::paragraph(
                    &p,
                    rect.left() + 6.0,
                    rect.top() + 17.0,
                    &message,
                    ink,
                    rect.right() - 6.0,
                    13.0,
                );
                crate::dialogs::grow_to(ui.ctx(), "tell-a-friend", lines, 5);
                let o = o + vec2(0.0, extra);
                // The buttons as wide as their labels in this language (#192), the row ending
                // where it ends in English.
                let [copy_at, email_at, close_at] = winui::buttons_to_right(
                    &p,
                    409.0,
                    9.0,
                    &[("Copy", 73.0), ("Email", 73.0), ("Close", 73.0)],
                    130.0,
                )[..] else {
                    unreachable!("three buttons")
                };
                if let Some((said, failed)) = self.said {
                    // Short of the buttons beside it (a long translation: "...", whole on hover).
                    let c = if failed {
                        winui::ERROR()
                    } else {
                        winui::MUTED()
                    };
                    winui::fit(
                        ui,
                        &p,
                        o.x + 11.0,
                        o.y + 150.0,
                        crate::i18n::t(said),
                        c,
                        o.x + copy_at.0 - 6.0,
                        "copied",
                    );
                }
                let copy = winui::button(
                    ui,
                    r(o, copy_at.0, 136.0, copy_at.1, 21.0),
                    "Copy",
                    ButtonKind::Default,
                    false,
                    None,
                )
                .clicked()
                    || winui::enter_pressed(ui);
                let email = winui::button(
                    ui,
                    r(o, email_at.0, 136.0, email_at.1, 21.0),
                    "Email",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked();
                let close = winui::button(
                    ui,
                    r(o, close_at.0, 136.0, close_at.1, 21.0),
                    "Close",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked();
                // The user asked for it: the message goes to the clipboard (a busy clipboard
                // says so; it showed nothing).
                let to_clipboard = crate::set_clipboard;
                if copy {
                    self.said = Some(if to_clipboard(&message) {
                        (crate::i18n::mark("Copied to the clipboard."), false)
                    } else {
                        // i18n: Tell a friend: Copy failed (another program holds the clipboard)
                        (crate::i18n::mark("The clipboard is busy: try again."), true)
                    });
                }
                if email {
                    let enc = |s: &str| {
                        url::form_urlencoded::byte_serialize(s.as_bytes())
                            .collect::<String>()
                            .replace('+', "%20")
                    };
                    let subject = crate::i18n::t("Mister Download Manager");
                    let opened = open::that_detached(format!(
                        "mailto:?subject={}&body={}",
                        enc(subject),
                        enc(&message)
                    ));
                    // No mail program: the message is copied instead, and said so.
                    if opened.is_err() {
                        self.said = Some(if to_clipboard(&message) {
                            // i18n: Tell a friend: Email found no mail program, so the message was copied
                            (
                                crate::i18n::mark(
                                    "No mail app: the message is copied, paste it in your mail.",
                                ),
                                false,
                            )
                        } else {
                            // i18n: Tell a friend: Email found no mail program
                            (crate::i18n::mark("No mail app opened."), true)
                        });
                    }
                }
                if close || winui::escape_pressed(ui) {
                    Outcome::Closed
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("1.0", "0.9.9"));
        assert!(
            newer("1.0.0", "1.0.0-rc.1"),
            "a release is newer than its pre-release"
        );
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0-beta", "0.1.0"));
        assert!(!newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn a_release_is_read_from_github() {
        let json = r###"{"tag_name":"v0.2.0","published_at":"2026-11-05T10:00:00Z",
            "html_url":"https://github.com/x/y/releases/tag/v0.2.0","body":"## New\n- Faster",
            "assets":[{"name":"SHA256SUMS.txt","browser_download_url":"https://x/SHA256SUMS.txt"}]}"###;
        assert_eq!(
            parse_release(json, "0.1.0").unwrap(),
            Found::Newer(Release {
                version: "0.2.0".into(),
                date: "2026-11-05".into(),
                notes: "## New\n- Faster".into(),
                page: "https://github.com/x/y/releases/tag/v0.2.0".into(),
                assets: vec![("SHA256SUMS.txt".into(), "https://x/SHA256SUMS.txt".into())],
            })
        );
        assert_eq!(parse_release(json, "0.2.0").unwrap(), Found::Latest);
        assert!(parse_release("{}", "0.1.0").is_err());
    }

    #[test]
    fn notes_become_plain_lines() {
        assert_eq!(
            plain_notes("## New\n- **Faster** `x`\n* b"),
            "New\n• Faster x\n• b"
        );
        assert_eq!(
            plain_notes("Tell us in an [issue](https://x/issues), or [the guide](docs/a.md#b)."),
            "Tell us in an issue, or the guide."
        );
        assert_eq!(plain_notes("[not a link] and [x]"), "[not a link] and [x]");
        // Hard-wrapped Markdown: one line per paragraph and per bullet.
        assert_eq!(
            plain_notes("Fixes for\nthe Mac.\n\n### Fixed\n\n- **Menus** did\n  nothing.\n- Paste"),
            "Fixes for the Mac.\n\nFixed\n\n• Menus did nothing.\n• Paste"
        );
    }

    #[test]
    fn the_report_fills_fields_the_bug_form_has() {
        let form = include_str!("../../../.github/ISSUE_TEMPLATE/bug.yml").replace("\r\n", "\n");
        let has = |line: String| form.contains(&format!("{line}\n"));
        let fields = report_fields();
        for (id, _) in &fields {
            assert!(has(format!("id: {id}")), "no field {id} in bug.yml");
            // Filled as text: GitHub's page put a dropdown's value from the address back to none.
            let field = form
                .split(&format!(
                    "id: {id}
"
                ))
                .next()
                .unwrap();
            assert!(
                field.ends_with(
                    "- type: input
    "
                ),
                "{id} isn't a text field"
            );
        }
        assert!(fields.iter().any(|(id, _)| *id == "install"));
        // Every value Install::label() and report_fields() can give, on any system, is one the
        // field's description names.
        let named = |v: &str| {
            [format!("{v}, "), format!("{v} or "), format!(" or {v}.")]
                .iter()
                .any(|s| form.contains(s.as_str()))
        };
        for install in mdm_tools::update::Install::ALL {
            assert!(named(install.label()), "{}", install.label());
        }
        for option in [
            "Windows 11",
            "Windows 10",
            "macOS (Apple Silicon)",
            "macOS (Intel)",
            "Linux",
            "Other",
        ] {
            assert!(named(option), "{option}");
        }
    }

    #[test]
    fn help_pages_are_the_websites_in_the_interface_language() {
        let at = |doc, lang| page_url(doc, lang);
        assert_eq!(
            at("README.md", "en"),
            "https://misterdownloadmanager.com/help/"
        );
        assert_eq!(
            at("README.md#tutorials", "en"),
            "https://misterdownloadmanager.com/help/#tutorials"
        );
        assert_eq!(
            at("video.md", "de"),
            "https://misterdownloadmanager.com/de/help/video.html"
        );
        assert_eq!(
            at("queues-and-scheduler.md#the-scheduler-window", "ar"),
            "https://misterdownloadmanager.com/ar/help/queues-and-scheduler.html#the-scheduler-window"
        );
        assert_eq!(at("", "fr"), "https://misterdownloadmanager.com/fr/");
        assert_eq!(at("", "en"), "https://misterdownloadmanager.com/");
        // The site's folders: its own codes, a region read as its language, else English.
        for (lang, folder) in [
            ("pt-br", "/pt-br"),
            ("pt", "/pt"),
            ("zh-hans", "/zh-hans"),
            ("es-mx", "/es"),
            ("fr-ca", "/fr"),
            ("zh-hant", ""),
            ("sv", ""),
            ("ca-es-valencia", ""),
            ("he", ""),
            ("", ""),
        ] {
            assert_eq!(
                at("options.md", lang),
                format!("https://misterdownloadmanager.com{folder}/help/options.html"),
                "{lang}"
            );
        }
    }

    /// The repository's root.
    fn root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn the_site_languages_are_the_websites() {
        // tools/website/sitei18n.py: `LANGS = [` then one `("code", …),` per language.
        let py = std::fs::read_to_string(root().join("tools/website/sitei18n.py")).unwrap();
        let list = py.split("\nLANGS = [").nth(1).unwrap();
        let codes: Vec<&str> = list
            .split("\n]")
            .next()
            .unwrap()
            .lines()
            .filter_map(|l| l.trim().strip_prefix("(\"")?.split('"').next())
            .collect();
        assert_eq!(codes, SITE_LANGS);
    }

    /// Every user guide page the program links to (`doc` as page_url takes it): the literal
    /// pages given to open_page and the Help menu (`Cmd::HelpPage`) in the app's source, the
    /// extension window's tutorial and help sections, and every website help address written in
    /// a crate or in the browser extension.
    fn linked_docs() -> Vec<String> {
        let mut docs = vec![mdm_ipc::extension::GUIDE_DOC.to_owned()];
        // The sections Options' Test browser integration opens (extension::checks' reasons).
        for why in ["options", "not-installed"] {
            docs.push(mdm_ipc::extension::help_doc(why));
        }
        let calls = regex::Regex::new(r#"(?:open_page|HelpPage)\(\s*"([^"]+)""#).unwrap();
        let site =
            regex::Regex::new(r"https://misterdownloadmanager\.com/help/([\w.-]*)(#[\w-]+)?")
                .unwrap();
        let mut files = vec![];
        let mut dirs = vec![root().join("crates"), root().join("extension")];
        while let Some(dir) = dirs.pop() {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|x| x == "rs" || x == "js") {
                    files.push(path);
                }
            }
        }
        let app = root().join("crates").join("app");
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            if f.starts_with(&app) {
                docs.extend(calls.captures_iter(&text).map(|c| c[1].to_owned()));
            }
            for c in site.captures_iter(&text) {
                let page = match &c[1] {
                    "" => "README.md".to_owned(),
                    p => format!("{}.md", p.strip_suffix(".html").unwrap_or(p)),
                };
                docs.push(format!("{page}{}", c.get(2).map_or("", |a| a.as_str())));
            }
        }
        docs.sort();
        docs.dedup();
        docs
    }

    /// A heading's section id as the website's Markdown (Python-Markdown's toc) makes it: its
    /// own `{#id}`, else its text in lower case, letters, digits, `_` and `-` kept, spaces `-`.
    fn heading_id(line: &str) -> Option<String> {
        let text = line.trim_start_matches('#');
        if text.len() == line.len() || !text.starts_with(' ') {
            return None;
        }
        if let Some((_, id)) = text.split_once("{#") {
            return Some(id.trim_end().trim_end_matches('}').to_owned());
        }
        let link = regex::Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap();
        let kept: String = link
            .replace_all(text, "$1")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ' '))
            .collect::<String>()
            .trim()
            .to_lowercase();
        let dashes = regex::Regex::new(r"[-\s]+").unwrap();
        Some(dashes.replace_all(&kept, "-").into())
    }

    #[test]
    fn every_linked_help_page_and_section_is_in_the_guide() {
        let docs = linked_docs();
        assert!(docs.len() >= 8, "{docs:?}");
        for doc in &docs {
            let (file, anchor) = doc.split_once('#').unwrap_or((doc, ""));
            let Ok(text) = std::fs::read_to_string(root().join("docs").join(file)) else {
                panic!("{doc}: docs/{file} isn't there: the website has no such page");
            };
            if !anchor.is_empty() {
                let ids: Vec<String> = text.lines().filter_map(heading_id).collect();
                assert!(
                    ids.iter().any(|id| id == anchor),
                    "{doc}: no section #{anchor} in docs/{file} ({ids:?})"
                );
            }
        }
    }

    /// The same pages on a built website: `python tools/website/build.py` (MDM_SITE_PREVIEW=1
    /// is enough, English), then `MDM_SITE_DIR=target/site cargo test -p mdm-app --
    /// --ignored linked_help_pages_are_on_the_built_site`: each page is a file there, with its
    /// section's id.
    #[test]
    #[ignore = "needs the website built, in MDM_SITE_DIR"]
    fn linked_help_pages_are_on_the_built_site() {
        let out = std::path::PathBuf::from(std::env::var("MDM_SITE_DIR").expect("MDM_SITE_DIR"));
        for doc in linked_docs() {
            let url = page_url(&doc, "en");
            let path = url.strip_prefix(SITE).unwrap();
            let (path, anchor) = path.split_once('#').unwrap_or((path, ""));
            let mut file = out.join(path.trim_start_matches('/'));
            if path.ends_with('/') {
                file.push("index.html");
            }
            let Ok(html) = std::fs::read_to_string(&file) else {
                panic!("{doc}: {url} isn't built ({})", file.display());
            };
            if !anchor.is_empty() {
                let id = format!("id=\"{anchor}\"");
                assert!(html.contains(&id), "{doc}: {url} has no #{anchor}");
            }
            println!("{doc} -> {url}: ok");
        }
    }
}
