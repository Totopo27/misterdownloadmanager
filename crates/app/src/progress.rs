//! The per-download progress dialog, painted to IDM 6.42's pixels
//! (notes/ui-spec/progress-dialog.md). Coordinates are client pixels from the spec.

use egui::{Color32, Pos2, Ui, ViewportBuilder, ViewportId, vec2};
use mdm_engine::Snapshot;
use mdm_engine::download::ConnStatus;

use crate::dialogs::display_path;
use crate::model::{self, Item, Status, size3};
// Laid out in English pixels: `fill` and `px` make each one where it is on screen (bars fill
// from the right in a right-to-left interface), `text` starts at an English pen.
use crate::winui::{self, ButtonKind, fill_l as fill, px_l as px, r, text};

use crate::winui::caption::CaptionButton;

/// The drawn title bar's height (#63): Windows only, where the frame is ours
/// (win32::progress_frame); Linux and macOS keep the system's decorations, which take no buttons
/// of ours (Wayland even places them itself), so the hide button sits in the tab row there.
const TITLE_H: f32 = if cfg!(windows) {
    crate::winui::caption::CAPTION_H
} else {
    0.0
};

/// Extra height added to the top tab pane for the deferred Rename row on Download status.
const RENAME_DROP: f32 = 28.0;

/// The frame applied to a progress window (Windows): its client size, pixels per point × 1000
/// and its system menu's "Hide to the notification area" (translated, when there is one).
type Framed = ([f32; 2], u32, Option<String>);

/// The window's title: IDM truncates the percentage in it (44.51 % → "44%"); without a size to
/// take a share of, only the name. The name keeps its own direction (an Arabic one doesn't pull
/// the percentage after it).
fn window_title(frac: Option<f64>, name: &str) -> String {
    let name = crate::i18n::isolate(name);
    match frac {
        Some(f) => format!("{}% {name}", (f * 100.0).floor() as u32),
        None => name,
    }
}

/// The title bar's buttons, from the window's end: Close, Maximize (greyed: a fixed size, as
/// IDM's), Minimize, and with a tray our "hide to the notification area". None where the system
/// draws the title bar.
fn caption_buttons(tray: bool) -> Vec<(CaptionButton, bool)> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let mut b = vec![
        (CaptionButton::Close, true),
        (CaptionButton::Maximize, false),
        (CaptionButton::Minimize, true),
    ];
    if tray {
        b.push((CaptionButton::ToTray, true));
    }
    b
}

/// The title bar where it's ours (Windows), `width` points wide at `o`; the button pressed. Where
/// its buttons and icon are goes to the window's hit test (win32::set_caption_hits), in client
/// pixels, each frame (`hwnd`: the window, once its frame is applied).
fn draw_caption(
    ui: &mut Ui,
    o: Pos2,
    width: f32,
    title: &str,
    buttons: &[(CaptionButton, bool)],
    hwnd: Option<isize>,
) -> Option<CaptionButton> {
    if !cfg!(windows) {
        return None;
    }
    // The one drawn title bar of every window (dialogs.rs), with its hit test.
    crate::dialogs::drawn_title(ui, o, width, title, buttons, hwnd)
}

/// A greyed label (the limits' captions while off).
fn disabled() -> Color32 {
    crate::theme::colors().progress_label_disabled
}

/// The segment / piece bar's 13 rows, top to bottom: (not here yet, here, a connection's
/// start), from the theme (IDM's gradients in the classic look; its dark variant dims the
/// "not here yet" rows, a bright white strip in the dark window).
fn seg_rows() -> impl Iterator<Item = (Color32, Color32, Color32)> {
    let s = &crate::theme::colors().segments;
    (0..13).map(|i| (s.missing[i], s.done[i], s.start[i]))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Status,
    Limiter,
    Completion,
}

impl Progress {
    /// Open on a tab by its English name in lower case: a top tab ("status", "limit", "done") or
    /// a torrent's details tab ("peers"; /show progress:peers, for the docs' screenshots and the
    /// fit audit); any other name leaves it as it is. A top tab hidden with "Hide tab" comes back.
    pub fn show_tab(&mut self, name: &str) {
        let top = match name {
            "status" => Some(Tab::Status),
            "limit" => Some(Tab::Limiter),
            "done" => Some(Tab::Completion),
            _ => None,
        };
        if let Some(t) = top {
            self.hidden_tabs.retain(|h| *h != t);
            self.tab = t;
            return;
        }
        let tab = match name {
            "files" => TTab::Files,
            "peers" => TTab::Peers,
            "trackers" => TTab::Trackers,
            "pieces" => TTab::Pieces,
            "general" => TTab::General,
            _ => return,
        };
        self.ttab = tab;
        self.details = true;
    }
}

/// A torrent's details tabs (M15, D-014), in the place of the connections table.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TTab {
    Files,
    Peers,
    Trackers,
    Pieces,
    General,
}

/// Why "Hang up modem when done" is greyed off Windows.
pub const NO_DIAL_UP: &str = crate::i18n::mark("This system has no dial-up connections to hang up");

/// The window's frames while its bar glides ([`Glide`]): as often as the bar moves a pixel, 25 a
/// second at most, only while its download runs and its figures move ([`glide_frame`]).
const GLIDE_FRAME: std::time::Duration = std::time::Duration::from_millis(40);
/// The same while MDM's main window is shown: each frame draws it again too, about three times a
/// progress window's work (measured: 25 a second then took 5 % of a core more, 12.5 half that).
const GLIDE_FRAME_MAIN: std::time::Duration = std::time::Duration::from_millis(80);
/// The app's own frames while a download runs (app.rs): a bar slower than a pixel in this long
/// needs no frames of its own.
const APP_FRAME: f64 = 0.25;
/// The bar's width (513 points less its frame) and the segment map's (511).
const BAR_WIDTH: f64 = 511.0;
/// A figure reached in about the time the last one took to come, a quarter more (seconds): the
/// next one comes before the bar has stopped, so it never halts between figures that keep
/// coming. Within these bounds (a figure after a long pause glides in half a second).
const GLIDE_SLACK: f64 = 1.25;
const GLIDE_MIN: f64 = 0.05;
const GLIDE_MAX: f64 = 0.5;
/// Figures that moved this recently (seconds) keep the frames coming, so the next one is drawn
/// as it comes, not at the app's next quarter second.
const GLIDE_HOLD: f64 = 1.0;
/// How long a finished download's window shows it whole before it gives way to Download
/// complete (app.rs): its bar, a glide behind the figures, would otherwise close short of the
/// end.
pub const SHOWN_WHOLE: std::time::Duration = std::time::Duration::from_millis(300);

/// The bar's fill and the segment map between the download's figures, which come about four
/// times a second: each part moves from where it was drawn to its new figure over about the
/// time between figures, so the bar glides where it jumped (the text keeps the figures as they
/// are). Never past a figure, never back: a figure lower than what is drawn (a download started
/// over) is drawn at once. The bar and the map move on the same clock, so they agree as their
/// figures do (a video's map is its bar; a file's map counts what is written, its bar also what
/// has come and waits to be, as before).
#[derive(Default)]
struct Glide {
    /// The figures last read: bytes done, each segment's start and bytes done.
    done: u64,
    segments: Vec<(u64, u64)>,
    /// Where the drawing set off from towards them (the same segments, in order).
    from_done: f64,
    from_segments: Vec<f64>,
    /// When they were read (egui's clock, seconds) and how long it takes to reach them.
    at: f64,
    span: f64,
    seen: bool,
    /// The bar's speed on its way to them (bytes a second).
    rate: f64,
}

/// What a frame draws ([`Glide::step`]).
struct Drawn {
    done: f64,
    segments: Vec<(u64, f64)>,
    /// How fast the bar moves (bytes a second), while it is on its way to the figures or they
    /// moved lately; else 0: no frames of its own wanted.
    rate: f64,
}

/// When the progress window wants its next frame while its bar moves at `rate` bytes a second
/// of `total`, on a screen of `ppp` pixels a point: when the bar has moved a pixel, 25 frames a
/// second at most (12.5 while the main window is shown, `main`); none when it moves less than a
/// pixel between the app's own frames (a slow download, an unknown size, a bar standing still).
fn glide_frame(rate: f64, total: Option<u64>, ppp: f32, main: bool) -> Option<std::time::Duration> {
    let total = total.filter(|t| *t > 0)? as f64;
    let pixels = rate / total * BAR_WIDTH * f64::from(ppp);
    let every = 1.0 / pixels;
    let cap = if main { GLIDE_FRAME_MAIN } else { GLIDE_FRAME };
    (pixels > 0.0 && every < APP_FRAME).then(|| std::time::Duration::from_secs_f64(every).max(cap))
}

impl Glide {
    /// How far the drawing is from `from` to the figures at `now` (0 to 1).
    fn part(&self, now: f64) -> f64 {
        if self.span <= 0.0 {
            1.0
        } else {
            ((now - self.at) / self.span).clamp(0.0, 1.0)
        }
    }

    /// The figures read at `now` (`done` bytes; the segments' starts and bytes done): what to
    /// draw. `finished`: the download is done, its figures drawn as they are at once (the
    /// window shows it whole before it gives way to Download complete, not a glide behind).
    fn step(&mut self, now: f64, done: u64, segments: &[(u64, u64)], finished: bool) -> Drawn {
        if !self.seen || finished {
            self.seen = true;
            self.at = now;
            self.span = 0.0;
            self.rate = 0.0;
            self.from_done = done as f64;
            self.from_segments = segments.iter().map(|&(_, d)| d as f64).collect();
            self.done = done;
            self.segments = segments.to_vec();
        } else if done != self.done || segments != self.segments.as_slice() {
            // Off from where it is drawn now; a part whose figure went below that, at once.
            let u = self.part(now);
            let at = |from: f64, to: u64| from + (to as f64 - from) * u;
            let drawn = at(self.from_done, self.done);
            let drawn_segments: Vec<(u64, f64)> = self
                .segments
                .iter()
                .zip(&self.from_segments)
                .map(|(&(start, d), &from)| (start, at(from, d)))
                .collect();
            // A segment that comes (a connection split one) grows from nothing, as the bar's
            // part of it does: the map's segments together stay what the bar shows.
            self.from_segments = segments
                .iter()
                .map(|&(start, d)| {
                    drawn_segments
                        .iter()
                        .find(|(s, _)| *s == start)
                        .map_or(0.0, |&(_, x)| x.min(d as f64))
                })
                .collect();
            let done_f = done as f64;
            self.from_done = drawn.min(done_f);
            self.span = ((now - self.at) * GLIDE_SLACK).clamp(GLIDE_MIN, GLIDE_MAX);
            self.rate = (done_f - self.from_done) / self.span;
            self.at = now;
            self.done = done;
            self.segments = segments.to_vec();
        }
        let u = self.part(now);
        let at = |from: f64, to: u64| from + (to as f64 - from) * u;
        let busy = u < 1.0 || now - self.at < GLIDE_HOLD;
        Drawn {
            done: at(self.from_done, self.done),
            segments: self
                .segments
                .iter()
                .zip(&self.from_segments)
                .map(|(&(start, d), &from)| (start, at(from, d)))
                .collect(),
            rate: if busy { self.rate } else { 0.0 },
        }
    }
}

