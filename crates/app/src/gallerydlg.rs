//! Download a gallery (#58): a gallery's page (Wikimedia Commons, Flickr, DeviantArt…) given to
//! Add URL opens this window instead of downloading the page as a file. gallery-dl (an optional
//! helper, installed and verified as yt-dlp is, or the person's own) checks it can read the
//! address, offline, and downloads its files into a folder of their own; at most so many files
//! when asked. Started again, it fetches only the files not there yet.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{Pos2, Ui, pos2};

use crate::dialogs::{Outcome, window};
use crate::winui::{self, ButtonKind, r, text};

type Slot<T> = Arc<Mutex<Option<Result<T, String>>>>;

/// What Download Now or Later adds.
pub struct GalleryAdd {
    pub page: url::Url,
    /// The gallery's own folder (Save to, then its name).
    pub folder: PathBuf,
    /// Its name in the list.
    pub name: String,
    pub limit: Option<u64>,
    pub start_now: bool,
}

pub struct GalleryDialog {
    page: String,
    folder: String,
    limit: String,
    install: Option<Slot<mdm_tools::Installed>>,
    /// gallery-dl's answer for the address (`check`), and the address it was for.
    check: Option<Slot<bool>>,
    checked: Option<(String, bool)>,
    error: Option<String>,
    start_now: Option<bool>,
    asking: crate::dialogs::FolderGate<()>,
    helpers: Option<crate::helpers::Programs>,
    /// The helper as found (read again after an install or Helpers, not every frame).
    tool: Option<Option<mdm_tools::Installed>>,
}

fn tool() -> Option<mdm_tools::Installed> {
    mdm_tools::tools_dir()
        .as_deref()
        .and_then(mdm_tools::gallery::installed)
}

