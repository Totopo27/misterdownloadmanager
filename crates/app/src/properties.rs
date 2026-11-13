//! File Properties (double-click / right-click → Properties;
//! notes/ui-spec/scheduler-and-misc.md §5). Light in both modes, as IDM's.

use std::path::{Path, PathBuf};

use egui::{Pos2, Ui, vec2};

use crate::chrome::{IconKey, ShellIcons};
use crate::dialogs::{Outcome, window};
use crate::model::{Item, Status};
use crate::winui::{self, ButtonKind, LIGHT_BG, LIGHT_TEXT, r, text};

/// What OK changes.
pub struct Changes {
    pub id: u64,
    pub description: String,
    pub referer: Option<String>,
    pub login: Option<(String, String)>,
    /// A new address for a download that isn't finished (Refresh download address).
    pub address: Option<url::Url>,
    /// Move the file (finished) or its destination (not started) into this folder.
    pub move_to: Option<PathBuf>,
    /// "Through …" changed (#44): downloaded directly (true) or through the account again.
    pub direct: Option<bool>,
}

pub struct Properties {
    pub id: u64,
    address: String,
    /// A video's file address (#795), read only: its page is the web page it was obtained
    /// from, not an address to change here.
    stream: bool,
    description: String,
    referer: String,
    login: String,
    password: String,
    move_to: Option<PathBuf>,
    error: Option<String>,
    /// The debrid accounts (#44): "Through …" shows while one takes the address.
    route: crate::debrid::Route,
    direct: bool,
    files_requested: bool,
}

impl Properties {
    pub fn new(item: &Item, route: crate::debrid::Route) -> Self {
        let (login, password) = item.login.clone().unwrap_or_default();
        let stream = *item.file_address() != item.url;
        Properties {
            id: item.id,
            address: item.file_address().to_string(),
            stream,
            description: item.description.clone(),
            referer: item.context.referer.clone().unwrap_or_default(),
            login,
            password,
            move_to: None,
            error: None,
            route,
            direct: item.direct,
            files_requested: false,
        }
    }

    pub fn take_files_requested(&mut self) -> bool {
        std::mem::take(&mut self.files_requested)
    }