pub struct Progress {
    pub id: u64,
    details: bool,
    tab: Tab,
    /// Tabs taken off the strip with their "Hide tab" button (#326), for this window.
    hidden_tabs: Vec<Tab>,
    ttab: TTab,
    limiter: bool,
    /// KBytes/sec typed on the Speed Limiter tab.
    limit_kb: String,
    /// The tab has been initialised from the download's current limit.
    limit_init: bool,
    /// A limit change to report to the app (`Action::Limit`), and whether to remember it.
    pending_limit: Option<(Option<u64>, bool)>,
    remember: bool,
    /// A torrent's own upload limit on the Speed Limiter tab: on, and its KBytes/sec box.
    up_on: bool,
    up_kb: String,
    /// An upload limit change to report (`Action::UploadLimit`).
    pending_upload: Option<Option<u64>>,
    /// Files ticked or unticked in a torrent's Files tab, to report (`Action::Choose`).
    pending_choose: Option<Action>,
    show_complete: bool,
    /// Torrent settings' "Keep sharing torrents after they finish" (the General tab says so).
    pub keep_sharing: bool,
    /// The torrent session (the app's), for a finished torrent still shared without a `Run`:
    /// its tabs read the session directly.
    pub host: Option<crate::torrents::Host>,
    /// Hidden in the notification area (#63): not shown, its tab and choices kept for when it
    /// comes back (the tray's "Downloads in progress", or the download double-clicked).
    pub hidden: bool,
    /// A tray shows MDM's icon, so the window can be hidden there (set by the app each frame).
    pub tray: bool,
    /// Options → General → "Minimize to the notification area": Minimize hides it there too.
    pub minimize_to_tray: bool,
    /// MDM's main window is shown (set by the app each frame): the glide's frames draw it too.
    pub main_shown: bool,
    /// Options on completion, used when "Show download complete dialog" is off.
    hang_up: bool,
    exit: bool,
    turn_off_on: bool,
    turn_off: usize,
    force: bool,
    /// The bar and the segment map between figures.
    glide: Glide,
    /// Deferred rename on Download status: initialised from `Item::rename_on_complete`, saved
    /// by clicking Confirm, pressing Enter, or leaving the box idle for 3 seconds.
    rename_init: bool,
    rename_text: String,
    rename_saved: Option<String>,
    rename_edited_at: Option<std::time::Instant>,
    rename_show_saved: bool,
    pending_rename: Option<Option<String>>,
    /// Translated reason key when deferred rename is unavailable for this download (shared or
    /// save folder, split archive volume, or single-file torrent).
    pub rename_disabled_reason: Option<&'static str>,
    /// In-memory state stamp for `rename_disabled_reason`, so disk checks run only when the
    /// window opens, when a download completes, or when list/folder settings change.
    pub rename_guard_stamp: Option<u64>,
    /// Those disk checks, under way off the window's thread for a newer stamp (#743): a folder
    /// on a sleeping drive or a network share can take seconds to answer. The reason shown
    /// stays the last one meanwhile (the rename is checked again when it is carried out).
    pub rename_guard: Option<(u64, crate::launch::Asked<u64, Option<&'static str>>)>,
}

pub enum Action {
    None,
    Pause,
    Resume,
    /// Stop the download and close the dialog.
    Cancel,
    /// Set this download's speed limit (bytes/s, None = off).
    /// This download's limit (None: unlimited); `true`: remember it for the file.
    Limit(Option<u64>, bool),
    /// A torrent's own upload limit (bytes/s, None: off), kept with it (`TorrentSpec`).
    UploadLimit(Option<u64>),
    /// A torrent's files to download from now on (#39, its Files tab): indexes into its list,
    /// whether that is every file, and their size together.
    Choose {
        files: Vec<usize>,
        all: bool,
        size: u64,
    },
    /// Set or clear the deferred rename (`Item::rename_on_complete`).
    Rename(Option<String>),
    /// Close the dialog, keep downloading.
    Close,
    /// Hide the dialog in the notification area, keep downloading (#63).
    Hide,
}

impl Progress {
    /// `show_complete`: Options' "Show download complete dialog", this file's first state.
    pub fn new(id: u64, show_complete: bool) -> Self {
        Progress {
            id,
            details: true,
            tab: Tab::Status,
            hidden_tabs: Vec::new(),
            ttab: TTab::Files,
            limiter: false,
            limit_kb: "10".into(),
            limit_init: false,
            pending_limit: None,
            remember: false,
            up_on: false,
            up_kb: "50".into(),
            pending_upload: None,
            pending_choose: None,
            show_complete,
            keep_sharing: false,
            host: None,
            hidden: false,
            tray: false,
            minimize_to_tray: false,
            main_shown: true,
            hang_up: false,
            exit: false,
            turn_off_on: false,
            turn_off: 0,
            force: false,
            glide: Glide::default(),
            rename_init: false,
            rename_text: String::new(),
            rename_saved: None,
            rename_edited_at: None,
            rename_show_saved: false,
            pending_rename: None,
            rename_disabled_reason: None,
            rename_guard_stamp: None,
            rename_guard: None,
        }
    }

    fn status_drop(&self) -> f32 {
        let _ = self;
        RENAME_DROP
    }

    /// Synchronise the Rename box from `item.rename_on_complete` on first display and whenever
    /// `item` changes while idle.
    pub fn sync_rename_from_item(&mut self, item: &Item) {
        let idle = self.rename_edited_at.is_none() && self.pending_rename.is_none();
        if !self.rename_init || (idle && self.rename_saved != item.rename_on_complete) {
            self.rename_init = true;
            self.rename_saved = item.rename_on_complete.clone();
            self.rename_text = item.rename_on_complete.clone().unwrap_or_default();
            self.rename_show_saved = self.rename_saved.is_some();
        }
    }

    fn commit_rename(&mut self, norm: Option<String>) {
        self.rename_saved = norm.clone();
        self.rename_edited_at = None;
        self.rename_show_saved = norm.is_some();
        self.pending_rename = Some(norm);
    }

    fn on_rename_text_changed(&mut self, now: std::time::Instant) {
        match crate::deferred_rename::validate_name(&self.rename_text) {
            Ok(norm) => {
                if norm == self.rename_saved {
                    self.rename_edited_at = None;
                    self.rename_show_saved = self.rename_saved.is_some();
                } else {
                    self.rename_show_saved = false;
                    self.rename_edited_at = Some(now);
                }
            }
            Err(_) => {
                self.rename_show_saved = false;
                self.rename_edited_at = None;
            }
        }
    }

    /// Check whether the 3-second idle auto-save timer has elapsed; returns the remaining wait if
    /// still counting down so the UI schedules a repaint.
    fn poll_rename_autosave(&mut self, now: std::time::Instant) -> Option<std::time::Duration> {
        let edited_at = self.rename_edited_at?;
        let Ok(norm) = crate::deferred_rename::validate_name(&self.rename_text) else {
            self.rename_edited_at = None;
            return None;
        };
        if norm == self.rename_saved {
            self.rename_edited_at = None;
            self.rename_show_saved = self.rename_saved.is_some();
            return None;
        }
        let elapsed = now.saturating_duration_since(edited_at);
        if elapsed >= crate::deferred_rename::AUTOSAVE_DELAY {
            self.commit_rename(norm);
            None
        } else {
            Some(crate::deferred_rename::AUTOSAVE_DELAY - elapsed)
        }
    }

    /// Flush any rename that was already confirmed (Confirm, Enter, assistive input, or the
    /// 3-second idle auto-save); half-typed unconfirmed text is never applied.
    pub fn take_confirmed_rename(&mut self) -> Option<Option<String>> {
        self.pending_rename.take()
    }

    pub fn show(&mut self, ctx: &egui::Context, item: &Item, snap: Option<&Snapshot>) -> Action {
        crate::crash::window("progress");
        let frac = item.fraction();
        let title = window_title(frac, &item.file_name);
        // The bar and the segment map glide between the figures (the text shows them as they
        // are): frames come faster for that only while the download runs and its figures move.
        let segments: Vec<(u64, u64)> = match snap {
            Some(s) if !s.segments.is_empty() => s
                .segments
                .iter()
                .map(|g| (g.start, g.cursor.saturating_sub(g.start)))
                .collect(),
            _ => vec![(0, item.downloaded)],
        };
        let drawn = self.glide.step(
            ctx.input(|i| i.time),
            item.downloaded,
            &segments,
            item.status == Status::Complete,
        );
        // Never past the fraction: a video's bar holds short of the end until it is finished,
        // as its percentage does ([`Item::fraction`]); no size, no fill.
        let bar = match (frac, item.size.filter(|t| *t > 0)) {
            (Some(f), Some(t)) => (drawn.done / t as f64).clamp(0.0, 1.0).min(f),
            _ => 0.0,
        };
        let drop = self.status_drop();
        let body = if self.details {
            [548.0, 412.0 + drop + torrent_drop(item)]
        } else {
            [548.0, 241.0 + drop]
        };
        // Windows: the title bar is ours (#63), drawn in the client above IDM's layout, so the
        // hide button sits in it, left of Minimize, as in IDM. Elsewhere the system draws it.
        let client = [body[0], body[1] + TITLE_H];
        // As large as the screen allows at the interface size (#262): the rest scrolls.
        let shown = crate::dialogs::shown_size(ctx, client);
        let vid = ViewportId::from_hash_of(("progress", self.id));
        let buttons = caption_buttons(self.tray);
        let key = egui::Id::unique(("progress-pos", self.id));
        let title_now = title.clone();
        // Hidden until dialogs::keep_theme has given it its theme.
        let mut builder = crate::quiet(ViewportBuilder::default())
            .with_visible(false)
            // The taskbar shows it: without isolates (#919).
            .with_title(crate::i18n::os_text(&title))
            .with_inner_size(shown)
            // IDM's progress window has 2 px borders and a fixed size. On Windows the sizing
            // frame is asked for and win32::progress_frame trims its borders, disables resizing
            // and gives the caption to the client, which draws it; elsewhere the window is simply
            // not resizable (its contents are laid out for one size: Show/Hide details switches
            // between two).
            .with_resizable(cfg!(windows))
            .with_icon(crate::app_icon())
            .with_maximize_button(false);
        if let Some(p) = crate::dialogs::place(ctx, key, [548.0, 412.0 + drop]) {
            builder = builder.with_position(p);
        }
        // Minimize hides it in the notification area when asked (and a tray can bring it back).
        let minimize_hides = self.tray && self.minimize_to_tray;
        // The window, once its frame is applied (Windows: its caption parts go to its hit test).
        let hwnd = ctx.data(|d| d.get_temp::<isize>(key.with("hwnd")));
        let action = ctx.show_viewport_immediate(vid, builder, |ui, _| {
            // Painted in the pass that closes it too (see dialogs::close_requested).
            let close = crate::dialogs::close_requested(ui);
            if !close && minimize_hides && ui.ctx().input(|i| i.viewport().minimized == Some(true))
            {
                return Action::Hide;
            }
            let action = egui::CentralPanel::no_frame()
                .frame(egui::Frame::new().fill(winui::BG()))
                .show(ui, |ui| {
                    let o = ui.max_rect().min;
                    // The window's width, laid out from the right in a right-to-left
                    // interface (the title bar; the layout under it has its own).
                    let _surface = winui::Surface::enter(o.x, o.x + shown[0]);
                    let caption = draw_caption(ui, o, shown[0], &title_now, &buttons, hwnd);
                    // The layout under the title bar, scrolled when the window is smaller.
                    let below = egui::Rect::from_min_max(o + vec2(0.0, TITLE_H), ui.max_rect().max);
                    let mut rest = ui.new_child(egui::UiBuilder::new().max_rect(below));
                    let action = crate::dialogs::scroll(&mut rest, key, body, |ui, o| {
                        let _surface = winui::Surface::enter(o.x, o.x + body[0]);
                        self.body(ui, o, item, snap, frac, (bar, &drawn.segments))
                    });
                    // The system menu's "Hide to the notification area" (Alt+Space).
                    #[cfg(windows)]
                    if let Some(h) = hwnd
                        && crate::win32::take_hide_asked(h)
                    {
                        return Action::Hide;
                    }
                    match caption {
                        Some(CaptionButton::Close) => Action::Close,
                        Some(CaptionButton::ToTray) => Action::Hide,
                        Some(CaptionButton::Minimize) if minimize_hides => Action::Hide,
                        Some(CaptionButton::Minimize) => {
                            crate::dialogs::minimize(ui.ctx(), hwnd);
                            action
                        }
                        _ => action,
                    }
                })
                .inner;
            if close {
                return Action::Close;
            }
            // Escape hides the window (the download goes on), as the × does.
            if matches!(action, Action::None) && winui::escape_pressed(ui) {
                Action::Close
            } else {
                action
            }
        });
        // Elsewhere the window manager is asked for its size at a new interface size (#262).
        crate::dialogs::follow_scale(ctx, vid, key, shown);
        // The glide's frames: while the download runs, its figures move and the window can be
        // seen (minimized, or hidden in the tray where it isn't drawn at all, it waits).
        let minimized = ctx.input_for(vid, |i| i.viewport().minimized) == Some(true);
        let ppp = ctx.input_for(vid, |i| i.pixels_per_point);
        if item.live.is_some()
            && !minimized
            && let Some(every) = glide_frame(drawn.rate, item.size, ppp, self.main_shown)
        {
            ctx.request_repaint_after(every);
        }
        // Apply the frame while the window is still hidden, and again whenever the client size
        // (details), the scale (another display, the interface size) or the system menu's item
        // changes.
        #[cfg(windows)]
        {
            let scale = ctx.input_for(vid, |i| i.pixels_per_point);
            let hide = buttons
                .iter()
                .any(|(b, _)| *b == CaptionButton::ToTray)
                .then(|| crate::i18n::t("Hide to the notification area").to_owned());
            let want: Framed = (shown, (scale * 1000.0).round() as u32, hide);
            if ctx
                .data(|d| d.get_temp::<Framed>(key.with("framed")))
                .as_ref()
                != Some(&want)
                && let Some(h) =
                    crate::win32::thin_frame(&title_now, shown, scale, want.2.as_deref())
            {
                ctx.data_mut(|d| {
                    d.insert_temp(key.with("framed"), want);
                    d.insert_temp(key.with("hwnd"), h);
                });
            }
        }
        let closing = matches!(action, Action::Close | Action::Cancel | Action::Hide);
        crate::dialogs::keep_theme(
            ctx,
            vid,
            key,
            &title_now,
            crate::theme::dark_surface(),
            !closing,
        );
        let action = match (action, self.pending_limit.take()) {
            (Action::None, Some((limit, remember))) => Action::Limit(limit, remember),
            (a, _) => a,
        };
        // An upload change waits a frame when a download one goes first (one action a frame);
        // so does a change of files.
        let action = match action {
            Action::None => self
                .pending_upload
                .take()
                .map_or(Action::None, Action::UploadLimit),
            a => a,
        };
        let action = match action {
            Action::None => self.pending_choose.take().unwrap_or(Action::None),
            a => a,
        };
        let action = match action {
            Action::None => self
                .pending_rename
                .take()
                .map_or(Action::None, Action::Rename),
            a => a,
        };
        if matches!(action, Action::Close | Action::Cancel) {
            crate::dialogs::forget_place(ctx, key);
        }
        // Gone (hidden keeps its place): the next window gets its thin frame again.
        if closing {
            #[cfg(windows)]
            if let Some(h) = hwnd {
                crate::win32::forget_caption(h);
            }
            ctx.data_mut(|d| {
                d.remove::<Framed>(key.with("framed"));
                d.remove::<isize>(key.with("hwnd"));
            });
        }
        action
    }