impl GalleryDialog {
    pub fn new(folder: PathBuf, page: &str) -> Self {
        GalleryDialog {
            page: page.to_owned(),
            folder: folder.display().to_string(),
            limit: String::new(),
            install: None,
            check: None,
            checked: None,
            error: None,
            start_now: None,
            asking: Default::default(),
            helpers: None,
            tool: None,
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        rt: &tokio::runtime::Handle,
    ) -> Outcome<GalleryAdd> {
        let installed = self.install.as_ref().and_then(|s| s.lock().unwrap().take());
        if let Some(res) = installed {
            self.install = None;
            self.tool = None;
            if let Err(e) = res {
                self.error = Some(crate::i18n::fmt(
                    crate::i18n::t("{what} could not be installed: {error}"),
                    &[("what", "gallery-dl"), ("error", &e)],
                ));
            }
        }
        let answered = self.check.as_ref().and_then(|s| s.lock().unwrap().take());
        if let Some(res) = answered {
            self.check = None;
            match res {
                Ok(ok) => self.checked = Some((self.page.trim().to_owned(), ok)),
                Err(e) => self.error = Some(e),
            }
        }
        let out = window(
            ctx,
            "gallery-page",
            // i18n: window title: download an image gallery's files (gallery-dl)
            "Download a gallery",
            [500.0, 214.0],
            false,
            winui::BG(),
            |ui, o| self.body(ui, o, rt),
        );
        if let Some(h) = &mut self.helpers
            && !matches!(h.show(ctx, rt), Outcome::Open)
        {
            self.helpers = None;
            self.tool = None;
            self.checked = None;
        }
        let folder = crate::dialogs::resolve_path(&self.folder);
        match crate::dialogs::ensure_folder(ctx, &mut self.asking, out, &folder) {
            Outcome::Open => Outcome::Open,
            Outcome::Closed => Outcome::Closed,
            Outcome::Done(()) => {
                let Ok(page) = url::Url::parse(self.page.trim()) else {
                    return Outcome::Closed;
                };
                let name = mdm_tools::gallery::name_of(&page);
                Outcome::Done(GalleryAdd {
                    folder: folder.join(&name),
                    name,
                    page,
                    limit: self.limit.trim().parse().ok().filter(|n| *n > 0),
                    start_now: self.start_now.unwrap_or(true),
                })
            }
        }
    }

    fn body(&mut self, ui: &mut Ui, o: Pos2, rt: &tokio::runtime::Handle) -> Outcome<()> {
        let p = ui.painter().clone();
        let tool = self.tool.get_or_insert_with(tool).clone();
        text(
            &p,
            o.x + 11.0,
            o.y + 21.0,
            crate::i18n::t("Page address"),
            winui::TEXT(),
        );
        winui::edit(
            ui,
            r(o, 11.0, 28.0, 478.0, 21.0),
            &mut self.page,
            true,
            false,
        );
        let page = self.page.trim().to_owned();
        let page_ok = url::Url::parse(&page).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
        // The address checked as soon as gallery-dl is there, and again when it changes.
        if let Some(t) = &tool
            && page_ok
            && self.check.is_none()
            && self.checked.as_ref().is_none_or(|(p, _)| *p != page)
        {
            let slot: Slot<bool> = Arc::new(Mutex::new(None));
            let (out, tool, url, ctx) = (slot.clone(), t.clone(), page.clone(), ui.ctx().clone());
            std::thread::spawn(move || {
                let res = mdm_tools::gallery::handles(&tool, &url).map_err(|e| e.to_string());
                *out.lock().unwrap() = Some(res);
                ctx.request_repaint();
            });
            self.check = Some(slot);
            self.error = None;
        }

        // The status line: gallery-dl missing or installing / checking / what it said.
        let line_y = o.y + 70.0;
        let can_install = mdm_tools::gallery::asset().is_some();
        let readable = self
            .checked
            .as_ref()
            .is_some_and(|(p, ok)| *p == page && *ok);
        let mut status_right = 489.0;
        if self.install.is_some() {
            crate::torrentdlg::spinner(ui, pos2(o.x + 18.0, line_y - 4.0), 6.0);
            let msg = crate::i18n::t("Downloading and verifying gallery-dl...");
            text(&p, o.x + 31.0, line_y, msg, winui::TEXT());
        } else if tool.is_none() {
            let msg = if can_install {
                // i18n: in Download a gallery, before gallery-dl is installed
                crate::i18n::t(
                    "Galleries use gallery-dl (about 23 MB, checked against its signature).",
                )
            } else {
                // i18n: in Download a gallery, on macOS and Linux other than x86-64 (no build to install)
                crate::i18n::t(
                    "Galleries use gallery-dl: install it with your system's packages (Homebrew, pip), then choose it in Helpers.",
                )
            };
            // The Install button, as wide as its label in this language (#192), and the line
            // up to 9 px before it.
            let install_at = can_install.then(|| {
                winui::buttons_to_right(&p, 489.0, 8.0, &[("Install gallery-dl", 90.0)], 200.0)[0]
            });
            if let Some((x, _)) = install_at {
                status_right = x - 9.0;
            }
            let rc = winui::fit(
                ui,
                &p,
                o.x + 11.0,
                line_y,
                msg,
                winui::MUTED(),
                o.x + status_right,
                "gallery-offer",
            );
            winui::readable(ui, rc, "gallery-status", msg);
            if let Some((x, w)) = install_at
                && winui::button(
                    ui,
                    r(o, x, 57.0, w, 21.0),
                    // i18n: button: download and install the gallery-dl program
                    "Install gallery-dl",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked()
                && let Some(dir) = mdm_tools::tools_dir()
            {
                let slot: Slot<mdm_tools::Installed> = Arc::new(Mutex::new(None));
                let (out, ctx) = (slot.clone(), ui.ctx().clone());
                let net = crate::settings::own_net();
                rt.spawn(async move {
                    let res = mdm_tools::gallery::install_latest(&dir, &net, None).await;
                    *out.lock().unwrap() = Some(res.map_err(|e| crate::errors::tools(&e)));
                    ctx.request_repaint();
                });
                self.install = Some(slot);
                self.error = None;
            }
        } else if self.check.is_some() {
            crate::torrentdlg::spinner(ui, pos2(o.x + 18.0, line_y - 4.0), 6.0);
            // i18n: in Download a gallery, while gallery-dl checks the address (offline)
            let msg = crate::i18n::t("Checking the address with gallery-dl...");
            text(&p, o.x + 31.0, line_y, msg, winui::TEXT());
        } else if let Some((_, ok)) = self.checked.as_ref().filter(|(p, _)| *p == page) {
            let (msg, color) = if *ok {
                // i18n: in Download a gallery: gallery-dl can read the address
                (
                    crate::i18n::t("gallery-dl can download this gallery's files."),
                    winui::TEXT(),
                )
            } else {
                // i18n: in Download a gallery: gallery-dl has no way to read the address
                (
                    crate::i18n::t(
                        "gallery-dl can't read this address. Add it as a file instead (Add URL).",
                    ),
                    winui::ERROR(),
                )
            };
            let rc = winui::fit(
                ui,
                &p,
                o.x + 11.0,
                line_y,
                msg,
                color,
                o.x + 489.0,
                "gallery-checked",
            );
            winui::readable(ui, rc, "gallery-status", msg);
        }
        if let Some(e) = &self.error {
            let rc = winui::fit(
                ui,
                &p,
                o.x + 11.0,
                o.y + 88.0,
                e,
                winui::ERROR(),
                o.x + 489.0,
                "gallery-error",
            );
            winui::readable(ui, rc, "gallery-error", e);
        }

        // Save to, and how many files at most.
        let labels = [
            crate::i18n::t("Save to"),
            // i18n: label before the box for how many of a gallery's files to download at most
            crate::i18n::t("Files at most"),
        ];
        let widest = labels
            .iter()
            .map(|l| winui::text_width(&p, l))
            .fold(0.0, f32::max);
        let bx = (11.0 + widest + 6.0).clamp(70.0, 150.0);
        for (i, l) in labels.iter().enumerate() {
            let y = 116.0 + i as f32 * 31.0;
            winui::fit(
                ui,
                &p,
                o.x + 11.0,
                o.y + y,
                l,
                winui::TEXT(),
                o.x + bx - 5.0,
                ("gallery-label", i),
            );
        }
        winui::edit(
            ui,
            r(o, bx, 103.0, 459.0 - bx, 21.0),
            &mut self.folder,
            true,
            false,
        );
        if winui::button(
            ui,
            r(o, 467.0, 103.0, 22.0, 21.0),
            "...",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked()
        {
            let from = self.folder.clone();
            winui::start_pick(ui.ctx(), "gallery-folder", move || {
                rfd::FileDialog::new().set_directory(from).pick_folder()
            });
        }
        if let Some(d) = winui::picked(ui.ctx(), "gallery-folder") {
            self.folder = d.display().to_string();
        }
        self.limit.retain(|c| c.is_ascii_digit());
        winui::edit(
            ui,
            r(o, bx, 134.0, 70.0, 21.0),
            &mut self.limit,
            true,
            false,
        );
        winui::fit(
            ui,
            &p,
            o.x + bx + 78.0,
            o.y + 147.0,
            // i18n: in Download a gallery, after the "Files at most" box
            crate::i18n::t("Empty: all of them. On most sites the newest come first."),
            winui::MUTED(),
            o.x + 489.0,
            "gallery-limit-hint",
        );

        // The bottom row, each button as wide as its label in this language (#192): Helpers at the
        // left, as wide as the room before Download Now allows.
        let [now_at, later_at, cancel_at] = winui::buttons_to_right(
            &p,
            489.0,
            8.0,
            &[("Download Now", 105.0), ("Later", 73.0), ("Cancel", 73.0)],
            160.0,
        )[..] else {
            unreachable!("three buttons")
        };
        let helpers_at = winui::buttons_from_left(
            &p,
            11.0,
            8.0,
            &[("Helpers...", 90.0)],
            now_at.0 - 8.0 - 11.0,
        )[0];
        if winui::button(
            ui,
            r(o, helpers_at.0, 182.0, helpers_at.1, 21.0),
            "Helpers...",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked()
            && self.helpers.is_none()
        {
            self.helpers = Some(crate::helpers::Programs::new());
        }
        let ready = readable && self.install.is_none() && self.check.is_none();
        let kind = |default: bool| match (ready, default) {
            (false, _) => ButtonKind::Disabled,
            (true, true) => ButtonKind::Default,
            (true, false) => ButtonKind::Normal,
        };
        let now = winui::button(
            ui,
            r(o, now_at.0, 182.0, now_at.1, 21.0),
            "Download Now",
            kind(true),
            false,
            None,
        )
        .clicked()
            || (ready && winui::enter_pressed(ui));
        let later = winui::button(
            ui,
            r(o, later_at.0, 182.0, later_at.1, 21.0),
            "Later",
            kind(false),
            false,
            None,
        )
        .clicked();
        let cancel = winui::button(
            ui,
            r(o, cancel_at.0, 182.0, cancel_at.1, 21.0),
            "Cancel",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked();
        if cancel || winui::escape_pressed(ui) {
            return Outcome::Closed;
        }
        if ready && (now || later) {
            self.start_now = Some(now);
            return Outcome::Done(());
        }
        Outcome::Open
    }
}