    /// `item` is read every frame, so status and size stay live.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        icons: &mut ShellIcons,
        item: &Item,
    ) -> Outcome<Changes> {
        let outcome = window(
            ctx,
            &format!("properties-{}", self.id),
            "File Properties",
            [477.0, 420.0],
            false,
            LIGHT_BG(),
            |ui, o| self.body(ui, o, icons, item),
        );
        match outcome {
            Outcome::Done(()) => {
                let address = url::Url::parse(self.address.trim())
                    .ok()
                    .filter(|u| self.address_editable(item) && *u != item.url);
                let referer = self.referer.trim();
                Outcome::Done(Changes {
                    id: self.id,
                    description: self.description.clone(),
                    referer: (!referer.is_empty()).then(|| referer.to_owned()),
                    login: (!self.login.is_empty())
                        .then(|| (self.login.clone(), self.password.clone())),
                    address,
                    move_to: self.move_to.take(),
                    direct: (self.direct != item.direct).then_some(self.direct),
                })
            }
            Outcome::Closed => Outcome::Closed,
            Outcome::Open => Outcome::Open,
        }
    }

    /// The address can be replaced (Refresh download address) while the download isn't finished
    /// or running, unless it isn't a file's address: a video's stream (#795) or a torrent's
    /// magnet link or .torrent file (#904), which are shown read only. The same answer decides
    /// the field and OK's check, so a field that can't be edited never holds OK back.
    fn address_editable(&self, item: &Item) -> bool {
        item.status != Status::Complete
            && item.live.is_none()
            && !self.stream
            && item.torrent.is_none()
    }

    /// OK closes the window: the address, when it can be replaced, is one MDM downloads.
    fn address_ok(&self, item: &Item) -> bool {
        !self.address_editable(item)
            || url::Url::parse(self.address.trim()).is_ok_and(|u| mdm_engine::supported(&u))
    }

    fn body(&mut self, ui: &mut Ui, o: Pos2, icons: &mut ShellIcons, item: &Item) -> Outcome<()> {
        let p = ui.painter().clone();
        let complete = item.status == Status::Complete;
        let running = item.live.is_some();
        // The address can be replaced while the download isn't finished or running.
        let editable = !complete && !running;
        // A running video's stream changes (the next one, a new address): followed.
        if self.stream || item.video.is_some() && running {
            let now = item.file_address();
            self.stream = *now != item.url;
            if self.stream {
                now.as_str().clone_into(&mut self.address);
            }
        }
        let ext = Path::new(&item.file_name)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        icons.paint(ui, IconKey::ExtLarge(ext.clone()), o + vec2(11.0, 11.0));
        let fit = |ui: &mut Ui, x: f32, base: f32, s: &str| {
            let (rect, cut) =
                winui::text_fit(&p, o.x + x, o.y + base, s, LIGHT_TEXT(), o.x + 466.0);
            winui::copyable(ui, rect, ("props", base as i32), s, cut);
        };
        fit(ui, 87.0, 31.0, &item.file_name);
        winui::etched_h(&p, o.x + 11.0, o.y + 50.0, 456.0);
        let size = item.size.map_or_else(String::new, |s| {
            crate::i18n::fmt(
                crate::i18n::t("{size} ({bytes} Bytes)"),
                &[
                    ("size", &crate::dialogs::dialog_size(s)),
                    ("bytes", &crate::i18n::integer(s)),
                ],
            )
        });
        // The values' column: IDM's 87, or after the widest label when a translation is longer
        // (the boxes keep their right edges).
        let col = [
            "Type:",
            "Status:",
            "Size:",
            "Save To:",
            "Address:",
            "Description:",
            "Referer:",
            "Server:",
        ]
        .iter()
        .map(|l| 11.0 + winui::text_width(&p, crate::i18n::t(l)).ceil() + 6.0)
        .fold(87.0_f32, f32::max);
        for (label, base, value) in [
            // i18n: label before the kind of file ("ZIP File")
            (
                crate::i18n::mark("Type:"),
                69.0,
                file_type(&item.file_name, &ext),
            ),
            // i18n: label before the download's state
            (crate::i18n::mark("Status:"), 88.0, item.status_text()),
            (crate::i18n::mark("Size:"), 108.0, size),
        ] {
            text(
                &p,
                o.x + 11.0,
                o.y + base,
                crate::i18n::t(label),
                LIGHT_TEXT(),
            );
            fit(ui, col, base, &value);
        }
        // Where its server was (#140): as the last run saw it, or the running one sees it.
        // i18n: label before where a download's server is (its country and round trip)
        text(
            &p,
            o.x + 11.0,
            o.y + 128.0,
            crate::i18n::t("Server:"),
            LIGHT_TEXT(),
        );
        let seen = item
            .snap
            .as_ref()
            .and_then(|s| s.server.as_ref())
            .map(crate::whereis::Seen::from)
            .or_else(|| item.server.clone());
        // A video's server is its file's once MDM's engine has downloaded it (#769, #795), as
        // the progress window shows it; yt-dlp's own and gallery-dl's are theirs.
        let helpers = item.video.is_some() || item.gallery.is_some();
        if item.torrent.is_some() || helpers && seen.is_none() {
            // i18n: Properties, where a download's server is: a torrent, a video or a gallery has no single server of MDM's own
            fit(
                ui,
                col,
                128.0,
                crate::i18n::t("not one server (peers, or a helper's)"),
            );
        } else if seen.is_some() {
            crate::whereis::line(
                ui,
                o.x + col,
                o.y + 128.0,
                o.x + 466.0,
                seen.as_ref(),
                &format!("props-{}", self.id),
            );
        } else {
            // i18n: Properties, where a download's server is: not seen yet (it hasn't run)
            fit(ui, col, 128.0, crate::i18n::t("known once it has run"));
        }

        text(
            &p,
            o.x + 11.0,
            o.y + 155.0,
            crate::i18n::t("Save To:"),
            LIGHT_TEXT(),
        );
        let dest = match &self.move_to {
            Some(dir) => dir.join(&item.file_name),
            None => item.dest.clone(),
        };
        readonly(
            ui,
            r(o, col, 140.0, 383.0 - col, 23.0),
            &dest.display().to_string(),
        );
        // Moving: a finished file is moved; one that hasn't started gets a new destination.
        let movable = !running && (complete || item.downloaded == 0);
        let pick = winui::button_light(
            ui,
            r(o, 393.0, 141.0, 73.0, 21.0),
            // i18n: verb: move the file to another folder
            "Move",
            if movable {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            },
            false,
        );
        let pick = winui::why_disabled(
            pick,
            !movable,
            if running {
                // i18n: tooltip of the greyed Move button in File Properties
                crate::i18n::mark("Pause the download first")
            } else {
                // i18n: tooltip of the greyed Move button in File Properties
                crate::i18n::mark("Only a finished download, or one not started yet, can be moved")
            },
        )
        .clicked();
        if pick && movable {
            let title = crate::i18n::t("Move to folder").to_owned();
            let from = item.dest.parent().unwrap_or(Path::new(".")).to_path_buf();
            winui::start_pick(ui.ctx(), &format!("move-to-{}", self.id), move || {
                crate::winui::file_dialog(&title)
                    .set_directory(from)
                    .pick_folder()
            });
        }
        if let Some(dir) = winui::picked(ui.ctx(), &format!("move-to-{}", self.id)) {
            self.move_to = Some(dir);
        }

        text(
            &p,
            o.x + 11.0,
            o.y + 186.0,
            crate::i18n::t("Address:"),
            LIGHT_TEXT(),
        );
        let address_editable = self.address_editable(item);
        if address_editable {
            let edit = winui::edit_light(
                ui,
                r(o, col, 171.0, 467.0 - col, 23.0),
                &mut self.address,
                true,
                false,
            );
            if edit.changed() {
                self.error = None;
            }
        } else {
            readonly(ui, r(o, col, 171.0, 467.0 - col, 23.0), &self.address);
        }
        text(
            &p,
            o.x + 11.0,
            o.y + 217.0,
            crate::i18n::t("Description:"),
            LIGHT_TEXT(),
        );
        winui::edit_light(
            ui,
            r(o, col, 202.0, 467.0 - col, 23.0),
            &mut self.description,
            true,
            false,
        );
        winui::etched_h(&p, o.x + 11.0, o.y + 233.0, 456.0);
        text(
            &p,
            o.x + 11.0,
            o.y + 251.0,
            crate::i18n::t("The web page from which this file was obtained:"),
            LIGHT_TEXT(),
        );
        // A video's page, a file's referer (#795).
        // i18n: the web page is not known
        let page = item.web_page().unwrap_or(crate::i18n::t("Unknown"));
        fit(ui, 11.0, 272.0, page);
        winui::etched_h(&p, o.x + 11.0, o.y + 282.0, 456.0);
        // i18n: HTTP term (sic): the page the download was started from
        text(
            &p,
            o.x + 11.0,
            o.y + 308.0,
            crate::i18n::t("Referer:"),
            LIGHT_TEXT(),
        );
        winui::edit_light(
            ui,
            r(o, col, 291.0, 467.0 - col, 23.0),
            &mut self.referer,
            editable,
            false,
        );
        // i18n: noun: label before the user name
        winui::text_right(
            &p,
            o.x + 110.0,
            o.y + 336.0,
            crate::i18n::t("Login"),
            LIGHT_TEXT(),
        );
        winui::edit_light(
            ui,
            r(o, 120.0, 321.0, 189.0, 21.0),
            &mut self.login,
            editable,
            false,
        );
        winui::text_right(
            &p,
            o.x + 110.0,
            o.y + 363.0,
            crate::i18n::t("Password"),
            LIGHT_TEXT(),
        );
        password(
            ui,
            r(o, 120.0, 350.0, 189.0, 21.0),
            &mut self.password,
            editable,
        );
        // Through a debrid account (#44), beside the login: unticked, downloaded directly. It
        // changes while the download isn't running or finished.
        if let Some(service) = self.route.service(&item.url) {
            let mut on = !self.direct;
            let caption = crate::i18n::fmt(
                // i18n: File Properties: a check box beside the login, ticked when the download goes through the user's debrid account; {service} the service, "Real-Debrid". Unticked: downloaded directly
                crate::i18n::t("Through {service}"),
                &[("service", service)],
            );
            winui::checkbox_light_until(
                ui,
                o + vec2(322.0, 329.0),
                &mut on,
                &caption,
                editable,
                Some(o.x + 467.0),
            );
            self.direct = !on;
        }
        winui::etched_h(&p, o.x + 11.0, o.y + 378.0, 456.0);
        if !complete {
            let last = item.last_try.clone().unwrap_or_default();
            let last_try = crate::i18n::fmt(
                // i18n: {last} = the date and time of the last attempt to download
                crate::i18n::t("Last try  {last}"),
                &[("last", &last)],
            );
            fit(ui, 11.0, 396.0, &last_try);
        }
        // The dialog's own error (a bad address) takes the Result line's place while it lasts.
        if let Some(e) = &self.error {
            winui::fit(
                ui,
                &p,
                o.x + 11.0,
                o.y + 415.0,
                e,
                winui::ERROR(),
                o.x + 250.0,
                "props-error",
            );
        } else if !complete {
            let result = item.error.clone().unwrap_or_else(|| item.status_text());
            let x1 = o.x + 250.0;
            let (rect, cut) = winui::text_fit(
                &p,
                o.x + 11.0,
                o.y + 415.0,
                &crate::i18n::fmt(
                    // i18n: {result} = how the last attempt ended (a status or an error message)
                    crate::i18n::t("Result: {result}"),
                    &[("result", &result)],
                ),
                LIGHT_TEXT(),
                x1,
            );
            if cut {
                winui::full_on_hover(ui, rect, "props-result", &result);
            }
        }
        // As wide as its label in this language (#192), into the room before OK.
        let (open_x, open_w) = winui::button_row_from_left(
            &ui.painter().clone(),
            &[("Open", 259.0, 73.0)],
            8.0,
            385.0,
            120.0,
        )[0];
        let open = winui::button_light(
            ui,
            r(o, open_x, 390.0, open_w, 21.0),
            // i18n: verb: opens the file
            "Open",
            if complete {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            },
            false,
        );
        let open = winui::why_disabled(
            open,
            !complete,
            // i18n: tooltip of the greyed Open button in File Properties
            "The download isn't complete yet",
        )
        .clicked();
        if open && complete {
            crate::dialogs::open_download(&item.dest, item.moved_away);
        }
        if item.torrent.is_some()
            && winui::button_light(
                ui,
                r(o, 170.0, 390.0, 78.0, 21.0),
                // i18n: File Properties button that opens the selected torrent's file list
                "Files...",
                ButtonKind::Normal,
                false,
            )
            .clicked()
        {
            self.files_requested = true;
        }
        let ok = winui::button_light(
            ui,
            r(o, 393.0, 390.0, 73.0, 21.0),
            "OK",
            ButtonKind::Default,
            false,
        )
        .clicked();
        let address_ok = self.address_ok(item);
        if winui::escape_pressed(ui) {
            Outcome::Closed
        } else if ok || winui::enter_pressed(ui) {
            if address_ok {
                Outcome::Done(())
            } else {
                self.error =
                    Some(crate::i18n::t("The address must be an http(s) or ftp(s) address").into());
                Outcome::Open
            }
        } else {
            Outcome::Open
        }
    }
}