    /// `frac`: the download's fraction as it is (the text); `drawn`: the bar's fill and the
    /// segments as they glide ([`Glide`]).
    fn body(
        &mut self,
        ui: &mut Ui,
        o: Pos2,
        item: &Item,
        snap: Option<&Snapshot>,
        frac: Option<f64>,
        drawn: (f64, &[(u64, f64)]),
    ) -> Action {
        let p = ui.painter().clone();
        self.sync_rename_from_item(item);
        if let Some(wait) = self.poll_rename_autosave(std::time::Instant::now()) {
            ui.ctx()
                .request_repaint_after(wait + std::time::Duration::from_millis(15));
        }
        let hide = self.tabs(ui, o);
        let hide_tab = match self.tab {
            Tab::Status => {
                self.status_page(ui, o, item, snap, frac);
                false
            }
            Tab::Limiter => self.limiter_page(ui, o, item),
            Tab::Completion => self.completion_page(ui, o, item),
        };
        if hide_tab {
            self.hide(self.tab);
            // The strip above was drawn with the tab still on it: draw the window again at once,
            // without showing this pass, so the tab goes in one step. A download's window
            // otherwise waits for its next timed repaint (250 ms while it downloads) on the old page.
            ui.ctx().request_discard("Hide tab");
            ui.ctx().request_repaint();
        }

        let lower = o + vec2(0.0, self.status_drop());

        // The progress bar: IDM's green on the shared, themed track. After the last byte, while
        // the file is checked or a video joined (#736), it starts again in the accent colour and
        // shows how far that is.
        let bar = r(lower, 17.0, 180.0, 513.0, 16.0);
        match finishing_fraction(snap) {
            Some(f) => winui::meter_in(&p, bar, f as f32, crate::theme::colors().accent),
            None => winui::meter(&p, bar, drawn.0 as f32),
        }

        let mut action = if hide { Action::Hide } else { Action::None };
        let details_label = if self.details {
            crate::i18n::mark("<< Hide details")
        } else {
            crate::i18n::mark("Show details >>")
        };
        let other_tab = self.tab != Tab::Status;
        let kind = if other_tab {
            ButtonKind::Default
        } else {
            ButtonKind::Normal
        };
        // The site sent its page instead of the file (#701): a button opens the link in the
        // browser, which gets past the page (its download then comes back to MDM).
        let web_page = item.got_web_page();
        // i18n: button in a download's progress window when the site sent a web page (a waiting page, a captcha) instead of the file: opens the download's link in the web browser
        let open_label = crate::i18n::mark("Open in browser");
        // The row as wide as its labels in this language need (#192), each laid out for the
        // longer of its two labels so it keeps its size when they change; Cancel stays at the
        // window's edge.
        let mut row = vec![
            (
                winui::widest_label(&p, &["<< Hide details", "Show details >>"]),
                30.0,
                138.0,
            ),
            (winui::widest_label(&p, &["Pause", "Resume"]), 311.0, 82.0),
            ("Cancel", 422.0, 73.0),
        ];
        if web_page {
            row.insert(1, (open_label, 176.0, 127.0));
        }
        let placed = winui::button_row(&p, &row, 8.0, 30.0, 200.0);
        let (details_at, open_at, pause_at, cancel_at) = match placed[..] {
            [details, open, pause, cancel] => (details, Some(open), pause, cancel),
            [details, pause, cancel] => (details, None, pause, cancel),
            _ => unreachable!("three or four buttons"),
        };
        if let Some(open_at) = open_at
            && winui::button(
                ui,
                r(lower, open_at.0, 206.0, open_at.1, 22.0),
                open_label,
                // The way on: the default, unless another tab's Show details is.
                if other_tab {
                    ButtonKind::Normal
                } else {
                    ButtonKind::Default
                },
                false,
                None,
            )
            .clicked()
        {
            crate::launch::open_page(item.url.as_str());
        }
        if winui::button(
            ui,
            r(lower, details_at.0, 206.0, details_at.1, 22.0),
            details_label,
            kind,
            false,
            None,
        )
        .clicked()
        {
            self.details = !self.details;
        }
        let running = item.status == Status::Downloading;
        let can_resume = matches!(item.status, Status::Stopped | Status::Error);
        let (label, pkind) = if running {
            // i18n: verb: pause the download
            (crate::i18n::mark("Pause"), ButtonKind::Normal)
        } else if can_resume {
            // i18n: verb: continue the download
            (crate::i18n::mark("Resume"), ButtonKind::Normal)
        } else {
            (crate::i18n::mark("Pause"), ButtonKind::Disabled)
        };
        if winui::button(
            ui,
            r(lower, pause_at.0, 206.0, pause_at.1, 22.0),
            label,
            pkind,
            false,
            None,
        )
        .clicked()
        {
            action = if running {
                Action::Pause
            } else {
                Action::Resume
            };
        }
        if winui::button(
            ui,
            r(lower, cancel_at.0, 206.0, cancel_at.1, 22.0),
            "Cancel",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked()
        {
            action = Action::Cancel;
        }

        if self.details && item.torrent.is_some() {
            self.torrent_details(ui, &p, lower + vec2(0.0, torrent_drop(item)), item);
        } else if self.details {
            winui::text_in_window(
                ui,
                &p,
                lower.x + 137.0,
                lower.y + 242.0,
                crate::i18n::t("Start positions and download progress by connections"),
                winui::TEXT(),
            );
            segment_bar(&p, lower, item, drawn.1);
            connections(ui, &p, lower, snap);
        }
        action
    }

    /// Classic 3-D tabs over a raised page frame (spec "Tab strip"); with a tray, the "hide to
    /// the notification area" button at the row's end. True when that button is pressed.
    fn tabs(&mut self, ui: &mut Ui, o: Pos2) -> bool {
        // The shared tab strip (the torrent tabs' too): one look in both themes, one Tab stop with
        // the arrows, sized to the translated labels. It had two of its own (classic 3-D in dark).
        let tabs: Vec<(Tab, &str)> = [
            // i18n: tab title: how the download is going
            (Tab::Status, crate::i18n::mark("Download status")),
            // i18n: tab title: the tool that limits the download speed
            (Tab::Limiter, crate::i18n::mark("Speed Limiter")),
            // i18n: tab title: what happens when the download finishes
            (Tab::Completion, crate::i18n::mark("Options on completion")),
        ]
        .into_iter()
        .filter(|t| !self.hidden_tabs.contains(&t.0))
        .collect();
        let p = ui.painter().clone();
        let english: Vec<&str> = tabs.iter().map(|t| t.1).collect();
        // The hide button sits here where the system draws the title bar (Linux, macOS; on
        // Windows it's in ours); the tabs end before it (long translations shorten first).
        let tab_button = self.tray && !cfg!(windows);
        let right = if tab_button { 500.0 } else { 525.0 };
        let light: Vec<winui::LightTab> = winui::tab_row(&p, &english, 20.0, right, 2.0, 8.0)
            .into_iter()
            .zip(tabs.iter())
            .map(|((x0, x1, _), (_, label))| winui::LightTab {
                label,
                sel: (x0 - 2.0, x1 + 2.0),
                unsel: (x0, x1),
            })
            .collect();
        let current = tabs.iter().position(|t| t.0 == self.tab).unwrap_or(0);
        let drop = self.status_drop();
        // The page: x 18..529, y 28..(171 + drop) (tabs_light's pane starts 20 px under its top).
        if let Some(i) =
            winui::tabs_light(ui, o, r(o, 18.0, 8.0, 514.0, 165.0 + drop), &light, current)
            && tabs[i].0 != self.tab
        {
            self.tab = tabs[i].0;
        }
        self.tab_row_menu(ui, r(o, 18.0, 6.0, 514.0, 22.0));
        // MDM's (#63): IDM puts this in its title bar, which here is the system's own (it takes
        // no buttons of ours), so it sits at the tab row's end, over the page's corner.
        tab_button
            && winui::to_tray_button(
                ui,
                r(o, 507.0, 6.0, 22.0, 20.0),
                "progress-hide",
                // i18n: tooltip of a button in a download's progress window: hides the window to the notification area (system tray); the download goes on
                crate::i18n::mark("Hide to the notification area"),
            )
            .clicked()
    }

    /// The tab row's right-click menu (#326): "Show hidden tabs" puts back the tabs hidden in
    /// this window, greyed while none is. From the keyboard too, on the focused tab: Shift+F10
    /// or the Menu key.
    fn tab_row_menu(&mut self, ui: &Ui, row: egui::Rect) {
        use crate::chrome::{MenuEntry, MenuKind, PopupResult};
        let ctx = ui.ctx();
        let key = egui::Id::unique(("progress-tab-menu", self.id));
        let by_pointer = ui.input(|i| {
            i.pointer
                .secondary_clicked()
                .then(|| i.pointer.interact_pos())
                .flatten()
                .filter(|p| row.contains(*p))
        });
        let by_keys = || {
            let tab = ctx
                .memory(|m| m.focused())
                .and_then(|f| ctx.read_response(f))
                .map(|r| r.rect)
                .filter(|r| row.contains(r.center()))?;
            ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::SHIFT, egui::Key::F10)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Menu)
            })
            // Under the tab, from its leading edge (a menu's top-right in right to left).
            .then(|| {
                if winui::rtl() {
                    tab.right_bottom()
                } else {
                    tab.left_bottom()
                }
            })
        };
        if let Some(at) = by_pointer.or_else(by_keys) {
            ctx.data_mut(|d| d.insert_temp(key, at));
        }
        let Some(at) = ctx.data(|d| d.get_temp::<Pos2>(key)) else {
            return;
        };
        let entries = [MenuEntry::Item(
            // i18n: right-click menu of a download window's tab row: bring back the tabs taken off with "Hide tab"
            "&Show hidden tabs",
            (),
            !self.hidden_tabs.is_empty(),
            false,
        )];
        let id = format!("progress-tab-menu-{}", self.id);
        match crate::chrome::popup(ctx, MenuKind::Context, &id, at, &entries, &|_| None) {
            PopupResult::Open | PopupResult::Neighbor(_) => return,
            PopupResult::Chosen(()) => {
                self.show_hidden_tabs();
                ctx.request_repaint();
            }
            PopupResult::Dismissed => {}
        }
        ctx.data_mut(|d| d.remove::<Pos2>(key));
    }

    /// Puts every tab hidden with "Hide tab" back on the strip, in its own place.
    fn show_hidden_tabs(&mut self) {
        self.hidden_tabs.clear();
    }

    /// Takes `tab` off the strip (its "Hide tab" button, #326) and shows Download status, which
    /// has no such button and so always stays.
    fn hide(&mut self, tab: Tab) {
        if tab != Tab::Status && !self.hidden_tabs.contains(&tab) {
            self.hidden_tabs.push(tab);
        }
        self.tab = Tab::Status;
    }

    /// The Speed Limiter tab, live for this download. Returns true when "Hide tab" is pressed.
    fn limiter_page(&mut self, ui: &mut Ui, o: Pos2, item: &Item) -> bool {
        let p = ui.painter().clone();
        if !self.limit_init {
            if let Some(rate) = item.torrent.as_ref().and_then(|t| t.upload_limit) {
                self.up_on = true;
                self.up_kb = (rate / 1024).max(1).to_string();
            }
            if let Some(rate) = item.live.as_ref().and_then(|l| l.dl.speed_limit()) {
                self.limiter = true;
                self.limit_kb = (rate / 1024).max(1).to_string();
            }
            // A remembered limit: the box is ticked (and the limit shows even while stopped).
            if let Some(rate) = item.own_limit {
                self.limiter = true;
                self.limit_kb = (rate / 1024).max(1).to_string();
                self.remember = true;
            }
            self.limit_init = true;
        }
        text(
            &p,
            o.x + 33.0,
            o.y + 51.0,
            crate::i18n::t("Transfer rate"),
            winui::TEXT(),
        );
        let rate = if item.status == Status::Downloading && !item.past_last_byte() {
            model::rate(item.rate())
        } else {
            String::new()
        };
        text(&p, o.x + 136.0, o.y + 51.0, &rate, winui::TEXT());
        let was_on = self.limiter;
        // A torrent's upload limit takes the right half (x 290): the download side's captions
        // stop before it.
        let torrent = item.torrent.is_some();
        winui::checkbox_until(
            ui,
            o + vec2(33.0, 64.0),
            &mut self.limiter,
            "Use Speed Limiter",
            true,
            torrent.then_some(286.0),
        );
        let label = if self.limiter {
            winui::TEXT()
        } else {
            disabled()
        };
        winui::fit(
            ui,
            &p,
            o.x + 33.0,
            o.y + 92.0,
            crate::i18n::t("Maximum download speed:"),
            label,
            o.x + if torrent { 284.0 } else { 525.0 },
            ("limit-label", self.id),
        );
        let rate_box = winui::edit_right(
            ui,
            r(o, 33.0, 100.0, 77.0, 23.0),
            &mut self.limit_kb,
            self.limiter,
        );
        self.limit_kb.retain(|c| c.is_ascii_digit());
        // A typed rate applies once typed (focus leaves, Enter), not at each digit (typing 100
        // would throttle to 1, then 10 KB/s on the way). A rate a screen reader or UI Automation
        // sets (Set value) applies at once: there is no typing to wait for, and often no focus
        // to leave, so it never applied.
        let typed = rate_box.lost_focus()
            || (rate_box.has_focus() && winui::enter_pressed(ui))
            || winui::value_set_by_assistive(&rate_box);
        // i18n: unit after an input box: kilobytes per second
        text(
            &p,
            o.x + 118.0,
            o.y + 113.0,
            crate::i18n::t("KBytes/sec"),
            label,
        );
        let remembered = self.remember;
        let remember_box = winui::checkbox(ui, o + vec2(33.0, 134.0), &mut self.remember, "", true);
        // Its caption is drawn apart (two lines, below): screen readers get it as its name.
        let name = format!(
            "{} {}",
            crate::i18n::t("Remember Speed Limiter settings for this file"),
            crate::i18n::t("on download stop/resume")
        );
        winui::expose(
            &remember_box,
            egui::Role::CheckBox,
            &name,
            true,
            Some(self.remember),
        );
        // Its caption is two lines drawn below; a click on them ticks it too.
        winui::caption_click(
            ui,
            r(o, 51.0, 127.0, 450.0, 27.0),
            "remember-limit",
            &mut self.remember,
            true,
        );
        // An empty or zero rate isn't "unlimited" while the box is ticked: the limit stays
        // as it was until a rate is typed.
        let kb: u64 = self.limit_kb.parse().unwrap_or(0);
        if (was_on != self.limiter || (self.limiter && typed) || remembered != self.remember)
            && !(self.limiter && kb == 0)
        {
            let limit = self.limiter.then_some(kb * 1024);
            self.pending_limit = Some((limit, self.remember));
        }
        text(
            &p,
            o.x + 51.0,
            o.y + 137.0,
            crate::i18n::t("Remember Speed Limiter settings for this file"),
            winui::TEXT(),
        );
        text(
            &p,
            o.x + 51.0,
            o.y + 150.0,
            crate::i18n::t("on download stop/resume"),
            winui::TEXT(),
        );
        if item.torrent.is_some() {
            self.upload_limit(ui, &p, o);
        }
        hide_tab(ui, o)
    }

    /// A torrent's own upload limit, right of the download one (#39): kept with the torrent,
    /// so it applies again whenever it runs or shares.
    fn upload_limit(&mut self, ui: &mut Ui, p: &egui::Painter, o: Pos2) {
        let was_on = self.up_on;
        winui::checkbox_until(
            ui,
            o + vec2(290.0, 64.0),
            &mut self.up_on,
            // i18n: a torrent's progress window, Speed Limiter tab: a limit for what this torrent sends to others
            "Limit uploading",
            true,
            Some(525.0),
        );
        let label = if self.up_on {
            winui::TEXT()
        } else {
            disabled()
        };
        winui::fit(
            ui,
            p,
            o.x + 290.0,
            o.y + 92.0,
            // i18n: a torrent's progress window, Speed Limiter tab: label above the upload speed box
            crate::i18n::t("Maximum upload speed:"),
            label,
            o.x + 525.0,
            ("upload-label", self.id),
        );
        let rate_box = winui::edit_right(
            ui,
            r(o, 290.0, 100.0, 77.0, 23.0),
            &mut self.up_kb,
            self.up_on,
        );
        self.up_kb.retain(|c| c.is_ascii_digit());
        let typed = rate_box.lost_focus()
            || (rate_box.has_focus() && winui::enter_pressed(ui))
            || winui::value_set_by_assistive(&rate_box);
        text(
            p,
            o.x + 375.0,
            o.y + 113.0,
            crate::i18n::t("KBytes/sec"),
            label,
        );
        let kb: u64 = self.up_kb.parse().unwrap_or(0);
        if (was_on != self.up_on || (self.up_on && typed)) && !(self.up_on && kb == 0) {
            self.pending_upload = Some(self.up_on.then_some(kb * 1024));
        }
    }

    /// Options on completion → "Show download complete dialog" for this file.
    pub fn shows_complete(&self) -> bool {
        self.show_complete
    }

    /// Options on completion's other settings, when they apply (the complete dialog is off).
    pub fn when_done(&self) -> Option<crate::queue::WhenDone> {
        (!self.show_complete).then(|| crate::queue::WhenDone {
            hang_up: self.hang_up,
            exit: self.exit,
            turn_off: self
                .turn_off_on
                .then(|| crate::queue::TurnOff::ALL[self.turn_off]),
            force: self.turn_off_on && self.force,
        })
    }

    /// Returns true when "Hide tab" is pressed.
    fn completion_page(&mut self, ui: &mut Ui, o: Pos2, item: &Item) -> bool {
        let p = ui.painter().clone();
        // The label as wide as its language makes it (half the row at most), then the path to
        // the row's end, shortened in the middle so the file's name stays in sight (#62).
        let (left, right) = (o.x + 33.0, o.x + 33.0 + 474.0);
        let label = winui::fit(
            ui,
            &p,
            left,
            o.y + 45.0,
            crate::i18n::t("Save To:"),
            winui::TEXT(),
            left + 237.0,
            "save-to-label",
        );
        // From the label's width: its rect is mirrored in a right-to-left interface.
        let at = (left + label.width() + 6.0).max(o.x + 90.0);
        crate::fileicons::paint_at(
            ui,
            &p,
            crate::fileicons::Kind::File(&item.file_name),
            egui::pos2(at, o.y + 33.0),
            16.0,
        );
        winui::fit_middle(
            ui,
            &p,
            at + crate::fileicons::INDENT,
            o.y + 45.0,
            &display_path(&item.dest),
            winui::TEXT(),
            right,
            "save-to-path",
        );
        winui::checkbox(
            ui,
            o + vec2(33.0, 55.0),
            &mut self.show_complete,
            "Show download complete dialog",
            true,
        );
        winui::etched_h(&p, o.x + 33.0, o.y + 74.0, 474.0);
        text(
            &p,
            o.x + 33.0,
            o.y + 89.0,
            crate::i18n::t(
                "These settings are unavailable when \"Show download complete dialog\" is turned on",
            ),
            winui::TEXT(),
        );
        // Used when the complete dialog is off (it asks what to do itself).
        let on = !self.show_complete;
        // Dial-up is hung up through Windows' rasdial; macOS and Linux have no such command.
        let hang_up = winui::checkbox(
            ui,
            o + vec2(33.0, 96.0),
            &mut self.hang_up,
            "Hang up modem when done",
            on && cfg!(windows),
        );
        winui::why_disabled(hang_up, on && !cfg!(windows), NO_DIAL_UP);
        winui::checkbox(
            ui,
            o + vec2(33.0, 114.0),
            &mut self.exit,
            "Exit Mister Download Manager when done",
            on,
        );
        winui::checkbox_until(
            ui,
            o + vec2(33.0, 132.0),
            &mut self.turn_off_on,
            "Turn off computer when done",
            on,
            Some(o.x + 267.0),
        );
        winui::checkbox(
            ui,
            o + vec2(52.0, 148.0),
            &mut self.force,
            "Force processes to terminate",
            on && self.turn_off_on,
        );
        if let Some(i) = winui::combo_light(
            ui,
            r(o, 267.0, 134.0, 119.0, 21.0),
            &crate::queue::TurnOff::LABELS,
            self.turn_off,
            on && self.turn_off_on,
            "progress-turn-off",
        ) {
            self.turn_off = i;
        }
        hide_tab(ui, o)
    }
}

/// "Hide tab" anchored at the bottom-right corner of the tab pane (`135 + RENAME_DROP`).
fn hide_tab(ui: &mut Ui, o: Pos2) -> bool {
    // A longer translation ("Registerkarte ausblenden") widens it to the left, into the free
    // room right of the Remember caption (which ends before x 300).
    let rc = winui::button_rect_for(
        ui.painter(),
        r(o, 401.0, 135.0 + RENAME_DROP, 96.0, 21.0),
        "Hide tab",
        190.0,
    );
    winui::button(ui, rc, "Hide tab", ButtonKind::Normal, false, None).clicked()
}

/// Right end for text on the tab page (its frame is x 18..529).
const PAGE_TEXT_RIGHT: f32 = 520.0;

impl Progress {
    // The rows sit 7 px inside the page's frame at the top and at the bottom (with Rename right
    // below Server), with a slightly wider break after Status, as before.
    fn status_page(
        &mut self,
        ui: &mut Ui,
        o: Pos2,
        item: &Item,
        snap: Option<&Snapshot>,
        frac: Option<f64>,
    ) {
        let p = &ui.painter().clone();
        // Long values are cut with "..." at the page edge and shown whole on hover.
        let fit = |ui: &Ui, key: &str, x: f32, base: f32, s: &str, color| {
            let (rect, cut) =
                winui::text_fit(p, o.x + x, o.y + base, s, color, o.x + PAGE_TEXT_RIGHT);
            winui::copyable(ui, rect, key, s, cut);
        };
        let web_page = item.got_web_page();
        // A failure that takes a paragraph to explain: the reason under "Error", in the rows of
        // the sizes and times, which say nothing then (a site's page, #701; a video's failed
        // merge, what stays for Resume).
        let explained = web_page || item.merge_failed();
        let status = match item.status {
            // A torrent reading and checking its files says so, not "Receiving data" (#39).
            _ if let Some(checking) = item.torrent_checking() => checking,
            // After the last byte: the file checked, a video's streams joined (#736), as the
            // list says, not "Connecting...".
            _ if let Some(finishing) = item.finishing(false) => finishing,
            // A torrent its debrid service is downloading (#44).
            Status::Downloading if item.at_service().is_some() => {
                item.at_service().unwrap_or_default()
            }
            Status::Downloading => {
                // A video (yt-dlp) has no engine connections: data arriving is the sign.
                let receiving = snap.is_some_and(|s| {
                    s.connections
                        .iter()
                        .any(|c| matches!(c.status, ConnStatus::Receiving))
                }) || ((item.video.is_some() || item.torrent.is_some())
                    && item.rate() > 0.0);
                // Every byte in and none on its way (#736): the last of it written out, the
                // file named; nothing is connecting.
                let completing = item.torrent.is_none()
                    && snap.is_some_and(|s| {
                        s.state == mdm_engine::download::State::Completing
                            || (s.size.is_some_and(|t| t > 0 && s.downloaded >= t)
                                && !s
                                    .connections
                                    .iter()
                                    .any(|c| matches!(c.status, ConnStatus::Connecting)))
                    });
                if completing {
                    // i18n: a download's status: every byte is in, the file is being written out and given its name
                    crate::i18n::t("Finishing...")
                } else if receiving {
                    crate::i18n::t("Receiving data...")
                } else {
                    crate::i18n::t("Connecting...")
                }
                .to_owned()
            }
            // i18n: a download's status (state it is in, not a command): finished
            Status::Complete => crate::i18n::t("Complete").to_owned(),
            // i18n: a download's status (state it is in, not a command): stopped for now
            Status::Stopped => crate::i18n::t("Paused").to_owned(),
            // i18n: a download's status (state it is in, not a command): waiting in a queue
            Status::Queued => crate::i18n::t("Waiting").to_owned(),
            // The site's page instead of the file (#701): why, and what to do, under it.
            Status::Error if explained => crate::i18n::t("Error").to_owned(),
            Status::Error => crate::i18n::fmt(
                // i18n: a download's status: it failed, with the reason
                crate::i18n::t("Error: {error}"),
                &[(
                    "error",
                    // i18n: the reason a download failed is not known
                    item.error.as_deref().unwrap_or(crate::i18n::t("unknown")),
                )],
            ),
        };
        fit(
            ui,
            "url",
            33.0,
            44.0,
            // A video's stream, not its page (#795).
            &crate::i18n::ltr(item.file_address().as_str()),
            winui::TEXT(),
        );
        let rows: [(&str, f32, f32, String); 6] = [
            // i18n: noun: label before the download's state
            (crate::i18n::mark("Status"), 61.0, 102.0, status),
            (
                crate::i18n::mark("File size"),
                80.0,
                135.0,
                item.size.map_or(String::new(), size3),
            ),
            (
                // i18n: label before the amount received so far
                crate::i18n::mark("Downloaded"),
                96.0,
                135.0,
                match frac {
                    // No size: only how much has come, no 0 % that never moves.
                    None => size3(item.downloaded),
                    Some(frac) if crate::i18n::code() == "en" => {
                        format!("{} ( {:.2} % )", size3(item.downloaded), frac * 100.0) // not translated: English path (IDM's spacing)
                    }
                    Some(frac) => format!(
                        "{} ( {} )",
                        size3(item.downloaded),
                        crate::i18n::percent(frac, 2)
                    ),
                },
            ),
            (
                crate::i18n::mark("Transfer rate"),
                112.0,
                135.0,
                // Every byte in (#736): nothing comes in while the file is checked or joined.
                if item.status == Status::Downloading && !item.past_last_byte() {
                    model::rate(item.rate())
                } else {
                    String::new()
                },
            ),
            (
                crate::i18n::mark("Time left"),
                128.0,
                135.0,
                item.time_left(),
            ),
            (
                // i18n: label: whether the server lets a stopped download continue (Yes / No)
                crate::i18n::mark("Resume capability"),
                145.0,
                135.0,
                match item.resumable {
                    Some(true) => crate::i18n::t("Yes"),
                    Some(false) => crate::i18n::t("No"),
                    None => "",
                }
                .to_owned(),
            ),
        ];
        // A value starts 6 px after its label at the least (a long translation); the rows that share
        // the column at 135 (all but Status) keep sharing one.
        let after = |label: &str| 33.0 + winui::text_width(p, crate::i18n::t(label)).ceil() + 6.0;
        let column = rows
            .iter()
            .filter(|r| r.2 == 135.0)
            .map(|r| after(r.0))
            .fold(135.0, f32::max);
        for (label, base, vx, value) in rows {
            // A site's page instead of the file (#701): its sizes and times say nothing, the
            // reason takes their rows.
            if explained && label != "Status" {
                continue;
            }
            let vx = if vx == 135.0 {
                column
            } else {
                vx.max(after(label))
            };
            text(
                p,
                o.x + 33.0,
                o.y + base,
                crate::i18n::t(label),
                winui::TEXT(),
            );
            // Light mode shows the status value in pure blue (light-dialogs.md).
            let vc = if label == "Status" {
                crate::theme::colors().status_value
            } else {
                winui::TEXT()
            };
            fit(ui, label, vx, base, &value, vc);
        }
        if explained {
            winui::paragraph(
                p,
                o.x + 33.0,
                o.y + 80.0,
                &crate::i18n::sentence_start(item.error.as_deref().unwrap_or_default()),
                winui::TEXT(),
                o.x + PAGE_TEXT_RIGHT,
                16.0,
            );
        }
        // Where its server is (#140): a file download's, and a video's whose streams MDM's engine
        // downloads (#769; a torrent or a gallery has no single server of MDM's own). As the
        // running download sees it, else as its last run did.
        let seen = snap
            .and_then(|s| s.server.as_ref())
            .map(crate::whereis::Seen::from)
            .or_else(|| item.server.clone());
        // A video yt-dlp downloads itself (#769): why, in that row.
        if let Some(why) = item.video_helper() {
            fit(ui, "helper", 33.0, 161.0, &helper_line(&why), winui::TEXT());
        } else if item.torrent.is_none()
            && item.gallery.is_none()
            && (item.video.is_none() || seen.is_some())
        {
            // i18n: label before where a download's server is (its country and round trip)
            let label = crate::i18n::t("Server");
            text(p, o.x + 33.0, o.y + 161.0, label, winui::TEXT());
            crate::whereis::line(
                ui,
                o.x + column.max(33.0 + winui::text_width(p, label).ceil() + 6.0),
                o.y + 161.0,
                o.x + PAGE_TEXT_RIGHT,
                seen.as_ref(),
                &format!("progress-{}", item.id),
            );
        }
        self.rename_row(ui, p, o, column, item);
    }