/// Windows' name for the file type ("BIN File", "Compressed (zipped) Folder").
fn file_type(name: &str, ext: &str) -> String {
    #[cfg(windows)]
    if let Some(t) = crate::win32::type_name(name) {
        return t;
    }
    let _ = name;
    if ext.is_empty() {
        // i18n: noun: a file of no known type
        crate::i18n::t("File").into()
    } else {
        crate::i18n::fmt(
            // i18n: a kind of file, by its extension: {ext} = "ZIP", "PDF"…
            crate::i18n::t("{ext} File"),
            &[("ext", &ext.to_ascii_uppercase())],
        )
    }
}

/// A read-only field holding a path or an address (left-to-right data): the shared one, so it
/// can be copied.
fn readonly(ui: &mut Ui, rc: egui::Rect, value: &str) {
    winui::readonly_field(ui, rc, value, LIGHT_BG(), true, ("props", rc.top() as i32));
}

fn password(ui: &mut Ui, rc: egui::Rect, value: &mut String, enabled: bool) {
    winui::password_light_on(ui, rc, value, enabled, LIGHT_BG());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(url: &str) -> Item {
        let mut item = Item::new(
            1,
            url::Url::parse(url).unwrap(),
            PathBuf::from("file.iso"),
            crate::model::Category::Programs,
        );
        item.status = Status::Stopped;
        item
    }

    /// #904: a torrent's address is its magnet link (or .torrent), shown read only; OK closes
    /// the window instead of asking for an http(s) or ftp(s) address it can't be given.
    #[test]
    fn a_torrents_properties_close_with_ok() {
        let mut torrent = item("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567");
        torrent.torrent = Some(crate::torrents::TorrentSpec {
            hash: "0123456789abcdef0123456789abcdef01234567".into(),
            files: None,
            folder: PathBuf::from("f"),
            uploaded: 0,
            seeded_secs: 0,
            peers: 0,
            up_speed: 0.0,
            upload_limit: None,
            in_order: false,
            ends_first: false,
        });
        let props = Properties::new(&torrent, crate::debrid::Route::default());
        assert!(!props.address_editable(&torrent));
        assert!(props.address_ok(&torrent));
    }

    /// A file's address that isn't finished stays editable, and a bad one still holds OK back.
    #[test]
    fn a_files_address_is_still_checked() {
        let file = item("https://example.com/file.iso");
        let mut props = Properties::new(&file, crate::debrid::Route::default());
        assert!(props.address_editable(&file));
        assert!(props.address_ok(&file));
        "not an address".clone_into(&mut props.address);
        assert!(!props.address_ok(&file));
        "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"
            .clone_into(&mut props.address);
        assert!(!props.address_ok(&file));
        "ftp://example.com/file.iso".clone_into(&mut props.address);
        assert!(props.address_ok(&file));
        let mut done = item("https://example.com/file.iso");
        done.status = Status::Complete;
        assert!(!props.address_editable(&done));
    }
}