    fn rename_row(&mut self, ui: &mut Ui, p: &egui::Painter, o: Pos2, column: f32, item: &Item) {
        let top = 171.0;
        let base = top + 13.0;
        // i18n: noun: label before the deferred new file or folder name in the progress window
        let rename_label = crate::i18n::t("Rename");
        // i18n: verb: button that saves the deferred rename in the progress window
        let confirm_key = crate::i18n::mark("Confirm");
        // i18n: status shown in green beside the Rename box once the new name is saved
        let saved_text = crate::i18n::t("Saved");

        text(p, o.x + 33.0, o.y + base, rename_label, winui::TEXT());

        let saved_w = winui::text_width(p, saved_text).ceil().max(38.0);
        let btn_w = (winui::text_width(p, crate::i18n::t(confirm_key)) + 20.0)
            .ceil()
            .max(52.0);
        let saved_x = PAGE_TEXT_RIGHT - saved_w;
        let btn_x = saved_x - 8.0 - btn_w;
        let edit_x = column.max(33.0 + winui::text_width(p, rename_label).ceil() + 6.0);
        let edit_w = (btn_x - 6.0 - edit_x).max(100.0);

        let disabled_reason = (item.status != Status::Complete)
            .then_some(self.rename_disabled_reason)
            .flatten();
        let enabled = item.status != Status::Complete && disabled_reason.is_none();
        let edit_rc = r(o, edit_x, top, edit_w, 21.0);
        let edit_resp = winui::edit(ui, edit_rc, &mut self.rename_text, enabled, false);
        if edit_resp.changed() {
            self.on_rename_text_changed(std::time::Instant::now());
            if let Some(wait) = self.poll_rename_autosave(std::time::Instant::now()) {
                ui.ctx()
                    .request_repaint_after(wait + std::time::Duration::from_millis(15));
            }
        }

        let validated = crate::deferred_rename::validate_name(&self.rename_text);
        let error_reason = if enabled {
            validated.as_ref().err().copied().map(rename_error_message)
        } else {
            None
        };
        if error_reason.is_some() {
            winui::error_frame(p, edit_rc);
        }

        let dirty = match &validated {
            Ok(norm) => *norm != self.rename_saved,
            Err(_) => false,
        };
        let can_confirm = enabled
            && validated.is_ok()
            && (dirty || (!self.rename_text.trim().is_empty() && !self.rename_show_saved));
        let btn_kind = if can_confirm {
            ButtonKind::Normal
        } else {
            ButtonKind::Disabled
        };
        let btn_resp = winui::button(
            ui,
            r(o, btn_x, top, btn_w, 21.0),
            confirm_key,
            btn_kind,
            false,
            None,
        );
        let clicked = btn_resp.clicked();
        let enter = edit_resp.has_focus() && winui::take_key(ui, egui::Key::Enter);
        let assistive = winui::value_set_by_assistive(&edit_resp);

        if let Some(reason_key) = error_reason.or(disabled_reason) {
            let why = crate::i18n::t(reason_key);
            ui.interact(
                edit_rc,
                egui::Id::unique(("progress-rename-reason", self.id)),
                egui::Sense::hover(),
            )
            .on_hover_text(why);
            edit_resp
                .clone()
                .on_hover_text(why)
                .on_disabled_hover_text(why);
            btn_resp.on_hover_text(why);
            if error_reason.is_some() && edit_resp.has_focus() {
                egui::Tooltip::for_widget(&edit_resp).show(|ui| {
                    ui.label(why);
                });
            }
        }

        if enabled
            && (clicked || enter || assistive)
            && let Ok(norm) = validated
            && (dirty || norm.is_some())
        {
            self.commit_rename(norm);
        }

        if enabled && self.rename_show_saved {
            text(p, o.x + saved_x, o.y + base, saved_text, winui::SUCCESS());
        }
    }
}

fn rename_error_message(err: crate::deferred_rename::RenameError) -> &'static str {
    use crate::deferred_rename::RenameError;
    match err {
        // i18n: reason shown on the Rename box when the entered name ends with a period
        RenameError::TrailingDot => crate::i18n::mark("A file name cannot end with a dot"),
        // i18n: reason shown on the Rename box when the entered name exceeds the length limit
        RenameError::TooLong => crate::i18n::mark("The name is too long"),
        // i18n: reason shown on the Rename box when the entered name has a forbidden character
        RenameError::InvalidChar => {
            crate::i18n::mark("This character is not allowed in a file name")
        }
        // i18n: reason shown on the Rename box when the entered name is a reserved device name on Windows (CON, NUL, COM1…)
        RenameError::ReservedDevice => crate::i18n::mark("This name is reserved by Windows"),
    }
}

/// How much lower a torrent's details sit than a download's (and how much taller its window
/// is): its tab strip, where the connections caption is, would touch the buttons above.
fn torrent_drop(item: &Item) -> f32 {
    if item.torrent.is_some() { 6.0 } else { 0.0 }
}

impl Progress {
    /// A torrent's details: a tab strip where the connections caption is, the piece map in
    /// the segment bar, and the tab's table below.
    fn torrent_details(&mut self, ui: &mut Ui, p: &egui::Painter, o: Pos2, item: &Item) {
        let d = match &item.live {
            Some(crate::model::Live {
                dl: crate::model::Job::Torrent(run),
                ..
            }) => Some(run.details()),
            // Finished and still shared (no Run): the session has it all the same, so the
            // tabs stay, the General tab's share ratio included.
            _ => self
                .host
                .as_ref()
                .and_then(|h| h.get())
                .zip(item.torrent.as_ref())
                .and_then(|(t, spec)| crate::torrents::details_of(&t, &spec.hash)),
        };
        let tabs = [
            // i18n: tab title: the torrent's files
            (TTab::Files, crate::i18n::mark("Files")),
            // i18n: tab title: the other computers sharing the torrent
            (TTab::Peers, crate::i18n::mark("Peers")),
            // i18n: tab title: the servers that list the peers
            (TTab::Trackers, crate::i18n::mark("Trackers")),
            // i18n: tab title: the parts the torrent is made of
            (TTab::Pieces, crate::i18n::mark("Pieces")),
            // i18n: tab title: general information
            (TTab::General, crate::i18n::mark("General")),
        ];
        // The usual tab control (the Scheduler's and Options' look): tabs sized to their
        // labels from the leading edge, the page under them holding the piece map and the
        // table.
        let english: Vec<&str> = tabs.iter().map(|t| t.1).collect();
        let light: Vec<winui::LightTab> = winui::tab_row(p, &english, 16.0, 530.0, 2.0, 8.0)
            .into_iter()
            .zip(tabs.iter())
            .map(|((x0, x1, _), (_, label))| winui::LightTab {
                label,
                sel: (x0 - 2.0, x1 + 2.0),
                unsel: (x0, x1),
            })
            .collect();
        let current = tabs.iter().position(|t| t.0 == self.ttab).unwrap_or(0);
        if let Some(i) = winui::tabs_light(ui, o, r(o, 14.0, 226.0, 521.0, 180.0), &light, current)
        {
            self.ttab = tabs[i].0;
        }
        let pieces = d.as_ref().map(|d| d.pieces.as_slice()).unwrap_or(&[]);
        piece_bar(p, o, pieces);
        let Some(d) = &d else {
            table(ui, p, o, "t-none", &[(25.0, "")], &[], &[], false, None);
            text(
                p,
                o.x + 25.0,
                o.y + 312.0,
                crate::i18n::t("Shown while the torrent is downloading."),
                winui::TEXT(),
            );
            return;
        };
        match self.ttab {
            TTab::Files => {
                let rows: Vec<Vec<String>> = d
                    .files
                    .iter()
                    .map(|(_, f, on, done)| {
                        let pct = if !on {
                            // i18n: a torrent file the user chose not to download
                            crate::i18n::t("Skipped").to_owned()
                        } else if f.size == 0 {
                            crate::i18n::percent(1.0, 0)
                        } else {
                            crate::i18n::percent(*done as f64 / f.size as f64, 1)
                        };
                        vec![f.path.display().to_string(), model::size(f.size), pct]
                    })
                    .collect();
                let ticks: Vec<bool> = d.files.iter().map(|f| f.2).collect();
                let clicked = files_table(ui, p, o, "t-files", &rows, &ticks, 6);
                // A box ticked or unticked (#39): the torrent downloads those files from now
                // on; the last one ticked can't be unticked (a torrent of nothing).
                if let Some(row) = clicked {
                    let mut on: Vec<bool> = ticks.clone();
                    on[row] = !on[row];
                    if on.iter().any(|b| *b) {
                        let chosen = d.files.iter().zip(&on).filter(|(_, b)| **b);
                        self.pending_choose = Some(Action::Choose {
                            files: chosen.clone().map(|(f, _)| f.0).collect(),
                            all: on.iter().all(|b| *b),
                            size: chosen.map(|(f, _)| f.1.size).sum(),
                        });
                    }
                }
            }
            TTab::Peers => {
                let rows: Vec<Vec<String>> = d
                    .peers
                    .iter()
                    .map(|q| {
                        vec![
                            q.addr.clone(),
                            q.client.clone().unwrap_or_default(),
                            model::size(q.fetched),
                            model::size(q.uploaded),
                        ]
                    })
                    .collect();
                table(
                    ui,
                    p,
                    o,
                    "t-peers",
                    &[
                        // i18n: column title: the web address (URL)
                        (25.0, crate::i18n::mark("Address")),
                        // i18n: column title: the peer's torrent program
                        (190.0, crate::i18n::mark("Client")),
                        // i18n: column title: how much came from the peer
                        (340.0, crate::i18n::mark("Received")),
                        // i18n: column title: how much went to the peer
                        (425.0, crate::i18n::mark("Sent")),
                    ],
                    &[185.0, 335.0, 420.0],
                    &rows,
                    false,
                    None,
                );
            }
            TTab::Trackers => {
                let rows: Vec<Vec<String>> = if d.trackers.is_empty() {
                    vec![vec![
                        crate::i18n::t("None: peers come from the DHT and other peers").into(),
                    ]]
                } else {
                    d.trackers.iter().map(|t| vec![t.clone()]).collect()
                };
                // i18n: column title: a server that lists the peers
                let head = [(25.0, crate::i18n::mark("Tracker"))];
                table(ui, p, o, "t-trackers", &head, &[], &rows, false, None);
            }
            TTab::Pieces => {
                let have = pieces.iter().filter(|b| **b).count();
                let head = crate::i18n::fmt(
                    // i18n: {have} and {total} = numbers of pieces; {size} = one piece's size
                    crate::i18n::t("{have} of {total} pieces ({size} each)"),
                    &[
                        ("have", &crate::i18n::integer(have as u64)),
                        ("total", &crate::i18n::integer(pieces.len() as u64)),
                        ("size", &model::size(u64::from(d.piece_length))),
                    ],
                );
                table(
                    ui,
                    p,
                    o,
                    "t-pieces",
                    &[(25.0, head.as_str())],
                    &[],
                    &[],
                    false,
                    None,
                );
                piece_legend(p, o);
                piece_grid(p, o, pieces);
            }
            TTab::General => {
                let st = d.stats.as_ref();
                let have = pieces.iter().filter(|b| **b).count();
                // What it sent in all its runs (#39; librqbit counts only this one), against what
                // it has: the same after a pause or a restart.
                let spec = item.torrent.as_ref();
                let sent = spec.map_or(0, |t| t.uploaded);
                let done = st.map_or(item.downloaded, |s| s.done);
                let ratio = crate::torrents::share_ratio_text(sent, done);
                let shared = spec.map_or(0, |t| t.seeded_secs);
                let hash = item
                    .torrent
                    .as_ref()
                    .map_or(String::new(), |t| t.hash.clone());
                let folder = item
                    .torrent
                    .as_ref()
                    .map_or(String::new(), |t| t.folder.display().to_string());
                let t = |s: &str| crate::i18n::t(s).to_owned();
                let rows = vec![
                    // i18n: the torrent's identifier
                    vec![t("Info hash"), hash],
                    vec![
                        t("Pieces"),
                        crate::i18n::fmt(
                            // i18n: {have} and {total} = numbers of pieces; {size} = one piece's size
                            crate::i18n::t("{have} of {total} ({size} each)"),
                            &[
                                ("have", &crate::i18n::integer(have as u64)),
                                ("total", &crate::i18n::integer(pieces.len() as u64)),
                                ("size", &model::size(u64::from(d.piece_length))),
                            ],
                        ),
                    ],
                    vec![
                        t("Peers"),
                        st.map_or(String::new(), |s| {
                            crate::i18n::fmt(
                                // i18n: {live} = peers connected now; {known} = peers heard of
                                crate::i18n::t("{live} connected, {known} known"),
                                &[
                                    ("live", &crate::i18n::integer(s.peers_live as u64)),
                                    ("known", &crate::i18n::integer(s.peers_seen as u64)),
                                ],
                            )
                        }),
                    ],
                    vec![
                        // i18n: how much was sent to other peers
                        t("Sent"),
                        model::size(sent),
                    ],
                    // i18n: sent divided by received
                    vec![t("Share ratio"), ratio],
                    vec![
                        // i18n: a torrent's General tab: how long it has been shared once finished, in all
                        t("Shared for"),
                        model::time(shared as f64),
                    ],
                    // i18n: label before the folder the torrent is saved in
                    vec![t("Saved in"), folder],
                    vec![
                        // i18n: noun: sharing the torrent with others after it is downloaded
                        t("Sharing"),
                        if self.keep_sharing {
                            // i18n: a torrent's sharing, when Torrent settings keep it on
                            t("Goes on after the download finishes")
                        } else {
                            t("Stops when the download finishes")
                        },
                    ],
                ];
                table(
                    ui,
                    p,
                    o,
                    "t-general",
                    &[
                        (25.0, crate::i18n::mark("Property")),
                        (130.0, crate::i18n::mark("Value")),
                    ],
                    &[125.0],
                    &rows,
                    false,
                    None,
                );
            }
        }
    }
}

/// The segment bar as a piece map: a column is filled when the pieces under it are there.
fn piece_bar(p: &egui::Painter, o: Pos2, pieces: &[bool]) {
    let (x0, y0) = (o.x + 17.0, o.y + 249.0);
    let border = crate::theme::colors().segment_border;
    fill(p, x0, y0, x0 + 512.0, y0, border);
    fill(p, x0, y0 + 14.0, x0 + 512.0, y0 + 14.0, border);
    fill(p, x0, y0, x0, y0 + 14.0, border);
    fill(p, x0 + 512.0, y0, x0 + 512.0, y0 + 14.0, border);
    let ix = o.x + 18.0;
    // Runs of equal columns, painted as one rectangle per row.
    let have = |c: usize| {
        if pieces.is_empty() {
            return false;
        }
        let a = c * pieces.len() / 511;
        let b = ((c + 1) * pieces.len() / 511).max(a + 1).min(pieces.len());
        pieces[a..b].iter().all(|x| *x)
    };
    let mut c = 0usize;
    while c < 511 {
        let on = have(c);
        let mut e = c;
        while e + 1 < 511 && have(e + 1) == on {
            e += 1;
        }
        for (row, cols) in seg_rows().enumerate() {
            let v = if on { cols.1 } else { cols.0 };
            let y = y0 + 1.0 + row as f32;
            fill(p, ix + c as f32, y, ix + e as f32, y, v);
        }
        c = e + 1;
    }
}

/// The Pieces tab's key, at the header's right: have / some / missing.
fn piece_legend(p: &egui::Painter, o: Pos2) {
    let (done, partial, missing) = piece_colors();
    let mut x = o.x + 505.0;
    for (c, label) in [
        // i18n: legend: pieces not downloaded yet
        (missing, crate::i18n::mark("Missing")),
        // i18n: legend: pieces partly downloaded
        (partial, crate::i18n::mark("Some")),
        // i18n: legend: pieces downloaded
        (done, crate::i18n::mark("Have")),
    ] {
        let label = crate::i18n::t(label);
        x -= winui::text_width(p, label);
        text(p, x, o.y + 290.0, label, winui::TEXT());
        x -= 10.0;
        p.rect_filled(
            winui::rect(egui::Rect::from_min_size(
                egui::pos2(x, o.y + 282.0),
                egui::vec2(7.0, 7.0),
            )),
            1.0,
            c,
        );
        x -= 14.0;
    }
}

/// Pieces there, partly there (a square standing for several), missing.
fn piece_colors() -> (Color32, Color32, Color32) {
    let c = crate::theme::colors();
    (c.piece, c.piece_partial, c.piece_missing)
}

/// The Pieces tab: one square per piece (or per group of pieces when there are many).
fn piece_grid(p: &egui::Painter, o: Pos2, pieces: &[bool]) {
    if pieces.is_empty() {
        return;
    }
    let (left, top, w, h) = (o.x + 22.0, o.y + 302.0, 498.0, 94.0);
    let cols = (w / 7.0) as usize;
    let rows = (h / 7.0) as usize;
    let cells = (cols * rows).min(pieces.len());
    let (done, partial, missing) = piece_colors();
    for k in 0..cells {
        let a = k * pieces.len() / cells;
        let b = ((k + 1) * pieces.len() / cells)
            .max(a + 1)
            .min(pieces.len());
        let got = pieces[a..b].iter().filter(|x| **x).count();
        let c = if got == b - a {
            done
        } else if got > 0 {
            partial
        } else {
            missing
        };
        let (x, y) = (
            left + (k % cols) as f32 * 7.0,
            top + (k / cols) as f32 * 7.0,
        );
        p.rect_filled(
            winui::rect(egui::Rect::from_min_size(
                egui::pos2(x, y),
                egui::vec2(6.0, 6.0),
            )),
            1.0,
            c,
        );
    }
}

/// The Files tab's table of a torrent's files, ticked or not: the progress window's (`shown`: 6
/// rows) and the Files window's (torrentfiles.rs, taller). The row whose box was clicked.
pub(crate) fn files_table(
    ui: &mut Ui,
    p: &egui::Painter,
    o: Pos2,
    id: &str,
    rows: &[Vec<String>],
    checks: &[bool],
    shown: usize,
) -> Option<usize> {
    table_sized(
        ui,
        p,
        o,
        id,
        &[
            (25.0, crate::i18n::mark("File")),
            (335.0, crate::i18n::mark("Size")),
            (430.0, crate::i18n::mark("Done")),
        ],
        &[330.0, 425.0],
        rows,
        true,
        Some(checks),
        shown,
    )
}

/// The height of a [`table_sized`] of `shown` rows' body (the progress window's 6: 100 px).
pub(crate) fn table_body_height(shown: usize) -> f32 {
    17.0 * shown as f32 - 2.0
}

/// A table in the connections table's frame: headers at x, column lines at `seps`, rows of
/// text cut to their column (whole on hover), scrolled by the wheel. `checks`: a check box
/// before each row's icon, ticked or not (the Files tab); the row whose box was clicked.
#[allow(clippy::too_many_arguments)]
fn table(
    ui: &mut Ui,
    p: &egui::Painter,
    o: Pos2,
    id: &str,
    heads: &[(f32, &str)],
    seps: &[f32],
    rows: &[Vec<String>],
    icons: bool,
    checks: Option<&[bool]>,
) -> Option<usize> {
    table_sized(ui, p, o, id, heads, seps, rows, icons, checks, 6)
}

/// [`table`] showing `shown` rows: it grows down from the same top.
#[allow(clippy::too_many_arguments)]
fn table_sized(
    ui: &mut Ui,
    p: &egui::Painter,
    o: Pos2,
    id: &str,
    heads: &[(f32, &str)],
    seps: &[f32],
    rows: &[Vec<String>],
    icons: bool,
    checks: Option<&[bool]>,
    shown: usize,
) -> Option<usize> {
    let mut clicked = None;
    let body_h = table_body_height(shown);
    let outer = r(o, 17.0, 273.0, 513.0, body_h + 28.0);
    winui::solid(p, outer, winui::FIELD_EDGE());
    let (bg, head, line) = (
        winui::FIELD(),
        winui::LIST_HEADER(),
        crate::theme::colors().header_sep,
    );
    fill(
        p,
        o.x + 18.0,
        o.y + 274.0,
        o.x + 528.0,
        o.y + 299.0 + body_h,
        bg,
    );
    fill(p, o.x + 19.0, o.y + 275.0, o.x + 527.0, o.y + 298.0, head);
    for x in seps {
        fill(p, o.x + x, o.y + 275.0, o.x + x, o.y + 298.0, line);
    }
    for (x, h) in heads {
        text(p, o.x + x, o.y + 290.0, crate::i18n::t(h), winui::TEXT());
    }
    let visible = shown;
    // Per window: each progress window scrolls its own tables.
    let key = egui::Id::unique(("torrent-table", id, ui.ctx().viewport_id()));
    let max_first = rows.len().saturating_sub(visible);
    let mut first = ui
        .ctx()
        .data(|d| d.get_temp::<usize>(key))
        .unwrap_or(0)
        .min(max_first);
    let body = r(o, 19.0, 299.0, 508.0, body_h);
    if ui.rect_contains_pointer(body) {
        let step = winui::wheel_steps(ui, key, body_h / visible as f32, visible as f32);
        first = first.saturating_add_signed(step).min(max_first);
        ui.ctx().data_mut(|d| d.insert_temp(key, first));
    }
    let clip = p.with_clip_rect(body);
    let right_of = |c: usize| seps.get(c).map_or(o.x + 520.0, |x| o.x + x - 4.0);
    for (k, row) in rows.iter().skip(first).take(visible + 1).enumerate() {
        let top = o.y + 299.0 + 17.0 * k as f32;
        for (c, cell) in row.iter().enumerate() {
            let mut x = heads.get(c).map_or(o.x + 25.0, |h| o.x + h.0);
            // A table of files (the Files tab): each name after its type's icon, and its
            // check box first when files can be chosen (#39). Only rows wholly shown take a
            // click (the one cut off under the frame doesn't).
            if icons
                && c == 0
                && let Some(&ticked) = checks.and_then(|t| t.get(first + k))
            {
                let mut on = ticked;
                let at = egui::pos2(x - 4.0, top + 2.0);
                if k < visible && winui::row_check(ui, &clip, at, &mut on, cell, (id, first + k)) {
                    clicked = Some(first + k);
                }
                x += 17.0;
            }
            if icons && c == 0 {
                crate::fileicons::paint_at(
                    ui,
                    &clip,
                    crate::fileicons::Kind::File(cell),
                    egui::pos2(x - 4.0, top),
                    16.0,
                );
                x += crate::fileicons::INDENT - 4.0;
            }
            let (rect, cut) =
                winui::text_fit(&clip, x, top + 12.0, cell, winui::TEXT(), right_of(c));
            // Cells hold info hashes, folders, trackers, peers: copyable (whole on hover).
            winui::copyable(ui, rect.intersect(body), (id, first + k, c), cell, cut);
        }
    }
    // The shared scroll bar (it was a thumb drawn alone, that couldn't be dragged).
    if rows.len() > visible {
        let mut pos = first as f32;
        if crate::scrollbar::bar(
            ui,
            key.with("bar"),
            r(o, 511.0, 275.0, 17.0, body_h + 24.0),
            true,
            rows.len() as f32,
            visible as f32,
            1.0,
            &mut pos,
        ) {
            let first = (pos.round().max(0.0) as usize).min(max_first);
            ui.ctx().data_mut(|d| d.insert_temp(key, first));
        }
    }
    clicked
}

/// How far the check or the merge after the last byte has come (#736), when it is under way
/// and that is known.
fn finishing_fraction(snap: Option<&Snapshot>) -> Option<f64> {
    use mdm_engine::download::State;
    let s = snap.filter(|s| matches!(s.state, State::Verifying | State::Merging))?;
    s.finishing.map(|f| f.clamp(0.0, 1.0))
}

/// The columns a segment fills in the segment bar, from its start column's right to its end's
/// left, `total` bytes across `width` columns: a finished segment reaches the next one's start
/// column, so a finished download leaves no gap (#736). None when it fills none.
fn segment_columns(start: u64, done: f64, total: u64, width: f64) -> Option<(f32, f32)> {
    let col = |v: f64| (v / total as f64 * width).floor();
    let first = col(start as f64);
    let end = col((start as f64 + done).min(total as f64));
    let last = (end - 1.0).min(width - 1.0);
    (last > first).then_some(((first + 1.0) as f32, last as f32))
}

/// The custom-drawn segment bar (spec "Segment bar"): 13-row gradients, red start columns;
/// `segments`: each one's start and bytes done, as drawn ([`Glide`]).
fn segment_bar(p: &egui::Painter, o: Pos2, item: &Item, segments: &[(u64, f64)]) {
    let (x0, y0) = (o.x + 17.0, o.y + 249.0);
    let border = crate::theme::colors().segment_border;
    fill(p, x0, y0, x0 + 512.0, y0, border);
    fill(p, x0, y0 + 14.0, x0 + 512.0, y0 + 14.0, border);
    fill(p, x0, y0, x0, y0 + 14.0, border);
    fill(p, x0 + 512.0, y0, x0 + 512.0, y0 + 14.0, border);
    for (x, y) in [
        (x0, y0),
        (x0 + 512.0, y0),
        (x0, y0 + 14.0),
        (x0 + 512.0, y0 + 14.0),
    ] {
        px(p, x, y, crate::theme::colors().segment_corner);
    }
    // Paint a run of columns [a, b] with one of the three row profiles.
    let run = |a: f32, b: f32, which: usize| {
        if b < a {
            return;
        }
        for (row, cols) in seg_rows().enumerate() {
            let c = match which {
                0 => cols.0,
                1 => cols.1,
                _ => cols.2,
            };
            fill(p, a, y0 + 1.0 + row as f32, b, y0 + 1.0 + row as f32, c);
        }
    };
    let ix = o.x + 18.0;
    run(ix, ix + 510.0, 0);
    let Some(total) = item.size.filter(|t| *t > 0) else {
        return;
    };
    let col = |v: f64| (v / total as f64 * 511.0).floor() as f32;
    for &(start, done) in segments {
        let sx = ix + col(start as f64);
        if let Some((a, b)) = segment_columns(start, done, total, 511.0) {
            run(ix + a, ix + b, 1);
        }
        run(sx, sx, 2);
    }
}

/// Why yt-dlp downloads a video itself rather than MDM's engine (#769), as one short line.
fn helper_line(why: &mdm_tools::streams::Helper) -> String {
    use mdm_tools::streams::Helper;
    match why {
        // i18n: progress window, a video's line: MDM's own downloader can't take a live broadcast, so the helper program yt-dlp records it
        Helper::Live => crate::i18n::t("yt-dlp downloads it itself: a live stream").to_owned(),
        Helper::Pieces(_) => {
            // i18n: progress window, a video's line: a stream sent in many small pieces of a rare kind (Adobe HDS, Smooth Streaming), which the helper program yt-dlp downloads itself
            crate::i18n::t("yt-dlp downloads it itself: a stream in pieces").to_owned()
        }
        Helper::Protocol(protocol) => crate::i18n::fmt(
            // i18n: progress window, a video's line: {protocol} = a streaming protocol's short name (rtmp, rtsp, mms); MDM's own downloader doesn't speak it, so the helper program yt-dlp downloads it
            crate::i18n::t("yt-dlp downloads it itself: {protocol}"),
            &[("protocol", &protocol.to_ascii_uppercase())],
        ),
        Helper::Protected => {
            // i18n: progress window, a video's line: the stream is copy-protected (DRM), left to the helper program yt-dlp, which refuses it
            crate::i18n::t("yt-dlp downloads it itself: protected (DRM)").to_owned()
        }
        // i18n: progress window, a video's line: the helper program yt-dlp gave no address MDM's own downloader could use
        Helper::NoAddress => crate::i18n::t("yt-dlp downloads it itself").to_owned(),
    }
}

/// The connections table (spec "Connections table"): N. · Downloaded · Info.
fn connections(ui: &mut Ui, p: &egui::Painter, o: Pos2, snap: Option<&Snapshot>) {
    let outer = r(o, 17.0, 273.0, 513.0, 128.0);
    winui::solid(p, outer, winui::FIELD_EDGE());
    fill(
        p,
        o.x + 18.0,
        o.y + 274.0,
        o.x + 528.0,
        o.y + 399.0,
        // Every list's background (IDM's light one is mauve, #beb4be: the only one of its kind).
        winui::FIELD(),
    );
    fill(
        p,
        o.x + 19.0,
        o.y + 275.0,
        o.x + 510.0,
        o.y + 298.0,
        winui::LIST_HEADER(),
    );
    for x in [48.0, 148.0, 498.0] {
        fill(
            p,
            o.x + x,
            o.y + 275.0,
            o.x + x,
            o.y + 298.0,
            crate::theme::colors().header_sep,
        );
    }
    // Column titles end in "..." before their column's line, whole on hover; a long "N." starts
    // at the list's edge.
    let head = |x: f32, right: f32, label: &str| {
        let (rc, cut) = winui::text_fit(p, o.x + x, o.y + 290.0, label, winui::TEXT(), o.x + right);
        if cut {
            winui::full_on_hover(ui, rc, ("connections-head", x as i32), label);
        }
    };
    // i18n: column title: connection number (abbreviation of Number)
    let number = crate::i18n::t("N.");
    let x = if winui::text_width(p, number) > 21.0 {
        21.0
    } else {
        25.0
    };
    head(x, 46.0, number);
    // i18n: column title: bytes this connection received
    head(55.0, 146.0, crate::i18n::t("Downloaded"));
    // i18n: column title: what the connection is doing
    head(155.0, 496.0, crate::i18n::t("Info"));
    // Listed by connection number, like IDM (the engine reports them in arrival order).
    let mut conns: Vec<_> = snap
        .map(|s| s.connections.iter().collect())
        .unwrap_or_default();
    conns.sort_by_key(|c| c.id);
    let key = egui::Id::unique(("conn-scroll", ui.ctx().viewport_id()));
    let visible = 6usize;
    let max_first = conns.len().saturating_sub(visible);
    let mut first = ui
        .ctx()
        .data(|d| d.get_temp::<usize>(key))
        .unwrap_or(0)
        .min(max_first);
    let body = r(o, 19.0, 299.0, 492.0, 100.0);
    if ui.rect_contains_pointer(body) {
        let step = winui::wheel_steps(ui, key, 100.0 / visible as f32, visible as f32);
        first = first.saturating_add_signed(step).min(max_first);
        ui.ctx().data_mut(|d| d.insert_temp(key, first));
    }
    let clip = p.with_clip_rect(body);
    for (k, c) in conns.iter().skip(first).take(visible + 1).enumerate() {
        let top = o.y + 299.0 + 17.0 * k as f32;
        winui::text_right(
            &clip,
            o.x + 33.0,
            top + 12.0,
            &(c.id + 1).to_string(),
            winui::TEXT(),
        );
        text(
            &clip,
            o.x + 56.0,
            top + 12.0,
            &size3(c.downloaded),
            winui::TEXT(),
        );
        let info = match &c.status {
            ConnStatus::Connecting => crate::i18n::t("Connecting...").to_owned(),
            ConnStatus::Receiving => crate::i18n::t("Receiving data...").to_owned(),
            ConnStatus::Retrying(e) => crate::i18n::fmt(
                // i18n: a connection's state: it failed and tries again, with the reason
                crate::i18n::t("Retrying: {error}"),
                &[("error", &crate::errors::describe(e))],
            ),
            ConnStatus::Finished => String::new(),
            ConnStatus::Failed(e) => crate::i18n::fmt(
                crate::i18n::t("Error: {error}"),
                &[("error", &crate::errors::describe(e))],
            ),
            ConnStatus::Retired { host, cap } => crate::i18n::fmt(
                // i18n: a connection closed because the server allows only that many
                &crate::i18n::tn(
                    u64::from(*cap),
                    "{host} allows {n} connection",
                    "{host} allows {n} connections",
                ),
                &[("host", &crate::i18n::ltr(host))],
            ),
        };
        // The Info column ends at the scrollbar.
        let (rect, cut) = winui::text_fit(
            &clip,
            o.x + 155.0,
            top + 12.0,
            &info,
            winui::TEXT(),
            o.x + 507.0,
        );
        if cut {
            winui::full_on_hover(ui, rect.intersect(body), ("conn", c.id), &info);
        }
    }
    // The shared scroll bar (a track and thumb were drawn here, the thumb not draggable).
    if conns.len() > visible {
        let mut pos = first as f32;
        if crate::scrollbar::bar(
            ui,
            key.with("bar"),
            r(o, 511.0, 275.0, 17.0, 124.0),
            true,
            conns.len() as f32,
            visible as f32,
            1.0,
            &mut pos,
        ) {
            let first = (pos.round().max(0.0) as usize).min(max_first);
            ui.ctx().data_mut(|d| d.insert_temp(key, first));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A download of unknown size has no percentage in its title (from #384).
    #[test]
    fn the_title_has_a_percentage_only_with_a_size() {
        assert!(!window_title(None, "clip.mp4").contains('%'));
        assert!(window_title(Some(0.445), "clip.mp4").starts_with("44% "));
    }

    #[test]
    fn hide_tab_takes_the_tab_off_the_strip() {
        let mut p = Progress::new(1, true);
        p.tab = Tab::Limiter;
        p.hide(Tab::Limiter);
        assert_eq!(p.tab, Tab::Status, "back on Download status");
        assert_eq!(p.hidden_tabs, [Tab::Limiter], "Speed Limiter off the strip");
        p.hide(Tab::Limiter);
        p.hide(Tab::Status);
        assert_eq!(
            p.hidden_tabs,
            [Tab::Limiter],
            "once each, never Download status"
        );
        p.hide(Tab::Completion);
        assert_eq!(p.hidden_tabs, [Tab::Limiter, Tab::Completion]);
        p.show_tab("limit");
        assert_eq!(p.tab, Tab::Limiter, "asked for, it comes back");
        assert_eq!(p.hidden_tabs, [Tab::Completion], "the other stays hidden");
    }

    #[test]
    fn show_hidden_tabs_brings_them_all_back() {
        let mut p = Progress::new(1, true);
        p.hide(Tab::Limiter);
        p.hide(Tab::Completion);
        p.show_hidden_tabs();
        assert!(p.hidden_tabs.is_empty(), "every tab back on the strip");
        assert_eq!(p.tab, Tab::Status, "the page shown stays as it was");
    }

    #[test]
    fn syncs_saved_rename_autosaves_and_ignores_unconfirmed_typing() {
        let mut item = Item::new(
            7,
            "https://example.com/unknown".parse().unwrap(),
            std::path::PathBuf::from("/tmp/unknown"),
            crate::model::Category::General,
        );
        item.rename_on_complete = Some("ArchivePack".into());

        let mut p = Progress::new(7, true);
        p.sync_rename_from_item(&item);
        assert_eq!(p.rename_text, "ArchivePack");
        assert!(p.rename_show_saved, "saved rename shows Saved");

        let t0 = std::time::Instant::now();
        p.rename_text = "Episode 02".into();
        p.on_rename_text_changed(t0);
        assert!(!p.rename_show_saved, "editing hides Saved until confirmed");
        assert_eq!(
            p.take_confirmed_rename(),
            None,
            "half-typed unconfirmed text before 3s is never promoted"
        );
        assert!(
            p.poll_rename_autosave(t0 + std::time::Duration::from_secs(2))
                .is_some(),
            "still waiting before 3 seconds"
        );
        assert!(
            p.poll_rename_autosave(t0 + std::time::Duration::from_secs(3))
                .is_none(),
            "3 seconds elapsed: auto-saves"
        );
        assert!(p.rename_show_saved, "Saved turns green after 3s idle");
        assert_eq!(p.take_confirmed_rename(), Some(Some("Episode 02".into())));

        p.rename_text = "bad/name".into();
        p.on_rename_text_changed(t0);
        assert_eq!(
            p.poll_rename_autosave(t0 + std::time::Duration::from_secs(5)),
            None
        );
        assert_eq!(p.take_confirmed_rename(), None);
    }

    #[test]
    fn switching_tabs_keeps_window_height_constant() {
        let mut p = Progress::new(1, true);
        assert_eq!(p.status_drop(), RENAME_DROP);
        p.tab = Tab::Limiter;
        assert_eq!(p.status_drop(), RENAME_DROP);
        p.tab = Tab::Completion;
        assert_eq!(p.status_drop(), RENAME_DROP);
    }
}

#[cfg(test)]
mod segment_tests {
    use super::segment_columns;

    /// #736: two finished halves fill every column up to the next one's start, and the last one
    /// to the bar's end: no gap at 100 %.
    #[test]
    fn finished_segments_leave_no_gap() {
        let (total, width) = (1000, 511.0);
        assert_eq!(segment_columns(0, 500.0, total, width), Some((1.0, 254.0)));
        // The second one's start column is 255 (drawn red), its fill runs to the end.
        assert_eq!(
            segment_columns(500, 500.0, total, width),
            Some((256.0, 510.0))
        );
        // Under way: up to where it has come.
        assert_eq!(segment_columns(0, 100.0, total, width), Some((1.0, 50.0)));
        // Nothing yet, or less than a column: no fill.
        assert_eq!(segment_columns(0, 0.0, total, width), None);
        assert_eq!(segment_columns(500, 1.0, total, width), None);
    }
}

#[cfg(test)]
mod glide_tests {
    use super::{GLIDE_HOLD, GLIDE_MAX, Glide, glide_frame};

    /// Figures every quarter second (`delta` bytes more each), read by frames every 40 ms that
    /// don't fall on them, as the app reads them: (time, the figure, what is drawn) a frame.
    fn run(frames: usize, delta: u64) -> Vec<(f64, u64, f64)> {
        let mut g = Glide::default();
        (0..frames)
            .map(|k| {
                let now = k as f64 * 0.04;
                let real = (now / 0.25).floor() as u64 * delta;
                (now, real, g.step(now, real, &[(0, real)], false).done)
            })
            .collect()
    }

    /// The first figures are drawn as they are.
    #[test]
    fn the_first_figures_are_drawn_as_they_are() {
        let mut g = Glide::default();
        let d = g.step(3.0, 700, &[(0, 300), (1000, 400)], false);
        assert_eq!(d.done, 700.0);
        assert_eq!(d.segments, [(0, 300.0), (1000, 400.0)]);
    }

    /// Between figures that keep coming the bar never stops, never goes back and never passes
    /// the figure read; it stays less than two figures behind.
    #[test]
    fn steady_figures_glide_without_a_stop() {
        let frames = run(200, 500_000);
        let mut last = 0.0;
        for (k, &(now, real, drawn)) in frames.iter().enumerate() {
            assert!(drawn <= real as f64 + 1e-6, "past the figure at {now}");
            assert!(drawn >= last - 1e-6, "back at {now}");
            if now > 0.3 {
                assert!(drawn > last, "stopped at frame {k} ({now:.2} s)");
                assert!(drawn >= real as f64 - 2.0 * 500_000.0, "behind at {now}");
            }
            last = drawn;
        }
    }

    /// A figure below what is drawn (the download started over) is drawn at once.
    #[test]
    fn a_figure_that_goes_back_is_drawn_at_once() {
        let mut g = Glide::default();
        g.step(0.0, 0, &[(0, 0)], false);
        g.step(0.25, 1000, &[(0, 1000)], false);
        let d = g.step(0.40, 1000, &[(0, 1000)], false);
        assert!(d.done > 0.0 && d.done < 1000.0);
        let d = g.step(0.41, 10, &[(0, 10)], false);
        assert_eq!(d.done, 10.0);
        assert_eq!(d.segments, [(0, 10.0)]);
    }

    /// A finished download is drawn whole at once, not a glide behind its last figures: its
    /// window shows it whole before it gives way to Download complete; no frames of its own.
    #[test]
    fn a_finished_download_is_drawn_whole_at_once() {
        let mut g = Glide::default();
        g.step(0.0, 0, &[(0, 0)], false);
        g.step(0.25, 999, &[(0, 999)], false);
        let d = g.step(0.30, 999, &[(0, 999)], false);
        assert!(d.done < 999.0, "{}", d.done);
        let d = g.step(0.31, 1000, &[(0, 1000)], true);
        assert_eq!(d.done, 1000.0);
        assert_eq!(d.segments, [(0, 1000.0)]);
        assert_eq!(d.rate, 0.0);
        let d = g.step(0.35, 1000, &[(0, 1000)], true);
        assert_eq!(d.done, 1000.0);
    }

    /// A file's bar counts what has come before it is written (the engine's map moves later):
    /// it glides on its own figure all the same.
    #[test]
    fn the_bar_glides_on_its_own_figure() {
        let mut g = Glide::default();
        g.step(0.0, 0, &[(0, 0)], false);
        let d = g.step(0.25, 1000, &[(0, 0)], false);
        assert!(d.rate > 0.0);
        let d = g.step(0.40, 1000, &[(0, 0)], false);
        assert!(d.done > 0.0 && d.done < 1000.0, "{}", d.done);
        assert_eq!(d.segments, [(0, 0.0)]);
    }

    /// The segment map moves with the bar: together its segments are always what the bar shows,
    /// and a segment that comes (a connection split one) grows from nothing.
    #[test]
    fn the_segment_map_moves_with_the_bar() {
        let mut g = Glide::default();
        let mut now = 0.0;
        let mut segs = vec![(0u64, 0u64), (5_000, 0)];
        for k in 0..60 {
            if k % 6 == 0 {
                segs[0].1 += 300;
                segs[1].1 += 100;
                if k == 30 {
                    segs.push((8_000, 50));
                }
            }
            let done: u64 = segs.iter().map(|s| s.1).sum();
            let d = g.step(now, done, &segs, false);
            let sum: f64 = d.segments.iter().map(|s| s.1).sum();
            assert!(
                (sum - d.done).abs() < 1e-6,
                "map {sum} vs bar {} at {now}",
                d.done
            );
            for ((_, shown), (_, real)) in d.segments.iter().zip(&segs) {
                assert!(*shown <= *real as f64 + 1e-6);
            }
            now += 0.04;
        }
    }

    /// Once the figures stop moving the bar reaches them and the frames stop being asked for.
    #[test]
    fn frames_are_asked_for_only_while_the_figures_move() {
        let mut g = Glide::default();
        assert_eq!(g.step(0.0, 0, &[(0, 0)], false).rate, 0.0);
        let d = g.step(0.25, 100, &[(0, 100)], false);
        assert!((d.rate - 100.0 / (0.25 * 1.25)).abs() < 1e-9, "{}", d.rate);
        let d = g.step(0.25 + GLIDE_MAX, 100, &[(0, 100)], false);
        assert_eq!(d.done, 100.0);
        assert!(d.rate > 0.0, "moved lately");
        let d = g.step(0.25 + GLIDE_HOLD + 0.01, 100, &[(0, 100)], false);
        assert_eq!(d.done, 100.0);
        assert_eq!(d.rate, 0.0, "still: the app's own pace");
    }

    /// The window is drawn as often as its bar moves a pixel, 25 times a second at most (12.5
    /// while the main window is shown); a bar that moves less than a pixel between the app's own
    /// frames asks for none.
    #[test]
    fn frames_come_as_often_as_the_bar_moves_a_pixel() {
        use std::time::Duration;
        let mb = 1024.0 * 1024.0;
        let total = Some(100 * 1024 * 1024);
        // 100 MB at 10 MB/s: 51 pixels a second, at the cap.
        assert_eq!(
            glide_frame(10.0 * mb, total, 1.0, false),
            Some(Duration::from_millis(40))
        );
        assert_eq!(
            glide_frame(10.0 * mb, total, 1.0, true),
            Some(Duration::from_millis(80))
        );
        // At 2 MB/s: 10 pixels a second (15 at 150 %).
        let every = glide_frame(2.0 * mb, total, 1.0, false).unwrap();
        assert!((every.as_secs_f64() - 1.0 / (0.02 * 511.0)).abs() < 1e-3);
        assert_eq!(glide_frame(2.0 * mb, total, 1.0, true), Some(every));
        let every = glide_frame(2.0 * mb, total, 1.5, false).unwrap();
        assert!((every.as_secs_f64() - 1.0 / (0.03 * 511.0)).abs() < 1e-3);
        // At 0.5 MB/s: 2.6 pixels a second, the app's 4 frames do.
        for main in [false, true] {
            assert_eq!(glide_frame(0.5 * mb, total, 1.0, main), None);
            assert_eq!(glide_frame(0.0, total, 1.0, main), None);
            assert_eq!(glide_frame(10.0 * mb, None, 1.0, main), None);
            assert_eq!(glide_frame(10.0 * mb, Some(0), 1.0, main), None);
        }
    }
}
