//! IDM's small dialogs as separate OS windows, laid out to the pixel from
//! notes/ui-spec/dialogs.md: "Enter new address to download", "Download File Info",
//! "Download complete". All coordinates below are client pixels from the spec.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use egui::{Pos2, Ui, ViewportBuilder, ViewportId, vec2};
use mdm_engine::ProbeInfo;
use url::Url;

use crate::model::{self, Category};
use crate::winui::{self, ButtonKind, r};

pub enum Outcome<T> {
    Open,
    Done(T),
    Closed,
}

const MAIN_RECT: &str = "mdm-main-rect";

/// The main window publishes its outer rect each frame so dialogs can centre on it, with the
/// zoom it is measured at (not in the frame the interface size changes: its rect is still
/// measured at the old one then, scale::settled).
pub fn publish_main_rect(ctx: &egui::Context) {
    if let Some(rc) = ctx.input(|i| i.viewport().outer_rect) {
        let zoom = ctx.zoom_factor();
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique(MAIN_RECT), (rc, zoom)));
    }
}

/// The main window's outer rect as last published, in points at the zoom drawn now (#262: the
/// same place on the screen is fewer points at a larger interface size).
fn main_rect(ctx: &egui::Context) -> Option<egui::Rect> {
    let (rc, zoom) = ctx.data(|d| d.get_temp::<(egui::Rect, f32)>(egui::Id::unique(MAIN_RECT)))?;
    let k = zoom / ctx.zoom_factor();
    Some(egui::Rect::from_min_max(
        (rc.min.to_vec2() * k).to_pos2(),
        (rc.max.to_vec2() * k).to_pos2(),
    ))
}

/// Where a dialog opens: centred over the main window (as IDM's are), inside the screen's work
/// area (its title bar first, for one larger than the screen), decided once when the dialog
/// first shows and kept until `forget_place` — so the user can still move it. Placed again when
/// the interface size changes (#262): the place is in points, which that changes.
pub fn place(ctx: &egui::Context, key: egui::Id, client: [f32; 2]) -> Option<Pos2> {
    let zoom = ctx.zoom_factor();
    if let Some((p, z)) = ctx.data(|d| d.get_temp::<(Pos2, f32)>(key))
        && z == zoom
    {
        return Some(p);
    }
    let main = main_rect(ctx)?;
    // A fixed-frame dialog's outer size is client + (6, 29) (spec "Conventions").
    let outer = vec2(client[0] + 6.0, client[1] + 29.0);
    let mut p = (main.center() - outer / 2.0).round();
    if let Some(area) = crate::scale::work_area(ctx, main.center()) {
        let whole = vec2(client[0], client[1]) + crate::scale::frame(ctx);
        p = crate::scale::keep_inside(p, whole, area).round();
    }
    ctx.data_mut(|d| d.insert_temp(key, (p, zoom)));
    Some(p)
}

pub fn forget_place(ctx: &egui::Context, key: egui::Id) {
    ctx.data_mut(|d| d.remove::<(Pos2, f32)>(key));
}

/// A fixed-size window's client of `size` points as it can be shown on the screen around the
/// main window: no larger than the work area at the interface size (#262), [`scroll`]ed where
/// it is smaller.
pub fn shown_size(ctx: &egui::Context, size: [f32; 2]) -> [f32; 2] {
    let at = main_rect(ctx).map_or(Pos2::ZERO, |m| m.center());
    let fit = crate::scale::shown(vec2(size[0], size[1]), crate::scale::room(ctx, at));
    [fit.x, fit.y]
}

/// A window's whole client, `size` points laid out from the origin `body` gets, drawn in what
/// the window has: scrolled when that is smaller ([`shown_size`]; scale::pane). Interface size
/// keys (Ctrl + / Ctrl − / Ctrl 0) pressed in the window are taken here too.
pub fn scroll<R>(
    ui: &mut Ui,
    id: egui::Id,
    size: [f32; 2],
    body: impl FnOnce(&mut Ui, Pos2) -> R,
) -> R {
    crate::scale::keys(ui.ctx());
    crate::scale::pane(ui, id, vec2(size[0], size[1]), body)
}

/// The interface size changed while window `vid` is open (#262): its size, `shown` points, at
/// the new scale. Windows' fixed frames are sized again by their owner from the new scale
/// (`dialog_frame`, `thin_frame`); elsewhere the window manager is asked. True when it changed
/// since the last call (the caller frames it again).
pub fn follow_scale(ctx: &egui::Context, vid: ViewportId, key: egui::Id, shown: [f32; 2]) -> bool {
    let scale = ctx.input_for(vid, |i| i.pixels_per_point);
    let k = key.with("scale-seen");
    let seen = ctx.data(|d| d.get_temp::<f32>(k));
    ctx.data_mut(|d| d.insert_temp(k, scale));
    let changed = seen.is_some_and(|s| s != scale);
    if changed && !cfg!(windows) {
        ctx.send_viewport_cmd_to(
            vid,
            egui::ViewportCommand::InnerSize(vec2(shown[0], shown[1])),
        );
    }
    changed
}

/// Window chrome shared by every dialog and the progress window: the caption's theme, and when
/// the window may be seen. winit gives a new window the *system* theme, so windows are created
/// hidden (`hidden_builder`), get their theme on the first frame and are shown on the next,
/// already right — no dark flash. The theme is re-sent whenever the wish changes. Call every
/// frame with `open` = false once the window is gone.
pub fn keep_theme(
    ctx: &egui::Context,
    vid: ViewportId,
    key: egui::Id,
    title: &str,
    dark: bool,
    open: bool,
) {
    let theme_key = key.with("theme");
    let shown_key = key.with("shown");
    let front_key = key.with("front");
    let caption_key = key.with("caption");
    // Not drawn in the last pass, the window is gone and this is a new one, hidden until it is
    // told otherwise, whatever the last one was told (#440).
    if !open || window_was_gone(ctx, key.with("theme-pass")) {
        ctx.data_mut(|d| {
            d.remove::<bool>(theme_key);
            d.remove::<bool>(shown_key);
            d.remove::<u32>(front_key);
            d.remove::<bool>(caption_key);
        });
    }
    if !open {
        return;
    }
    let sent = ctx.data(|d| d.get_temp::<bool>(theme_key));
    if sent != Some(dark) {
        ctx.send_viewport_cmd_to(
            vid,
            egui::ViewportCommand::SetTheme(crate::theme::system_theme(dark)),
        );
        ctx.data_mut(|d| d.insert_temp(theme_key, dark));
        // The window is revealed on the next frame: make sure there is one.
        ctx.request_repaint();
    }
    // The theme went out in an earlier frame: show the window now.
    let shown = ctx.data(|d| d.get_temp::<bool>(shown_key).unwrap_or(false));
    if sent.is_some() && !shown {
        ctx.send_viewport_cmd_to(vid, egui::ViewportCommand::Visible(true));
        ctx.data_mut(|d| d.insert_temp(shown_key, true));
        // The next frame sees where it went (below).
        ctx.request_repaint();
    }
    // Shown (in an earlier frame), once: in front of the other programs' windows, where Windows
    // didn't give it the foreground (#208: Download File Info behind the browser that asked for
    // it). Not in off-screen test runs, whose windows stay out of the way. Elsewhere the window
    // manager places a new window and decides on its focus; an app has no say there.
    // Looked for in a few frames at most (its title may change meanwhile: a progress window's).
    #[cfg(windows)]
    {
        const TRIES: u32 = 30;
        let tries = ctx.data(|d| d.get_temp::<u32>(front_key).unwrap_or(0));
        if shown && !crate::offscreen() && tries < TRIES {
            let tries = match crate::win32::come_forward(title) {
                Some(lifted) => {
                    if lifted {
                        tracing::info!("window {title:?} shown behind another program's: lifted");
                    }
                    TRIES
                }
                None => {
                    ctx.request_repaint();
                    tries + 1
                }
            };
            ctx.data_mut(|d| d.insert_temp(front_key, tries));
        }
    }
    #[cfg(not(windows))]
    let _ = front_key;
    // Belt and braces, once per window and theme (until the window is found): asked every frame,
    // the window's thread waited on DWM, or on a busy window of this process while looking for
    // its own, in a stalled system (#633, #583).
    #[cfg(windows)]
    if ctx.data(|d| d.get_temp::<bool>(caption_key)) != Some(dark)
        && crate::win32::caption_dark(title, dark)
    {
        ctx.data_mut(|d| d.insert_temp(caption_key, dark));
    }
    #[cfg(not(windows))]
    let _ = (title, caption_key);
}

/// Whether the window `key` is drawn again after a pass that didn't draw it: egui destroys a
/// viewport a pass doesn't draw, so what was kept about its window went with it (a dialog that
/// ended without a closing pass, as Download File Info behind the Create folder question, #440).
/// False while it is drawn pass after pass, whatever the time between them (a minimized main
/// window runs its passes seconds apart). `key` is this caller's alone.
fn window_was_gone(ctx: &egui::Context, key: egui::Id) -> bool {
    let pass = ctx.cumulative_pass_nr();
    let last = ctx.data(|d| d.get_temp::<u64>(key));
    ctx.data_mut(|d| d.insert_temp(key, pass));
    last.is_none_or(|l| pass.saturating_sub(l) > 1)
}

/// The dialogs later downloads wait behind (`Download File Info`, the duplicate question): they
/// must be on the screen, even while the main window is minimized (Windows hides a window's owned
/// windows with it, leaving the downloads waiting behind one nobody can see, #440).
#[cfg(windows)]
const QUEUE_GATES: [&str; 2] = ["file-info", "duplicate"];

/// A window that must be on the screen and isn't (hidden, not minimized): shown again after a
/// moment, however it got that way, directly: winit believes it is still visible and ignores
/// `Visible(true)`. The other platforms have nothing to ask: MDM's windows have no owner there,
/// and the window manager doesn't say whether it shows a window.
#[cfg(windows)]
fn keep_on_screen(ctx: &egui::Context, key: egui::Id, title: &str) {
    const AFTER_SECONDS: f64 = 1.5;
    let since_key = key.with("hidden-since");
    let shown = ctx.data(|d| d.get_temp::<bool>(key.with("shown")).unwrap_or(false));
    if !shown || !crate::win32::window_hidden(title) {
        ctx.data_mut(|d| d.remove::<f64>(since_key));
        return;
    }
    let now = ctx.input(|i| i.time);
    let since = ctx.data(|d| d.get_temp::<f64>(since_key));
    if since.is_some_and(|s| now - s >= AFTER_SECONDS) {
        tracing::warn!("window {title:?} was hidden: shown again");
        crate::win32::show_hidden(title);
        ctx.data_mut(|d| d.remove::<f64>(since_key));
    } else if since.is_none() {
        ctx.data_mut(|d| d.insert_temp(since_key, now));
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}

/// A fixed-size dialog window whose client area is exactly `client` pixels; `body` gets the
/// client origin and paints at absolute spec coordinates.
pub(crate) fn window<T>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    client: [f32; 2],
    minimize: bool,
    bg: egui::Color32,
    body: impl FnMut(&mut Ui, Pos2) -> Outcome<T>,
) -> Outcome<T> {
    dialog_window(ctx, id, title, client, minimize, bg, Kind::Plain, body)
}

/// A question that must be answered (Yes / No, a countdown): a `window` that stays above the
/// app's other windows and takes the focus when it opens, so a click on the dialog that asked
/// it, or on the main window, never hides it behind them (on Windows it is also owned by the
/// main window; macOS and Linux have no owner to keep it in front).
pub(crate) fn question<T>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    client: [f32; 2],
    minimize: bool,
    bg: egui::Color32,
    body: impl FnMut(&mut Ui, Pos2) -> Outcome<T>,
) -> Outcome<T> {
    dialog_window(ctx, id, title, client, minimize, bg, Kind::Question, body)
}

/// News that must not go unseen (Download complete, as IDM shows it): a `window` shown above
/// every other program's while `on_top`, without taking the keyboard focus from the one being
/// typed in. The caller drops `on_top` once it has been clicked (`notice_seen`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn notice<T>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    client: [f32; 2],
    minimize: bool,
    bg: egui::Color32,
    on_top: bool,
    body: impl FnMut(&mut Ui, Pos2) -> Outcome<T>,
) -> Outcome<T> {
    dialog_window(
        ctx,
        id,
        title,
        client,
        minimize,
        bg,
        Kind::Notice { on_top },
        body,
    )
}

/// Whether notice `id` has the keyboard focus (it was clicked, or activated): seen.
pub(crate) fn notice_seen(ctx: &egui::Context, id: &str) -> bool {
    ctx.input_for(ViewportId::from_hash_of(id), |i| i.viewport().focused) == Some(true)
}

/// How a dialog's window behaves towards the others.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    /// Above the app's windows, taking the focus when it opens (`question`).
    Question,
    /// Never takes the focus by itself; above every program's windows while `on_top` (`notice`).
    Notice {
        on_top: bool,
    },
}

#[allow(clippy::too_many_arguments)]
fn dialog_window<T>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    client: [f32; 2],
    minimize: bool,
    bg: egui::Color32,
    kind: Kind,
    mut body: impl FnMut(&mut Ui, Pos2) -> Outcome<T>,
) -> Outcome<T> {
    let modal = kind == Kind::Question;
    let key = egui::Id::unique(("dialog-pos", id));
    // The title in the interface language (Win32 finds the window by it, so it is used as is
    // from here on).
    let title = crate::i18n::t(title);
    // A style theme's title bar is ours (Windows, #45): drawn above the layout, which keeps its
    // size; the frame (win32::thin_frame) gives the caption to the client.
    let drawn = drawn_captions();
    let caption_h = if drawn {
        crate::winui::caption::CAPTION_H
    } else {
        0.0
    };
    let outer = [client[0], client[1] + caption_h];
    // As large as the screen allows at the interface size (#262): the rest scrolls.
    let shown = shown_size(ctx, outer);
    let vid = ViewportId::from_hash_of(id);
    // A new window: nothing kept about the last one applies to it (#440).
    if window_was_gone(ctx, key.with("window-pass")) {
        forget_window(ctx, vid, key);
    }
    // Hidden until `keep_theme` has given it its theme (see there).
    let mut builder = ViewportBuilder::default()
        .with_visible(false)
        .with_title(crate::i18n::os_text(title))
        .with_inner_size(shown)
        // The thin frame trims the sizing borders and keeps the size fixed.
        .with_resizable(drawn)
        .with_minimize_button(minimize)
        .with_maximize_button(false);
    // Every dialog carries the app icon: without one Windows draws its generic placeholder.
    builder = crate::quiet(builder.with_icon(crate::app_icon()));
    if modal {
        builder = builder.with_always_on_top();
    }
    if let Kind::Notice { on_top } = kind {
        // Shown without activating it (winit: SW_SHOWNOACTIVATE), and the level always given,
        // so that dropping `on_top` turns topmost off (a missing level changes nothing).
        builder = builder.with_active(false).with_window_level(if on_top {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        });
    }
    if let Some(p) = place(ctx, key, shown) {
        builder = builder.with_position(p);
    }
    crate::crash::window(id);
    let out = ctx.show_viewport_immediate(vid, builder, |ui, _| {
        // Painted in the pass that closes it too (see close_requested).
        let close = close_requested(ui);
        let out = egui::CentralPanel::no_frame()
            .frame(egui::Frame::new().fill(bg))
            .show(ui, |ui| {
                let origin = ui.max_rect().min;
                // The window's width, laid out from the right in a right-to-left interface (the
                // title bar; the layout under it has its own, scrolled with it).
                let _surface = winui::Surface::enter(origin.x, origin.x + shown[0]);
                // Our title bar's ×: like the window manager's, the window is painted in its
                // closing pass too (#207), then closes.
                let mut caption_close = false;
                if drawn {
                    use crate::winui::caption::CaptionButton;
                    let mut buttons = vec![(CaptionButton::Close, true)];
                    if minimize {
                        buttons.push((CaptionButton::Minimize, true));
                    }
                    let hwnd = ui.ctx().data(|d| d.get_temp::<isize>(key.with("hwnd")));
                    match drawn_title(ui, origin, shown[0], title, &buttons, hwnd) {
                        Some(CaptionButton::Close) => caption_close = true,
                        Some(CaptionButton::Minimize) => crate::dialogs::minimize(ui.ctx(), hwnd),
                        _ => {}
                    }
                }
                // The layout under the title bar, scrolled when the window is smaller.
                let below = egui::Rect::from_min_max(
                    origin + egui::vec2(0.0, caption_h),
                    ui.max_rect().max,
                );
                let mut rest = ui.new_child(egui::UiBuilder::new().max_rect(below));
                let out = scroll(&mut rest, key, client, |ui, o| {
                    let _surface = winui::Surface::enter(o.x, o.x + client[0]);
                    body(ui, o)
                });
                if caption_close { Outcome::Closed } else { out }
            })
            .inner;
        if close { Outcome::Closed } else { out }
    });
    let rescaled = follow_scale(ctx, vid, key, shown);
    // Elsewhere follow_scale asked the window manager itself.
    #[cfg(not(windows))]
    let _ = rescaled;
    // Give the window IDM's modal-dialog frame (3 px borders, 26 px caption) once it exists,
    // and again at a new size (a dialog that grows: the frame is sized from the client).
    #[cfg(windows)]
    {
        let framed = key.with("framed");
        let scale = ctx.input_for(vid, |i| i.pixels_per_point);
        // The size and whose title bar it is: either changing frames it again, as does a new
        // scale (the interface size, #262; another monitor's).
        let want = [shown[0], shown[1] + if drawn { 1e6 } else { 0.0 }];
        if rescaled || ctx.data(|d| d.get_temp::<[f32; 2]>(framed)) != Some(want) {
            if drawn {
                if let Some(h) = crate::win32::thin_frame(title, shown, scale, None) {
                    ctx.data_mut(|d| {
                        d.insert_temp(framed, want);
                        d.insert_temp(key.with("hwnd"), h);
                    });
                }
            } else if crate::win32::dialog_frame(title, shown, !crate::theme::dark_surface(), scale)
            {
                ctx.data_mut(|d| d.insert_temp(framed, want));
            }
        }
    }
    keep_theme(
        ctx,
        ViewportId::from_hash_of(id),
        key,
        title,
        // Every dialog follows the mode (the light dialogs too), its caption the theme's surface.
        crate::theme::dark_surface(),
        matches!(out, Outcome::Open),
    );
    // A question takes the focus once it is shown (not while hidden for its theme), and so does
    // Download File Info outside Windows (#484): asked for from the browser, it opened behind
    // it. Windows lifts it in keep_theme instead.
    let focused = key.with("focus-sent");
    let takes_focus = modal || (cfg!(not(windows)) && id == "file-info");
    if takes_focus && matches!(out, Outcome::Open) && !crate::offscreen() {
        let shown = ctx.data(|d| d.get_temp::<bool>(key.with("shown")).unwrap_or(false));
        if shown && !ctx.data(|d| d.get_temp::<bool>(focused).unwrap_or(false)) {
            ctx.send_viewport_cmd_to(ViewportId::from_hash_of(id), egui::ViewportCommand::Focus);
            ctx.data_mut(|d| d.insert_temp(focused, true));
        }
    }
    #[cfg(windows)]
    if matches!(out, Outcome::Open) && QUEUE_GATES.contains(&id) {
        keep_on_screen(ctx, key, title);
    }
    if !matches!(out, Outcome::Open) {
        forget_window(ctx, vid, key);
    }
    out
}

/// What a dialog's window keeps while it is open, forgotten: the next opening is a new one (its
/// text boxes start afresh).
fn forget_window(ctx: &egui::Context, vid: ViewportId, key: egui::Id) {
    let opened = opening_key_of(vid);
    ctx.data_mut(|d| {
        let n = d.get_temp::<u64>(opened).unwrap_or(0);
        d.insert_temp(opened, n + 1);
        d.remove::<bool>(key.with("focus-sent"));
    });
    forget_place(ctx, key);
    #[cfg(windows)]
    ctx.data_mut(|d| {
        d.remove::<[f32; 2]>(key.with("framed"));
        d.remove::<isize>(key.with("hwnd"));
    });
}

/// Whether the windows' title bars are ours: a style theme's on Windows (#45). Elsewhere the
/// window manager draws them, which takes no style of ours (Wayland places its own buttons).
pub fn drawn_captions() -> bool {
    cfg!(windows) && crate::theme::current().controls.caption != crate::palette::CaptionLook::System
}

/// A title bar we draw (where the window's frame is ours: win32::thin_frame), `width` points
/// wide at `o`, with `buttons` from the window's end; the button pressed. Where its buttons and
/// icon are goes to the window's hit test (win32::set_caption_hits), in client pixels, each
/// frame (`hwnd`: the window, once its frame is applied).
pub fn drawn_title(
    ui: &mut Ui,
    o: Pos2,
    width: f32,
    title: &str,
    buttons: &[(crate::winui::caption::CaptionButton, bool)],
    hwnd: Option<isize>,
) -> Option<crate::winui::caption::CaptionButton> {
    use crate::winui::caption::CAPTION_H;
    let rect = egui::Rect::from_min_size(o, egui::vec2(width, CAPTION_H));
    let bar = crate::winui::caption::title_bar(ui, rect, title, buttons);
    #[cfg(windows)]
    if let Some(hwnd) = hwnd {
        let ppp = ui.ctx().pixels_per_point();
        let px = |r: egui::Rect| {
            let (a, b) = ((r.min - o) * ppp, (r.max - o) * ppp);
            [
                a.x.round() as i32,
                a.y.round() as i32,
                b.x.round() as i32,
                b.y.round() as i32,
            ]
        };
        crate::win32::set_caption_hits(
            hwnd,
            crate::win32::CaptionHits {
                strip_h: (CAPTION_H * ppp).round() as i32,
                icon: px(bar.icon),
                buttons: bar.buttons.iter().map(|r| px(*r)).collect(),
                max_button: None,
            },
        );
    }
    #[cfg(not(windows))]
    let _ = hwnd;
    bar.pressed
}

/// Whether the window manager asked this window to close (its title bar's ×, Alt+F4, a
/// `WM_DELETE_WINDOW`). The window must still be painted in that pass, and the request honoured
/// after it: egui clears a dialog's window to transparent black before each pass, so a pass that
/// paints nothing leaves a black window on screen until the window is destroyed (#207: one to
/// several seconds on Linux).
pub(crate) fn close_requested(ui: &Ui) -> bool {
    ui.ctx().input(|i| i.viewport().close_requested())
}

/// Bring dialog `id` to the front with the keyboard focus once it shows (a window that opens by
/// itself with news, such as a new version found at start, as apps announce theirs). Call every
/// frame while it should; true once done. Off-screen test runs never take the focus.
pub fn focus_when_shown(ctx: &egui::Context, id: &str) -> bool {
    let key = egui::Id::unique(("dialog-pos", id));
    let shown = ctx.data(|d| d.get_temp::<bool>(key.with("shown")).unwrap_or(false));
    if !shown {
        return false;
    }
    if !crate::offscreen() {
        raise(ctx, ViewportId::from_hash_of(id));
    }
    true
}

/// How many times the current window has closed (its text boxes' ids include it).
pub fn opening_key(ctx: &egui::Context) -> egui::Id {
    opening_key_of(ctx.viewport_id())
}

fn opening_key_of(viewport: ViewportId) -> egui::Id {
    egui::Id::unique(("dialog-opening", viewport))
}

/// IDM shows paths under the user profile relative to it: `Downloads\Compressed\a.zip`.
pub fn display_path(p: &Path) -> String {
    match dirs::home_dir().and_then(|h| p.strip_prefix(h).ok().map(Path::to_owned)) {
        Some(rel) => rel.display().to_string(),
        None => p.display().to_string(),
    }
}

/// The inverse of `display_path`: relative paths are under the user profile.
pub fn resolve_path(s: &str) -> PathBuf {
    let p = PathBuf::from(s.trim());
    if p.is_absolute() {
        p
    } else {
        dirs::home_dir().map_or(p.clone(), |h| h.join(p))
    }
}

/// IDM's dialog size text, e.g. `60.00  MB` (two spaces, as measured).
pub fn dialog_size(b: u64) -> String {
    model::size(b).replacen(' ', "  ", 1)
}

// ---- Enter new address to download (client 560 × 93) ---------------------------------------

#[derive(Default)]
pub struct AddUrl {
    pub address: String,
    pub auth: bool,
    pub login: String,
    pub password: String,
    error: Option<String>,
    /// The addresses entered before, newest first (the Address drop list).
    history: Vec<String>,
    list_open: bool,
    /// The address Options → Sites Logins was last looked up for, and the login it gave
    /// (#511): "Use authorization" shows it while the box is left as it was filled.
    looked_up: String,
    filled: Option<(String, String)>,
    /// The clipboard's link, read off the window's thread (#661).
    clip: crate::clipboard::Prefill,
}

pub struct NewUrl {
    pub url: Url,
    pub login: Option<(String, String)>,
}

impl AddUrl {
    /// Opens like IDM's: pre-filled with the clipboard's text when it is an http(s) URL (or a
    /// magnet link: OK then opens Add torrent). The clipboard is read off the window's thread,
    /// and the address put in the box on the frame it comes ([`AddUrl::fill_from_clipboard`]).
    pub fn from_clipboard(history: &[String]) -> Self {
        AddUrl {
            history: history.to_vec(),
            clip: crate::clipboard::Prefill::ask(),
            ..AddUrl::default()
        }
    }

    /// The clipboard's text in the box, when it is an address MDM downloads and the box is
    /// still empty (nothing typed or picked meanwhile).
    fn fill_from_clipboard(&mut self, clip: &str) {
        let clip = clip.trim();
        if !self.address.is_empty() {
            return;
        }
        if let Ok(u) = Url::parse(clip)
            && (mdm_engine::supported(&u) || u.scheme() == "magnet")
        {
            self.address = clip.to_owned();
        }
    }

    /// "Use authorization" shows the Sites Logins entry the download will use for the address
    /// (#511): `site_login` is that same match (`Settings::site_login`). A login typed or
    /// changed in the box is left alone; one filled from Options follows the address.
    fn follow_site_login(&mut self, site_login: impl Fn(&Url) -> Option<(String, String)>) {
        if self.looked_up == self.address {
            return;
        }
        self.looked_up.clone_from(&self.address);
        let found = crate::model::parse_address(&self.address)
            .ok()
            .and_then(|u| site_login(&u));
        if found == self.filled {
            return;
        }
        let as_filled = match &self.filled {
            Some((user, pass)) => self.auth && self.login == *user && self.password == *pass,
            None => !self.auth && self.login.is_empty() && self.password.is_empty(),
        };
        if as_filled {
            self.auth = found.is_some();
            (self.login, self.password) = found.clone().unwrap_or_default();
        }
        self.filled = found;
    }

    /// The login OK hands on: none while the box shows the Sites Logins entry as it was filled,
    /// so the download keeps following Options, as it does without one.
    fn typed_login(&self) -> Option<(String, String)> {
        let own = (self.login.clone(), self.password.clone());
        (self.auth && self.filled.as_ref() != Some(&own)).then_some(own)
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        site_login: impl Fn(&Url) -> Option<(String, String)>,
    ) -> Outcome<NewUrl> {
        if let Some(clip) = self.clip.take() {
            self.fill_from_clipboard(&clip);
        }
        self.follow_site_login(site_login);
        window(
            ctx,
            "add-url",
            "Enter new address to download",
            [560.0, 93.0],
            false,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                // i18n: noun: the web address (URL)
                winui::text(
                    &p,
                    o.x + 12.0,
                    o.y + 25.0,
                    crate::i18n::t("Address"),
                    winui::TEXT(),
                );
                let addr_rc = r(o, 68.0, 11.0, 396.0, 21.0);
                let addr = winui::combo_edit(ui, addr_rc, &mut self.address);
                // An error about the address goes once it is edited.
                if addr.changed() {
                    self.error = None;
                }
                if ui.memory(|m| m.focused().is_none()) {
                    addr.request_focus();
                }
                // The arrow: the addresses entered before.
                if winui::combo_arrow(ui, addr_rc, "add-url", self.list_open).clicked()
                    || winui::combo_keys(ui, &addr)
                {
                    self.list_open = !self.list_open;
                }
                // Up / Down step through the addresses entered before.
                let at = self.history.iter().position(|h| *h == self.address.trim());
                if !self.list_open
                    && let Some(i) = winui::history_keys(ui, &addr, self.history.len(), at)
                {
                    self.address = self.history[i].clone();
                    self.error = None;
                }
                // Left / Right move its caret (#812); while its list is open, Escape closes
                // the list and the box keeps the keys.
                winui::keep_edit_keys(ui, &addr, self.list_open);
                let list_was_open = self.list_open;
                if self.list_open
                    && let Some(i) = winui::drop_list(
                        ui,
                        "add-url-history",
                        addr_rc,
                        &self.history,
                        crate::i18n::t("No addresses yet"),
                        &mut self.list_open,
                    )
                {
                    self.address = self.history[i].clone();
                    addr.request_focus();
                }
                // Enter presses the default button (OK), as in IDM, wherever the focus is.
                let _ = &addr;
                let enter = winui::enter_pressed(ui);

                winui::group_box(
                    &p,
                    r(o, 11.0, 45.0, 453.0, 40.0),
                    Some(winui::caption_gap(
                        &p,
                        o.x + 20.0,
                        o.x + 124.0,
                        "Use authorization",
                        false,
                    )),
                );
                winui::checkbox(
                    ui,
                    o + vec2(20.0, 39.0),
                    &mut self.auth,
                    "Use authorization",
                    true,
                );
                let label = if self.auth {
                    winui::TEXT()
                } else {
                    winui::TEXT_DISABLED()
                };
                // i18n: noun: the user name and password section
                winui::text(&p, o.x + 84.0, o.y + 69.0, crate::i18n::t("Login"), label);
                winui::edit(
                    ui,
                    r(o, 116.0, 55.0, 117.0, 23.0),
                    &mut self.login,
                    self.auth,
                    false,
                );
                winui::text(
                    &p,
                    o.x + 289.0,
                    o.y + 69.0,
                    crate::i18n::t("Password"),
                    label,
                );
                winui::edit(
                    ui,
                    r(o, 342.0, 55.0, 113.0, 23.0),
                    &mut self.password,
                    self.auth,
                    true,
                );

                let ok = winui::button(
                    ui,
                    r(o, 475.0, 12.0, 79.0, 19.0),
                    "OK",
                    ButtonKind::Default,
                    false,
                    None,
                )
                .clicked();
                let cancel = winui::button(
                    ui,
                    r(o, 475.0, 40.0, 79.0, 21.0),
                    "Cancel",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked();
                if let Some(e) = &self.error {
                    let red = winui::ERROR();
                    // Ends before the OK / Cancel buttons; whole on hover when cut.
                    // On the group box's top line, over a strip of the dialog's colour (as a
                    // caption sits on its line), not struck through by it.
                    let w = winui::text_width(&p, e).min(465.0 - 140.0) + 6.0;
                    winui::fill_l(
                        &p,
                        o.x + 137.0,
                        o.y + 39.0,
                        o.x + 137.0 + w,
                        o.y + 52.0,
                        winui::BG(),
                    );
                    winui::fit(
                        ui,
                        &p,
                        o.x + 140.0,
                        o.y + 49.0,
                        e,
                        red,
                        o.x + 465.0,
                        "add-url-error",
                    );
                }
                // Escape closes an open drop list first, then the dialog.
                if cancel || (!list_was_open && winui::escape_pressed(ui)) {
                    return Outcome::Closed;
                }
                if ok || (enter && !list_was_open) {
                    match crate::model::parse_address(&self.address) {
                        Ok(url) if mdm_engine::supported(&url) || url.scheme() == "magnet" => {
                            return Outcome::Done(NewUrl {
                                url,
                                login: self.typed_login(),
                            });
                        }
                        _ => self.error = Some(crate::i18n::t("Invalid address").into()),
                    }
                }
                Outcome::Open
            },
        )
    }
}

// ---- Download File Info (client 570 × 208) -------------------------------------------------

/// What Download File Info asks the disk about Save As (#743).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlaceAsk {
    pub dir: PathBuf,
    pub name: String,
    /// A name MDM gave: numbered on when it is taken there (`naming::destination`). A typed one
    /// only has its folder looked at (its download is numbered as it starts if it is taken).
    pub number: bool,
}

/// The disk's answer: where the download goes, and whether its folder is there.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Found {
    pub dest: PathBuf,
    pub folder: bool,
}

/// How the disk is looked at: [`look`], or a test's slow one.
pub(crate) type Look = Arc<dyn Fn(&PlaceAsk) -> Found + Send + Sync>;

/// The engine's naming rules on the disk as it is now; on a thread of its own (it may wait).
fn look(ask: &PlaceAsk) -> Found {
    Found {
        dest: if ask.number {
            mdm_engine::naming::destination(&ask.dir, &ask.name)
        } else {
            ask.dir.join(&ask.name)
        },
        folder: ask.dir.is_dir(),
    }
}

/// Save As's place, looked at off the window's thread (#743, #751): a sleeping hard disk, a USB
/// drive or a network folder can take seconds to answer whether a name is taken, and the window
/// waited for it (the freeze report caught 8 s). Save As shows the name as given meanwhile
/// (fitted, without " (1)"), and takes the numbered one when the answer comes, unless it was
/// typed over. A download started before the answer is numbered as it starts
/// (`Destination::Free`, `Item::number_at_start`): nothing is overwritten either way.
pub(crate) struct SaveAsPlace {
    look: Look,
    repaint: Option<egui::Context>,
    /// The last question, Save As's text it is for (as the answer set it, once taken), and its
    /// answer to come. A new question asks on a thread of its own: one that hangs on a dead
    /// share holds up nothing else.
    asking: Option<(PlaceAsk, String, crate::launch::Asked<PlaceAsk, Found>)>,
}

impl SaveAsPlace {
    /// Looked at on the disk; `repaint` shows the answer when it comes.
    pub fn new(repaint: Option<&egui::Context>) -> Self {
        Self::with(Arc::new(look), repaint)
    }

    /// Looked at with `look` (a test's).
    pub fn with(look: Look, repaint: Option<&egui::Context>) -> Self {
        SaveAsPlace {
            look,
            repaint: repaint.cloned(),
            asking: None,
        }
    }

    /// `name` in `dir` as MDM names it: Save As's text for now (fitted, not numbered: no wait
    /// for the disk), its free numbered name asked for.
    pub fn name_in(&mut self, dir: &Path, name: &str) -> String {
        let shown = display_path(&mdm_engine::naming::guess(dir, name));
        let ask = PlaceAsk {
            dir: dir.to_owned(),
            name: name.to_owned(),
            number: true,
        };
        self.ask(ask, &shown);
        shown
    }

    fn ask(&mut self, ask: PlaceAsk, shown: &str) {
        if self
            .asking
            .as_ref()
            .is_some_and(|(a, s, _)| *a == ask && s == shown)
        {
            return;
        }
        let asked = crate::launch::Asked::default();
        let look = self.look.clone();
        // Never waits: the answer is taken on a later frame (`follow`).
        let _ = asked.get(
            &ask,
            std::time::Duration::MAX,
            std::time::Duration::ZERO,
            self.repaint.as_ref(),
            move |a| look(a),
        );
        self.asking = Some((ask, shown.to_owned(), asked));
    }

    /// Each frame, never waiting: once the answer has come for what Save As (`save_as`) still
    /// shows, a name MDM gave takes its free numbered name; whether its folder is there. None
    /// while it hasn't come, or when Save As was typed over since.
    pub fn follow(&mut self, save_as: &mut String) -> Option<bool> {
        let (ask, shown, asked) = self.asking.as_mut()?;
        if shown != save_as {
            return None;
        }
        let found = asked.answer(ask)?;
        if ask.number {
            *save_as = display_path(&found.dest);
            shown.clone_from(save_as);
        }
        Some(found.folder)
    }

    /// Whether Save As's folder is there, as looked at for the text it shows (None: not known
    /// yet). A path typed in Save As is asked about here, as it is (its folder only).
    pub fn folder(&mut self, save_as: &mut String) -> Option<bool> {
        if let Some(there) = self.follow(save_as) {
            return Some(there);
        }
        if self.asking.as_ref().is_some_and(|(_, s, _)| s == save_as) {
            return None; // asked: the answer is on its way
        }
        let dest = resolve_path(save_as);
        let (Some(dir), Some(name)) = (dest.parent(), dest.file_name()) else {
            return None;
        };
        let ask = PlaceAsk {
            dir: dir.to_owned(),
            name: name.to_string_lossy().into_owned(),
            number: false,
        };
        self.ask(ask, &save_as.clone());
        self.follow(save_as)
    }
}

pub struct FileInfo {
    pub url: Url,
    pub login: Option<(String, String)>,
    /// Shown relative to the user profile, like IDM (`Downloads\Video\a.mp4`).
    pub save_as: String,
    pub category: Category,
    pub description: String,
    /// "Remember this path for … category"; starts as Options → Save to → "Change folder for
    /// … category on last selected" says.
    pub remember: bool,
    url_text: String,
    /// The URL box holds no address MDM downloads: Start and Later wait.
    url_bad: bool,
    /// Save As has been given the focus (once, when the dialog opens).
    focused: bool,
    /// To look an edited address up again, with the headers and options the download's own
    /// requests carry (app::RequestParts).
    rt: tokio::runtime::Handle,
    request: crate::app::RequestParts,
    probe: Arc<Mutex<Option<Probed>>>,
    info: Option<Result<ProbeInfo, String>>,
    /// The look's open answer, for the download to read instead of asking again (#28).
    kept: Option<mdm_engine::KeptAnswer>,
    edited: bool,
    downloads: PathBuf,
    /// Options → Save to folders per category (label, folder).
    dirs: Vec<(String, String)>,
    category_open: bool,
    /// Browser request context for the download this dialog adds.
    pub context: mdm_ipc::Command,
    /// Start or Later held while the folder is looked at, and asked to be created when missing.
    asking: FolderGate<Start>,
    /// The download already running while the dialog shows (Options → Downloads → "Start
    /// downloading immediately while displaying Download File Info dialog").
    pub early: Option<u64>,
    /// The folders downloads were saved to before, newest first (the Save As drop list).
    pub folders: Vec<String>,
    folders_open: bool,
    /// "+" was clicked: the app opens the new-category window.
    pub new_category: bool,
    /// The downloads waiting their turn behind this one (a browser selection's arrive one by
    /// one): Cancel asks whether they are cancelled too (#394). The app sets it each frame.
    pub waiting: usize,
    /// Cancel was pressed while some waited: the question is up, this dialog waits for it.
    cancel_ask: bool,
    /// The question's answer was Yes: the waiting ones are cancelled with this one (`Closed`
    /// comes with it; the app clears the waiting ones).
    pub cancel_all: bool,
    /// The debrid accounts (#44): which one this address goes through, if any.
    route: crate::debrid::Route,
    /// The account that takes the address ("Real-Debrid (labuser)"), as last looked at: the
    /// "Download through" line shows while there is one.
    through: Option<String>,
    /// "Download through …" unticked: downloaded directly instead (`Item::direct`). Unticked
    /// from the start when the browser had already started the download.
    pub direct: bool,
    /// Whether Save As names a folder, looked at off the window's thread: it is asked every
    /// frame, and a folder on a network share that doesn't answer would freeze the window.
    dest_dir: crate::launch::Asked<PathBuf, bool>,
    /// Save As's name numbered in its folder, and whether the folder is there, looked at off the
    /// window's thread (#743).
    place: SaveAsPlace,
}

pub enum Start {
    Now,
    Later,
}

impl FileInfo {
    /// Opens the dialog and looks the URL up in the background (name, size), like IDM, with
    /// `request`'s headers (the download's own). A download from the browser needs no look-up:
    /// the browser has had the answer, and its name (a link's `download` name, a Unicode
    /// `filename*`) is the one it checked; asking again could get another.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        new: NewUrl,
        context: mdm_ipc::Command,
        request: crate::app::RequestParts,
        route: crate::debrid::Route,
        downloads: PathBuf,
        dirs: Vec<(String, String)>,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
    ) -> Self {
        let direct = crate::debrid::browser_started(&context);
        let through = route.through(&new.url);
        let from_browser = context
            .name
            .as_deref()
            .map(mdm_engine::naming::sanitize)
            .filter(|n| !n.is_empty());
        let name = from_browser
            .clone()
            .unwrap_or_else(|| crate::model::browser_guess(&new.url, &context));
        let category = Category::of(&name);
        let folder = crate::model::category_folder(category, &downloads, &dirs);
        let mut place = SaveAsPlace::new(Some(ctx));
        let save_as = place.name_in(&folder, &name);
        let (probe, info) = match from_browser {
            Some(file_name) => (
                Arc::new(Mutex::new(None)),
                Some(Ok(ProbeInfo {
                    final_url: context
                        .final_url
                        .as_deref()
                        .and_then(|u| Url::parse(u).ok())
                        .unwrap_or_else(|| new.url.clone()),
                    size: context.size,
                    file_name,
                    // The browser had the answer: nothing was asked of the server here.
                    server: None,
                    web_page: false,
                })),
            ),
            None => (start_probe(rt, ctx, &new.url, &request), None),
        };
        FileInfo {
            dest_dir: Default::default(),
            place,
            url_text: new.url.to_string(),
            url_bad: false,
            focused: false,
            rt: rt.clone(),
            request,
            url: new.url,
            login: new.login,
            save_as,
            category,
            description: String::new(),
            remember: false,
            probe,
            info,
            kept: None,
            edited: false,
            downloads,
            dirs,
            category_open: false,
            context,
            asking: FolderGate::default(),
            early: None,
            folders: Vec::new(),
            folders_open: false,
            new_category: false,
            waiting: 0,
            cancel_ask: false,
            cancel_all: false,
            route,
            through,
            direct,
        }
    }

    /// Look the address up again as it will now be downloaded: through the debrid account
    /// or directly (#44), its name and size can differ.
    fn look_again(&mut self, ctx: &egui::Context) {
        self.request.opts.unlocker = self.route.unlocker(&self.url, self.direct);
        self.through = self.route.through(&self.url);
        self.probe = start_probe(&self.rt, ctx, &self.url, &self.request);
        self.info = None;
        // The old way's answer is closed.
        self.kept = None;
    }

    /// The account that takes the address appeared or changed (the services' host lists are
    /// read when MDM starts, and Options may change them while the dialog is open): looked up
    /// again its way. A download the browser had started keeps the browser's answer, and goes
    /// directly unless the line is ticked.
    fn follow_route(&mut self, ctx: &egui::Context) {
        let through = self.route.through(&self.url);
        if through == self.through {
            return;
        }
        if self.direct {
            self.through = through;
        } else {
            self.look_again(ctx);
        }
    }

    /// The URL box changed: a new address MDM downloads is the one to download (looked up
    /// again; its file name too, unless Save As was edited); anything else holds Start and
    /// Later back.
    fn url_edited(&mut self, ctx: &egui::Context) {
        match crate::model::parse_address(&self.url_text) {
            Ok(url) if mdm_engine::supported(&url) => {
                self.url_bad = false;
                if url == self.url {
                    return;
                }
                let from_browser = crate::debrid::browser_started(&self.context);
                if !self.edited {
                    let name = mdm_engine::naming::file_name(None, &url);
                    self.category = Category::of(&name);
                    let dir =
                        crate::model::category_folder(self.category, &self.downloads, &self.dirs);
                    self.save_as = self.place.name_in(&dir, &name);
                }
                // Another address: the browser's name, size and form are not its own, and on
                // another site neither are its cookies and page (never sent there).
                if url.origin() != self.url.origin() {
                    self.context.cookies = None;
                    self.context.referer = None;
                    self.request.headers.remove(reqwest::header::COOKIE);
                    self.request.headers.remove(reqwest::header::REFERER);
                }
                self.context.name = None;
                self.context.size = None;
                self.context.mime = None;
                self.context.final_url = None;
                self.context.post = None;
                self.context.post_type = None;
                self.request.opts.post = None;
                self.request.opts.browser_final = None;
                self.request.headers.remove(reqwest::header::CONTENT_TYPE);
                // Another address isn't the download the browser had started: it goes through
                // the debrid account that takes its host (#44), unless the user unticked it.
                if from_browser {
                    self.direct = false;
                }
                self.url = url;
                self.look_again(ctx);
            }
            _ => self.url_bad = true,
        }
    }

    /// Save As keeps its file name in `folder`.
    fn set_folder(&mut self, folder: &Path) {
        let name = Path::new(&self.save_as)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Fitted to the new folder too (#28): a deeper one leaves a long name less room.
        self.save_as = self.place.name_in(folder, &name);
        self.edited = true;
    }

    /// "Remember this path for … category": the category and the folder to keep for it.
    pub fn remembered(&self) -> Option<(Category, String)> {
        self.remember.then(|| {
            let dir = Path::new(&self.save_as)
                .parent()
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            (self.category, dir)
        })
    }

    /// The look's open answer, for the download of this address to start from (#28); taken
    /// once.
    pub fn take_kept(&mut self) -> Option<mdm_engine::KeptAnswer> {
        self.kept.take()
    }

    /// The address was looked up and answered (name and size known): the moment IDM starts an
    /// early download.
    pub fn probed(&self) -> bool {
        matches!(self.info, Some(Ok(_)))
    }

    /// The name in Save As is only the address's guess: not edited, and no answer (the
    /// browser's, the look-up's) gave one. The server names the file when it starts (#28).
    pub fn name_guessed(&self) -> bool {
        !self.edited && !self.probed()
    }

    pub fn size(&self) -> Option<u64> {
        self.info
            .as_ref()
            .and_then(|i| i.as_ref().ok())
            .and_then(|i| i.size)
    }

    /// The chosen destination as an absolute path.
    pub fn dest(&self) -> PathBuf {
        resolve_path(&self.save_as)
    }

    pub(crate) fn set_category(&mut self, c: Category) {
        self.category = c;
        let name = Path::new(&self.save_as)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let folder = crate::model::category_folder(c, &self.downloads, &self.dirs);
        self.save_as = self.place.name_in(&folder, &name);
    }

    /// Whether Save As's folder is there, as looked at off the window's thread for the name it
    /// shows now (None: not known yet). A name MDM gave has its free numbered name by then.
    pub fn folder_found(&mut self) -> Option<bool> {
        self.place.folder(&mut self.save_as)
    }

    /// The dialog; Start (or Download Later) waits for a missing folder to be created.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        icons: &mut crate::chrome::ShellIcons,
    ) -> Outcome<Start> {
        let folder = self
            .dest()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let out = self.dialog(ctx, icons);
        let out = ensure_folder(ctx, &mut self.asking, out, &folder);
        // #394: more downloads wait their turn behind this one (the browser's selected links):
        // Cancel (the button, Escape, the window's ×) asks whether they are cancelled too.
        if matches!(out, Outcome::Closed) && self.waiting > 0 && !self.cancel_ask {
            self.cancel_ask = true;
        }
        if self.cancel_ask {
            // The question answers for the dialog: a Start pressed meanwhile waits for it, as
            // with the Create folder question.
            return match cancel_waiting(ctx, self.waiting) {
                Outcome::Done(all) => {
                    self.cancel_ask = false;
                    self.cancel_all = all;
                    Outcome::Closed
                }
                // The question's own ×: as No, this one alone is skipped.
                Outcome::Closed => {
                    self.cancel_ask = false;
                    Outcome::Closed
                }
                Outcome::Open => Outcome::Open,
            };
        }
        out
    }

    fn dialog(
        &mut self,
        ctx: &egui::Context,
        icons: &mut crate::chrome::ShellIcons,
    ) -> Outcome<Start> {
        if self.info.is_none()
            && let Some(res) = self.probe.lock().unwrap().take()
        {
            let res = res.map(|(pi, kept)| {
                self.kept = kept;
                pi
            });
            if let (Ok(pi), false) = (&res, self.edited) {
                self.category = Category::of(&pi.file_name);
                let dir = crate::model::category_folder(self.category, &self.downloads, &self.dirs);
                self.save_as = self.place.name_in(&dir, &pi.file_name);
            }
            self.info = Some(res);
        }
        // The name numbered in its folder, once the disk has answered (#743).
        self.place.follow(&mut self.save_as);
        self.follow_route(ctx);
        // The "Download through" line (#44) takes a row of its own while an account takes the
        // address: the window grows by it.
        let dy = if self.through.is_some() { 26.0 } else { 0.0 };
        // Preview has 92 px for its label in this language (#192), from 8 px after the group box
        // (it ends at 462) to the window's edge (562): where it needs more, the window grows by it
        // and the right column (icon, size, Preview) widens, its centre moving by half of it.
        let preview_w = winui::button_fit_width(
            &ctx.layer_painter(egui::LayerId::background()),
            "Preview",
            73.0,
            f32::INFINITY,
        );
        let wide = (preview_w - 92.0).max(0.0);
        let cx = (wide / 2.0).floor();
        window(
            ctx,
            "file-info",
            "Download File Info",
            [570.0 + wide, 234.0 + dy],
            true,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                winui::text_right(
                    &p,
                    o.x + 88.0,
                    o.y + 18.0,
                    crate::i18n::t("URL"),
                    winui::TEXT(),
                );
                let url_rc = r(o, 96.0, 5.0, 366.0, 23.0);
                let url_box = winui::edit(ui, url_rc, &mut self.url_text, true, false);
                // An edited address is the one downloaded (it was ignored before).
                if url_box.changed() {
                    self.url_edited(ui.ctx());
                }
                if self.url_bad {
                    winui::error_frame(&p, url_rc);
                }

                // i18n: noun: label before the list of categories
                winui::text_right(
                    &p,
                    o.x + 88.0,
                    o.y + 46.0,
                    crate::i18n::t("Category"),
                    winui::TEXT(),
                );
                let cat_rect = r(o, 96.0, 36.0, 174.0, 21.0);
                let shown = self.category.shown();
                let cat = winui::combo(ui, cat_rect, &shown, "category");
                if cat.clicked() || winui::combo_keys(ui, &cat) {
                    self.category_open = !self.category_open;
                }
                let category_was_open = self.category_open;
                // "+": a new category (the app's category window).
                if winui::small_button(ui, r(o, 276.0, 35.0, 22.0, 21.0), "+").clicked() {
                    self.new_category = true;
                }

                winui::text_right(
                    &p,
                    o.x + 88.0,
                    o.y + 77.0,
                    crate::i18n::t("Save As"),
                    winui::TEXT(),
                );
                let save_rc = r(o, 96.0, 63.0, 335.0, 21.0);
                let save = winui::combo_edit(ui, save_rc, &mut self.save_as);
                // Opened, the keys are in Save As (its name is what one changes here).
                if !self.focused {
                    save.request_focus();
                    self.focused = true;
                }
                if save.changed() {
                    self.edited = true;
                }
                // The arrow: the folders saved to before.
                if winui::combo_arrow(ui, save_rc, "save-as", self.folders_open).clicked()
                    || winui::combo_keys(ui, &save)
                {
                    self.folders_open = !self.folders_open;
                }
                // Up / Down step through the folders saved to before (the file name stays).
                let dir = Path::new(&self.save_as).parent().map(|d| d.to_path_buf());
                let at = self
                    .folders
                    .iter()
                    .position(|f| dir.as_deref() == Some(resolve_path(f).as_path()));
                if !self.folders_open
                    && let Some(i) = winui::history_keys(ui, &save, self.folders.len(), at)
                {
                    let folder = resolve_path(&self.folders[i]);
                    self.set_folder(&folder);
                    self.edited = true;
                }
                // Left / Right move its caret, not the focus to "..." beside it (#812).
                winui::keep_edit_keys(ui, &save, self.folders_open);
                let folders_were_open = self.folders_open;
                if self.folders_open {
                    let shown: Vec<String> = self
                        .folders
                        .iter()
                        .map(|f| display_path(Path::new(f)))
                        .collect();
                    if let Some(i) = winui::drop_list(
                        ui,
                        "save-as-history",
                        save_rc,
                        &shown,
                        crate::i18n::t("No folders yet"),
                        &mut self.folders_open,
                    ) {
                        let folder = PathBuf::from(&self.folders[i]);
                        self.set_folder(&folder);
                    }
                }
                // "...": choose the folder.
                if winui::small_button(ui, r(o, 439.0, 64.0, 22.0, 21.0), "...").clicked() {
                    let now = self
                        .dest()
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_default();
                    // i18n: title of the folder picker
                    let title = crate::i18n::t("Select a folder").to_owned();
                    winui::start_pick(ui.ctx(), "file-info-folder", move || {
                        let mut pick = crate::winui::file_dialog(&title);
                        if now.is_dir() {
                            pick = pick.set_directory(&now);
                        }
                        pick.pick_folder()
                    });
                }
                if let Some(dir) = winui::picked(ui.ctx(), "file-info-folder") {
                    self.set_folder(&dir);
                }

                let caption = crate::i18n::fmt(
                    crate::i18n::t("Remember this path for \"{category}\" category"),
                    &[("category", &self.category.shown())],
                );
                let gap_end = o.x + 122.0 + winui::text_width(&p, &caption) + 5.0;
                winui::group_box(
                    &p,
                    r(o, 96.0, 97.0, 366.0, 45.0),
                    Some((o.x + 104.0, gap_end)),
                );
                winui::checkbox(
                    ui,
                    o + vec2(104.0, 90.0),
                    &mut self.remember,
                    &caption,
                    true,
                );
                let mut folder = format!(
                    "{}\\",
                    display_path(&crate::model::category_folder(
                        self.category,
                        &self.downloads,
                        &self.dirs
                    ))
                    .trim_end_matches('\\')
                );
                let folder_rc = r(o, 104.0, 112.0, 348.0, 23.0);
                winui::edit(ui, folder_rc, &mut folder, false, false);
                // Greyed as IDM shows it, and still copyable.
                winui::copyable(ui, folder_rc, "remembered-folder", &folder, false);

                // i18n: noun: label before a free-text box describing the file
                winui::text_right(
                    &p,
                    o.x + 87.0,
                    o.y + 161.0,
                    crate::i18n::t("Description"),
                    winui::TEXT(),
                );
                winui::edit(
                    ui,
                    r(o, 96.0, 148.0, 366.0, 23.0),
                    &mut self.description,
                    true,
                    false,
                );

                // Through a debrid account (#44): ticked, the service unlocks the link; unticked,
                // the address is downloaded directly. Either way it is looked up again.
                if let Some(through) = &self.through {
                    let mut on = !self.direct;
                    let caption = crate::i18n::fmt(
                        // i18n: Download File Info: a check box, ticked when the download goes through the user's debrid account; {service} the service and the account, "Real-Debrid (labuser)". Unticked: downloaded directly
                        crate::i18n::t("Download through {service}"),
                        &[("service", through)],
                    );
                    winui::checkbox_until(
                        ui,
                        o + vec2(96.0, 186.0),
                        &mut on,
                        &caption,
                        true,
                        Some(o.x + 462.0),
                    );
                    if on == self.direct {
                        self.direct = !on;
                        self.look_again(ui.ctx());
                    }
                }

                // Where the server is (#140), as the look saw it: "..." while it looks; blank when
                // nothing was asked (the browser had the answer) or the look failed.
                // i18n: label before where a download's server is (its country and round trip)
                winui::text_right(
                    &p,
                    o.x + 88.0,
                    o.y + 193.0 + dy,
                    crate::i18n::t("Server"),
                    winui::TEXT(),
                );
                match &self.info {
                    None => crate::whereis::line(
                        ui,
                        o.x + 96.0,
                        o.y + 193.0 + dy,
                        o.x + 462.0,
                        None,
                        "file-info",
                    ),
                    Some(Ok(pi)) => {
                        if let Some(server) = &pi.server {
                            let seen = crate::whereis::Seen::from(server);
                            crate::whereis::line(
                                ui,
                                o.x + 96.0,
                                o.y + 193.0 + dy,
                                o.x + 462.0,
                                Some(&seen),
                                "file-info",
                            );
                        }
                    }
                    Some(Err(_)) => {}
                }

                // Held back while the address or the file's name can't be downloaded to (an
                // empty Save As, or a folder, made it the profile folder).
                let name_bad = {
                    let typed = self.save_as.trim();
                    let dest = self.dest();
                    // A local disk answers within the wait; a share that doesn't reads as no
                    // folder meanwhile (the download says so if it can't write there).
                    let folder = || {
                        self.dest_dir
                            .get(
                                &dest,
                                std::time::Duration::from_secs(1),
                                std::time::Duration::from_millis(50),
                                Some(ui.ctx()),
                                |d| d.is_dir(),
                            )
                            .unwrap_or(false)
                    };
                    typed.is_empty()
                        || typed.ends_with(['\\', '/'])
                        || dest.file_name().is_none()
                        || folder()
                };
                let blocked = if self.url_bad {
                    Some("Invalid address")
                } else if name_bad {
                    // i18n: why Start Download is greyed in Download File Info
                    Some(crate::i18n::mark("Type the file's name in Save As"))
                } else {
                    None
                };
                let (later_kind, start_kind) = if blocked.is_some() {
                    (ButtonKind::Disabled, ButtonKind::Disabled)
                } else {
                    (ButtonKind::Normal, ButtonKind::Default)
                };
                // The row as wide as its labels in this language need (#192), ending where it
                // ends in English.
                let [later_at, start_at, cancel_at] = winui::button_row(
                    &p,
                    &[
                        ("Download Later", 97.0, 109.0),
                        ("Start Download", 225.0, 109.0),
                        ("Cancel", 352.0, 109.0),
                    ],
                    19.0,
                    11.0,
                    200.0,
                )[..] else {
                    unreachable!("three buttons")
                };
                let later = winui::button(
                    ui,
                    r(o, later_at.0, 206.0 + dy, later_at.1, 21.0),
                    "Download Later",
                    later_kind,
                    false,
                    Some(9),
                );
                let later = winui::why_disabled(later, blocked.is_some(), blocked.unwrap_or(""));
                let start = winui::button(
                    ui,
                    r(o, start_at.0, 206.0 + dy, start_at.1, 21.0),
                    "Start Download",
                    start_kind,
                    true,
                    Some(0),
                );
                let start = winui::why_disabled(start, blocked.is_some(), blocked.unwrap_or(""));
                let cancel = winui::button(
                    ui,
                    r(o, cancel_at.0, 206.0 + dy, cancel_at.1, 21.0),
                    "Cancel",
                    ButtonKind::Normal,
                    false,
                    Some(0),
                );

                // Right column, centred on x ≈ 516 (+ cx): file icon, size, Preview.
                // The shell's own 32 px icon for this file type, as IDM shows.
                let ext = Path::new(&self.save_as)
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                icons.paint(
                    ui,
                    crate::chrome::IconKey::ExtLarge(ext),
                    o + vec2(500.0 + cx, 45.0),
                );
                let (size, reason) = match &self.info {
                    None => ("...".to_owned(), None),
                    Some(Ok(pi)) => (
                        // i18n: a download's size is not known (the server didn't say)
                        pi.size
                            .map_or(crate::i18n::t("unknown").to_owned(), dialog_size),
                        None,
                    ),
                    // i18n: in Download File Info, where the size goes: the address didn't answer
                    Some(Err(e)) => (crate::i18n::t("Error").to_owned(), Some(e.clone())),
                };
                let w = winui::text_width(&p, &size);
                let at = winui::text(
                    &p,
                    o.x + 515.0 + cx - w / 2.0,
                    o.y + 103.0,
                    &size,
                    winui::TEXT(),
                );
                // Why the address didn't answer (not found, sign-in needed…): on hover, copyable.
                if let Some(reason) = reason {
                    winui::copyable(ui, at, "file-info-error", &reason, true);
                }
                // As wide as its label in this language (#192), around the column's centre, from
                // 8 px after the group box (it ends at 462) to the window's edge.
                let (px, pw) =
                    winui::button_around(&p, "Preview", 480.0 + cx, 73.0, 470.0, 562.0 + wide);
                winui::button(
                    ui,
                    r(o, px, 128.0, pw, 21.0),
                    // i18n: verb: open the part already downloaded
                    "Preview",
                    ButtonKind::Disabled,
                    false,
                    None,
                )
                .on_hover_text(crate::i18n::t(
                    // i18n: tooltip of the greyed Preview button in Download File Info
                    "Preview isn't available yet",
                ));

                // The categories in a list window of their own (the shared drop list: keys,
                // Escape, never cut off by this short window).
                if self.category_open {
                    let all = Category::all();
                    let labels: Vec<String> = all.iter().map(|c| c.shown()).collect();
                    if let Some(i) = winui::drop_list(
                        ui,
                        "file-info-category",
                        cat_rect,
                        &labels,
                        "",
                        &mut self.category_open,
                    ) && let Some(c) = all.get(i)
                    {
                        self.set_category(*c);
                    }
                }

                // Escape closes an open drop list first, then the dialog.
                let key =
                    |k| !folders_were_open && !category_was_open && ui.input(|i| i.key_pressed(k));
                if cancel.clicked() || key(egui::Key::Escape) {
                    Outcome::Closed
                } else if blocked.is_some() {
                    Outcome::Open
                } else if start.clicked() || key(egui::Key::Enter) {
                    Outcome::Done(Start::Now)
                } else if later.clicked() {
                    Outcome::Done(Start::Later)
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// Cancel in Download File Info while more downloads wait their turn behind it (#394): are
/// those cancelled too? `Done(true)`: yes; `Done(false)`: this one alone is skipped.
fn cancel_waiting(ctx: &egui::Context, n: usize) -> Outcome<bool> {
    let extra = grow_extra(ctx, "cancel-waiting");
    question(
        ctx,
        "cancel-waiting",
        "Mister Download Manager",
        [374.0, 100.0 + extra],
        false,
        winui::BG(),
        |ui, o| {
            let p = ui.painter().clone();
            let text = crate::i18n::fmt(
                // i18n: Cancel in Download File Info with downloads waiting behind it
                &crate::i18n::tn(
                    n as u64,
                    "There is {n} more download waiting. Cancel it too?",
                    "There are {n} more downloads waiting. Cancel them too?",
                ),
                &[("n", &crate::i18n::integer(n as u64))],
            );
            let lines = winui::paragraph(
                &p,
                o.x + 11.0,
                o.y + 21.0,
                &text,
                winui::TEXT(),
                o.x + 362.0,
                13.0,
            );
            grow_to(ui.ctx(), "cancel-waiting", lines, 2);
            let o = o + vec2(0.0, extra);
            let yes = winui::button(
                ui,
                r(o, 94.0, 63.0, 73.0, 21.0),
                "Yes",
                ButtonKind::Default,
                false,
                None,
            )
            .clicked();
            let no = winui::button(
                ui,
                r(o, 207.0, 63.0, 73.0, 21.0),
                "No",
                ButtonKind::Normal,
                false,
                None,
            )
            .clicked();
            if no || winui::escape_pressed(ui) {
                Outcome::Done(false)
            } else if yes || winui::enter_pressed(ui) {
                Outcome::Done(true)
            } else {
                Outcome::Open
            }
        },
    )
}

/// Look `url` up in the background (name, size) as the download will ask it (`request`), the
/// answer landing in the slot returned.
/// A look's result: what it learned, and its open answer when the download can read it.
type Probed = Result<(ProbeInfo, Option<mdm_engine::KeptAnswer>), String>;

fn start_probe(
    rt: &tokio::runtime::Handle,
    ctx: &egui::Context,
    url: &Url,
    request: &crate::app::RequestParts,
) -> Arc<Mutex<Option<Probed>>> {
    let probe = Arc::new(Mutex::new(None));
    let (slot, url, ctx) = (probe.clone(), url.clone(), ctx.clone());
    let (headers, opts) = (request.headers.clone(), request.opts.clone());
    rt.spawn(async move {
        // In the interface language (the engine's own words are English).
        let res = mdm_engine::probe_kept(url, headers, &opts)
            .await
            .map_err(|e| crate::errors::describe(&e));
        *slot.lock().unwrap() = Some(res);
        ctx.request_repaint();
    });
    probe
}

// ---- Download complete (client 455 × 215) --------------------------------------------------

pub struct Complete {
    pub id: u64,
    pub url: String,
    pub path: PathBuf,
    pub size: u64,
    pub dont_show: bool,
    /// What went wrong besides, in the interface language (a video's subtitles, #68): one line
    /// under the size.
    pub notices: Vec<String>,
    /// Clicked (or activated) once: from then on a window like any other, no longer above every
    /// program's (it shows above them until then, as IDM's does: #77).
    pub seen: bool,
    /// Open was pressed: the file is looked for and handed to its program off the window's
    /// thread (#743); the box closes when it opened, and stays (for Open folder) when it didn't.
    pub opening: Option<crate::launch::Asked<PathBuf, bool>>,
    /// Snooze picked a time (#741): the window closes, and no other opens until then.
    pub snooze: Option<crate::finish::SnoozeFor>,
    /// The file was dragged out of the box to a place MDM couldn't learn (#869): Open and Open
    /// folder say so.
    pub moved_away: bool,
}

impl Complete {
    /// Download complete for this finished download.
    pub fn of(i: &model::Item) -> Self {
        Complete {
            id: i.id,
            // A video's file address, not its page (#795).
            url: i.file_address().to_string(),
            path: i.dest.clone(),
            size: i.size.unwrap_or(i.downloaded),
            dont_show: false,
            notices: i.notices.clone(),
            seen: false,
            opening: None,
            snooze: None,
            moved_away: i.moved_away,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let id = format!("complete-{}", self.id);
        self.seen |= notice_seen(ctx, &id);
        notice(
            ctx,
            &id,
            "Download complete",
            [455.0, 215.0],
            true,
            winui::BG(),
            !self.seen,
            |ui, o| {
                let p = ui.painter().clone();
                // The system's open folder (IDM shows one here): the shared file icons'.
                crate::fileicons::paint_at(
                    ui,
                    &p,
                    crate::fileicons::Kind::FolderOpen,
                    o + vec2(10.0, 11.0),
                    32.0,
                );
                winui::text(
                    &p,
                    o.x + 51.0,
                    o.y + 21.0,
                    crate::i18n::t("Download complete"),
                    winui::TEXT(),
                );
                let line = crate::i18n::fmt(
                    crate::i18n::t("Downloaded {size} ({bytes} Bytes)"),
                    &[
                        ("size", &dialog_size(self.size)),
                        ("bytes", &crate::i18n::integer(self.size)),
                    ],
                );
                winui::text(&p, o.x + 51.0, o.y + 34.0, &line, winui::TEXT());
                // What didn't come with it (a video's subtitles the site refused), shortened to
                // the window and whole on hover.
                if !self.notices.is_empty() {
                    winui::fit(
                        ui,
                        &p,
                        o.x + 51.0,
                        o.y + 52.0,
                        &self.notices.join(" "),
                        winui::ERROR(),
                        o.x + 444.0,
                        ("complete-notice", self.id),
                    );
                }
                // i18n: noun: the web address (URL)
                winui::text(
                    &p,
                    o.x + 11.0,
                    o.y + 72.0,
                    crate::i18n::t("Address"),
                    winui::TEXT(),
                );
                // Read-only and copyable (an editable copy let typing seem to work, then reverted).
                winui::readonly_field(
                    ui,
                    r(o, 11.0, 78.0, 434.0, 23.0),
                    &self.url,
                    winui::BG(),
                    true,
                    "complete-url",
                );
                winui::text(
                    &p,
                    o.x + 11.0,
                    o.y + 117.0,
                    crate::i18n::t("The file saved as"),
                    winui::TEXT(),
                );
                winui::readonly_field(
                    ui,
                    r(o, 11.0, 124.0, 434.0, 23.0),
                    &display_path(&self.path),
                    winui::BG(),
                    true,
                    "complete-saved",
                );

                let btn = |ui: &mut Ui, (x, w): (f32, f32), label: &str, kind| {
                    winui::button(ui, r(o, x, 157.0, w, 21.0), label, kind, false, None)
                };
                // As wide as their labels in this language (#192): a longer one pushes those
                // after it right, Close last at the window's edge.
                let [open_at, with_at, folder_at, close_at] = winui::button_row_from_left(
                    &p,
                    &[
                        ("Open", 12.0, 90.0),
                        ("Open with...", 115.0, 90.0),
                        ("Open folder", 219.0, 90.0),
                        ("Close", 354.0, 90.0),
                    ],
                    13.0,
                    444.0,
                    200.0,
                )[..] else {
                    unreachable!("four buttons")
                };
                // i18n: verb: opens the downloaded file
                let open = btn(ui, open_at, "Open", ButtonKind::Default);
                let with = btn(ui, with_at, "Open with...", ButtonKind::Normal);
                let folder = btn(ui, folder_at, "Open folder", ButtonKind::Normal);
                // i18n: verb: closes this window
                let close = btn(ui, close_at, "Close", ButtonKind::Normal);
                // Windows: the drag handle at the trailing edge, after the snooze.
                let room = if cfg!(windows) { 406.0 } else { 444.0 };
                if let Some(s) = complete_footer(ui, o, 191.0, &mut self.dont_show, room) {
                    self.snooze = Some(s);
                }

                // Drag handle: white box with a 1 px #828790 border; drag it to drop the file into
                // Explorer, a mail or a chat (IDM 5.05). The drop target decides, as for a file
                // dragged in Explorer: moved on the same drive (or with Shift), copied to another
                // (or with Ctrl), and MDM's download follows a move (#869). Windows only: dragging
                // a file out to another app goes through OLE there; Linux and macOS have no such
                // drag built yet (no drag source in their window library), so the handle isn't
                // shown there and nothing is moved: Open folder is the way to the file.
                #[cfg(windows)]
                {
                    let handle = r(o, 414.0, 187.0, 27.0, 20.0);
                    let grip = ui
                        .interact(
                            handle,
                            egui::Id::unique(("drag-out", self.id)),
                            winui::pointer_drag(),
                        )
                        // i18n: tooltip of the handle in Download complete: drag it to drop the downloaded file somewhere
                        .on_hover_text(crate::i18n::t("Drag the file into a folder or an app"));
                    if grip.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    // The file is looked for beside the window, with the drag (#743); where a move
                    // took it comes back to the app (`dragged_out`), however late.
                    if grip.drag_started() {
                        let (id, from, wake) = (self.id, self.path.clone(), ui.ctx().clone());
                        let followed = move |out| {
                            DRAGGED_OUT
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push((id, from, out));
                            wake.request_repaint();
                        };
                        if let Err(e) = crate::win32::drag_file_out(
                            crate::i18n::t("Download complete"),
                            &self.path,
                            followed,
                        ) {
                            tracing::warn!("drag out: {e}");
                        }
                    }
                    p.rect_filled(handle, 0.0, winui::FIELD());
                    p.rect_stroke(
                        handle,
                        0.0,
                        egui::Stroke::new(1.0, winui::FIELD_EDGE()),
                        egui::StrokeKind::Inside,
                    );
                    crate::icons::Icon::DragOut.image(true).paint_at(
                        ui,
                        egui::Rect::from_center_size(handle.center(), vec2(16.0, 16.0)),
                    );
                }

                // Enter: Open, the default button (as in the other dialogs).
                // The keys of a drag out of the grip are the drag's (Escape cancels it), not
                // this window's (#561: the drag runs beside the window, which sees them too).
                #[cfg(windows)]
                let dragging = crate::win32::dragging_out();
                #[cfg(not(windows))]
                let dragging = false; // the grip is Windows-only
                // A file that can't open says why (the box stays for Open folder). Looked for and
                // handed over off the window's thread (#743): the box closes once it opened.
                let opened = self.opening.as_ref().and_then(|a| a.answer(&self.path));
                if opened.is_some() {
                    self.opening = None;
                }
                if (open.clicked() || (!dragging && winui::enter_pressed(ui)))
                    && self.opening.is_none()
                {
                    let opening = crate::launch::Asked::default();
                    let moved_away = self.moved_away;
                    let _ = opening.get(
                        &self.path,
                        std::time::Duration::MAX,
                        std::time::Duration::ZERO,
                        Some(ui.ctx()),
                        move |p| open_download_now(p, moved_away),
                    );
                    self.opening = Some(opening);
                }
                if opened == Some(true) || self.snooze.is_some() {
                    Outcome::Done(())
                } else if with.clicked() {
                    open_download_with(&self.path, self.moved_away);
                    Outcome::Done(())
                } else if folder.clicked() {
                    show_download_in_folder(&self.path, self.moved_away);
                    Outcome::Done(())
                } else if close.clicked() || (!dragging && winui::escape_pressed(ui)) {
                    Outcome::Done(())
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// The last row of Download complete and of a group's summary (#766, #741): "Don't show this
/// dialog again" (the setting), and Snooze after it, a menu of how long no such window shows.
/// `y`: the checkbox's top; `room`: where the row ends (English x). Returns the time picked.
fn complete_footer(
    ui: &mut Ui,
    o: Pos2,
    y: f32,
    dont_show: &mut bool,
    room: f32,
) -> Option<crate::finish::SnoozeFor> {
    let caption = "Don't show this dialog again";
    winui::checkbox(ui, o + vec2(24.0, y), dont_show, caption, true);
    let p = ui.painter().clone();
    // i18n: button in the Download complete window: opens a menu of how long no Download complete window shows (30 minutes, 1 hour...)
    let label = crate::i18n::mark("Snooze");
    let w = winui::menu_button_width(&p, label, 80.0, 160.0);
    // After the checkbox's caption; at the row's end when that is short of room.
    let x = (winui::checkbox_caption_end(&p, 24.0, caption) + 12.0).min(room - w);
    let picked = winui::menu_button(
        ui,
        r(o, x, y - 4.0, w, 21.0),
        label,
        &crate::finish::SnoozeFor::LABELS,
        "complete-snooze",
    );
    picked.map(|i| crate::finish::SnoozeFor::ALL[i])
}

// ---- Downloads complete: a group's summary (client 455 × 169 and up, #766) -----------------

/// One failed download of a group, as its summary lists it.
pub struct FailedOne {
    pub name: String,
    pub error: String,
}

/// What a group's summary asks of the app.
pub enum GroupAction {
    /// Its downloads selected in the list, the main window shown.
    ShowInList,
    Close,
}

/// Downloads added together (a playlist, a batch, Link grabber's links) once all are done: how
/// many finished, how many failed (listed), with Open folder and Show in list, in place of a
/// Download complete window each (#766).
pub struct GroupDone {
    pub group: u64,
    /// Every download it sums up (Show in list selects them).
    pub ids: Vec<u64>,
    pub finished: usize,
    pub failed: Vec<FailedOne>,
    /// Where the finished files are (the first one's folder), and a file there to select in it.
    pub folder: PathBuf,
    pub file: PathBuf,
    /// The finished files' size together.
    pub size: u64,
    pub dont_show: bool,
    pub seen: bool,
    pub snooze: Option<crate::finish::SnoozeFor>,
}

/// How many failed downloads a summary names; "and N more" says the rest.
const FAILED_SHOWN: usize = 3;

impl GroupDone {
    /// The summary of `finished` and `failed` (downloads of `items`).
    pub fn new(group: u64, finished: &[u64], failed: &[u64], items: &[model::Item]) -> Self {
        let find = |id: &u64| items.iter().find(|i| i.id == *id);
        let done: Vec<&model::Item> = finished.iter().filter_map(find).collect();
        let bad: Vec<&model::Item> = failed.iter().filter_map(find).collect();
        let file = done
            .first()
            .or(bad.first())
            .map(|i| i.dest.clone())
            .unwrap_or_default();
        let folder = file.parent().map(Path::to_owned).unwrap_or_default();
        GroupDone {
            group,
            ids: done.iter().chain(bad.iter()).map(|i| i.id).collect(),
            finished: done.len(),
            failed: bad
                .iter()
                .map(|i| FailedOne {
                    name: i.file_name.clone(),
                    error: i.error.clone().unwrap_or_default(),
                })
                .collect(),
            folder,
            file,
            size: done.iter().map(|i| i.size.unwrap_or(i.downloaded)).sum(),
            dont_show: false,
            seen: false,
            snooze: None,
        }
    }

    /// The headline's two halves: "12 downloads finished", and "1 failed" when some did.
    pub fn counts(&self) -> (String, Option<String>) {
        (
            finished_count(self.finished as u64),
            (!self.failed.is_empty()).then(|| {
                // i18n: the summary of downloads added together; after "12 downloads finished": {n} of them failed
                crate::i18n::tn(self.failed.len() as u64, "{n} failed", "{n} failed")
            }),
        )
    }

    /// The failed list's rows: the downloads named, and "and N more" when there are others.
    fn failed_rows(&self) -> usize {
        match self.failed.len() {
            0 => 0,
            n if n <= FAILED_SHOWN => n,
            _ => FAILED_SHOWN + 1,
        }
    }

    /// Its client size: taller by the failed list.
    pub fn client(&self) -> [f32; 2] {
        let rows = self.failed_rows();
        let extra = if rows == 0 {
            0.0
        } else {
            22.0 + 17.0 * rows as f32
        };
        [455.0, 169.0 + extra]
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<GroupAction> {
        let id = format!("group-done-{}", self.group);
        self.seen |= notice_seen(ctx, &id);
        let client = self.client();
        // The buttons' row: under the folder, or under the failed list.
        let buttons_y = client[1] - 58.0;
        notice(
            ctx,
            &id,
            // i18n: title of the window that sums up downloads added together (a playlist, a batch) once all are done
            "Downloads complete",
            client,
            true,
            winui::BG(),
            !self.seen,
            |ui, o| {
                let p = ui.painter().clone();
                crate::fileicons::paint_at(
                    ui,
                    &p,
                    crate::fileicons::Kind::FolderOpen,
                    o + vec2(10.0, 11.0),
                    32.0,
                );
                // "12 downloads finished · 1 failed", the failures in the error colour.
                let (finished, failed) = self.counts();
                let drawn = winui::text(&p, o.x + 51.0, o.y + 21.0, &finished, winui::TEXT());
                if let Some(failed) = failed {
                    let after = 51.0 + drawn.width() + 6.0;
                    let dot = winui::separator_dot(&p, o.x + after, o.y + 21.0, winui::MUTED());
                    winui::text(
                        &p,
                        o.x + after + dot.width() + 5.0,
                        o.y + 21.0,
                        &failed,
                        winui::ERROR(),
                    );
                }
                if self.finished > 0 {
                    let line = crate::i18n::fmt(
                        // i18n: under "12 downloads finished": {size} the finished files' size together, like "1.25 GB"
                        crate::i18n::t("{size} in all"),
                        &[("size", &crate::i18n::ltr(&dialog_size(self.size)))],
                    );
                    winui::text(&p, o.x + 51.0, o.y + 34.0, &line, winui::TEXT());
                }
                winui::text(
                    &p,
                    o.x + 11.0,
                    o.y + 72.0,
                    // i18n: label over the folder the downloads were saved in
                    crate::i18n::t("Saved in"),
                    winui::TEXT(),
                );
                winui::readonly_field(
                    ui,
                    r(o, 11.0, 78.0, 434.0, 23.0),
                    &display_path(&self.folder),
                    winui::BG(),
                    true,
                    "group-done-folder",
                );
                if !self.failed.is_empty() {
                    winui::text(
                        &p,
                        o.x + 11.0,
                        o.y + 120.0,
                        // i18n: label over the list of the downloads of the group that failed
                        crate::i18n::t("Failed:"),
                        winui::TEXT(),
                    );
                    for (k, f) in self.failed.iter().take(FAILED_SHOWN).enumerate() {
                        let line = if f.error.is_empty() {
                            crate::i18n::isolate(&f.name)
                        } else {
                            crate::i18n::fmt(
                                // i18n: one failed download in the summary of a group of downloads: {name} its file name, {error} why it failed
                                crate::i18n::t("{name}: {error}"),
                                &[
                                    ("name", &crate::i18n::isolate(&f.name)),
                                    ("error", &f.error),
                                ],
                            )
                        };
                        winui::fit(
                            ui,
                            &p,
                            o.x + 24.0,
                            o.y + 137.0 + 17.0 * k as f32,
                            &line,
                            winui::ERROR(),
                            o.x + 444.0,
                            ("group-failed", k),
                        );
                    }
                    if self.failed.len() > FAILED_SHOWN {
                        // i18n: under the first failed downloads in the summary of a group of downloads: {n} more failed (Show in list shows them all)
                        let more = crate::i18n::tn(
                            (self.failed.len() - FAILED_SHOWN) as u64,
                            "and {n} more",
                            "and {n} more",
                        );
                        winui::text(
                            &p,
                            o.x + 24.0,
                            o.y + 137.0 + 17.0 * FAILED_SHOWN as f32,
                            &more,
                            winui::MUTED(),
                        );
                    }
                }
                let btn = |ui: &mut Ui, (x, w): (f32, f32), label: &str, kind| {
                    winui::button(ui, r(o, x, buttons_y, w, 21.0), label, kind, false, None)
                };
                // As wide as their labels in this language, Close last at the window's edge.
                let [folder_at, list_at, close_at] = winui::button_row_from_left(
                    &p,
                    &[
                        ("Open folder", 12.0, 90.0),
                        ("Show in list", 115.0, 90.0),
                        ("Close", 354.0, 90.0),
                    ],
                    13.0,
                    444.0,
                    200.0,
                )[..] else {
                    unreachable!("three buttons")
                };
                let folder = btn(ui, folder_at, "Open folder", ButtonKind::Default);
                // i18n: button: shows these downloads in MDM's list, selected
                let list = btn(ui, list_at, "Show in list", ButtonKind::Normal);
                // i18n: verb: closes this window
                let close = btn(ui, close_at, "Close", ButtonKind::Normal);
                let footer = complete_footer(ui, o, buttons_y + 34.0, &mut self.dont_show, 444.0);
                if footer.is_some() {
                    self.snooze = footer;
                }
                if folder.clicked() || winui::enter_pressed(ui) {
                    show_in_folder(&self.file);
                    Outcome::Done(GroupAction::Close)
                } else if list.clicked() {
                    Outcome::Done(GroupAction::ShowInList)
                } else if close.clicked() || self.snooze.is_some() || winui::escape_pressed(ui) {
                    Outcome::Done(GroupAction::Close)
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// "12 downloads finished": a group's summary's headline, and the tray icon's tip while
/// Download complete is snoozed (#766, #741).
pub fn finished_count(n: u64) -> String {
    // i18n: the summary of downloads added together (the Downloads complete window; the tip of the tray icon): {n} of them finished
    crate::i18n::tn(n, "{n} download finished", "{n} downloads finished")
}

// ---- Speed Limiter settings (client 453 × 145, light) --------------------------------------

/// Downloads → Speed Limiter → Settings... (notes/ui-spec/scheduler-and-misc.md §2).
pub struct SpeedSettings {
    pub kb: String,
    pub on_startup: bool,
    /// Beyond IDM (D-012): one limit for all downloads together.
    pub global_on: bool,
    pub global_kb: String,
    pub only_active: bool,
    /// How much taller the note is than IDM's three lines (a longer translation): the rows
    /// below it and the window move down by this much (from the frame before).
    note_extra: f32,
}

/// What Speed Limiter settings → OK sets.
pub struct SpeedChoice {
    pub kb: u64,
    pub on_startup: bool,
    pub global_on: bool,
    pub global_kb: u64,
    pub only_active: bool,
}

impl SpeedSettings {
    pub fn new(s: &crate::settings::Settings) -> Self {
        SpeedSettings {
            kb: s.speed_limit_kb.to_string(),
            on_startup: s.limiter_on_startup,
            global_on: s.global_limit_on,
            global_kb: s.global_limit_kb.to_string(),
            only_active: s.global_only_active,
            note_extra: 0.0,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<SpeedChoice> {
        let extra = self.note_extra;
        window(
            ctx,
            "speed-settings",
            "Speed Limiter settings",
            [453.0, 197.0 + extra],
            false,
            winui::LIGHT_BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let black = winui::LIGHT_TEXT();
                // The label ends where `text_right(265)` ended it: at 266.
                let kb_box = winui::sentence(
                    ui,
                    &p,
                    // i18n: {kb} is an input box (kilobytes per second), drawn where it stands in the translation
                    crate::i18n::t("Maximum download speed for one file {kb} KBytes/sec"),
                    &[winui::SentenceLine {
                        lead: winui::Lead::Right(o.x + 266.0),
                        lead_baseline: o.y + 21.0,
                        boxes: o + vec2(275.0, 7.0),
                        gap: 11.0,
                        baseline: o.y + 21.0,
                    }],
                    &[("kb", vec2(50.0, 23.0))],
                    |p, x, baseline, s| {
                        winui::text(p, x, baseline, s, black);
                    },
                );
                winui::number_field(
                    ui,
                    kb_box[0],
                    &mut self.kb,
                    1,
                    9_999_999,
                    true,
                    winui::LIGHT_BG(),
                );
                let note = crate::i18n::t(
                    "Note: Internet servers may break connection when the speed is too limited! Thus it's not\nrecommended to use Speed Limiter to download from servers that do not support\n\"resume\" feature.",
                );
                // Wrapped inside the window (IDM's English runs to x 440); a translation longer than three
                // lines pushes the rest down and the window grows.
                let lines =
                    winui::paragraph(&p, o.x + 11.0, o.y + 47.0, note, black, o.x + 449.0, 13.0);
                self.note_extra = lines.saturating_sub(3) as f32 * 13.0;
                let o = o + vec2(0.0, self.note_extra);
                winui::checkbox_light(
                    ui,
                    o + vec2(11.0, 86.0),
                    &mut self.on_startup,
                    "Always turn on Speed Limiter on Mister Download Manager startup",
                    true,
                );
                // Beyond IDM: a limit for all downloads together.
                winui::etched_h(&p, o.x + 11.0, o.y + 108.0, 431.0);
                winui::checkbox_light_until(
                    ui,
                    o + vec2(11.0, 121.0),
                    &mut self.global_on,
                    "Limit all downloads together to",
                    true,
                    Some(o.x + 196.0),
                );
                let on = self.global_on;
                winui::number_field(
                    ui,
                    r(o, 196.0, 116.0, 60.0, 23.0),
                    &mut self.global_kb,
                    1,
                    9_999_999,
                    on,
                    winui::LIGHT_BG(),
                );
                let c = if on { black } else { winui::LIGHT_DISABLED() };
                // i18n: unit after an input box: kilobytes per second
                winui::text(
                    &p,
                    o.x + 264.0,
                    o.y + 131.0,
                    crate::i18n::t("KBytes/sec"),
                    c,
                );
                winui::checkbox_light(
                    ui,
                    o + vec2(29.0, 143.0),
                    &mut self.only_active,
                    "Only while I'm using the computer",
                    on,
                );
                let ok = winui::button_light(
                    ui,
                    r(o, 285.0, 164.0, 73.0, 21.0),
                    "OK",
                    ButtonKind::Default,
                    true,
                )
                .clicked();
                let cancel = winui::button_light(
                    ui,
                    r(o, 369.0, 164.0, 73.0, 21.0),
                    "Cancel",
                    ButtonKind::Normal,
                    false,
                )
                .clicked();
                if cancel || winui::escape_pressed(ui) {
                    Outcome::Closed
                } else if ok || winui::enter_pressed(ui) {
                    Outcome::Done(SpeedChoice {
                        kb: winui::number_in(&self.kb, 1, 9_999_999),
                        on_startup: self.on_startup,
                        global_on: self.global_on,
                        global_kb: winui::number_in(&self.global_kb, 1, 9_999_999),
                        only_active: self.only_active,
                    })
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

// ---- Paragraphs that grow their dialog -------------------------------------------------------

/// The height a dialog adds for its paragraph `key` (13 px a line beyond the lines its layout
/// planned), as the paragraph measured it last frame ([`grow_to`]): a long translation or name
/// moves what is under it down and the window grows, instead of running over the controls.
pub fn grow_extra(ctx: &egui::Context, key: &str) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(egui::Id::unique(("grow", key))))
        .unwrap_or(0.0)
}

/// Records that dialog `key` needs `extra` px more than its layout planned ([`grow_extra`]):
/// for what grows by other steps than whole lines (a row of links that wraps).
pub fn grow_px(ctx: &egui::Context, key: &str, extra: f32) {
    let extra = extra.max(0.0).ceil();
    if extra != grow_extra(ctx, key) {
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique(("grow", key)), extra));
        ctx.request_repaint();
    }
}

/// Records that paragraph `key` took `lines` lines where `planned` fit (see [`grow_extra`]); a
/// change draws again at once, at the new size.
pub fn grow_to(ctx: &egui::Context, key: &str, lines: usize, planned: usize) {
    let extra = lines.saturating_sub(planned) as f32 * 13.0;
    if extra != grow_extra(ctx, key) {
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique(("grow", key)), extra));
        ctx.request_repaint();
    }
}

// ---- Message (light) --------------------------------------------------------------------------

/// A notice with OK, sized to its text (crate::i18n::t("Cannot find …")): one line when it
/// fits, wrapped and taller when it doesn't (a long path or reason is never cut off).
pub fn message(ctx: &egui::Context, id: &str, title: &str, text: &str) -> Outcome<()> {
    // Tahoma −11 averages about 5.5 px a character; the box is kept between 260 and 480 px.
    let width = (text.chars().count() as f32 * 5.6 + 40.0).clamp(260.0, 480.0);
    let extra = grow_extra(ctx, id);
    window(
        ctx,
        id,
        title,
        [width, 80.0 + extra],
        false,
        winui::LIGHT_BG(),
        |ui, o| {
            let p = ui.painter().clone();
            let lines = winui::paragraph(
                &p,
                o.x + 16.0,
                o.y + 24.0,
                text,
                winui::LIGHT_TEXT(),
                o.x + width - 16.0,
                13.0,
            );
            grow_to(ui.ctx(), id, lines, 1);
            let ok = winui::button_light(
                ui,
                r(o, (width - 73.0) / 2.0, 47.0 + extra, 73.0, 21.0),
                "OK",
                ButtonKind::Default,
                false,
            )
            .clicked();
            let keys =
                ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape));
            if ok || keys {
                Outcome::Closed
            } else {
                Outcome::Open
            }
        },
    )
}

// ---- Download limit reached (light) ---------------------------------------------------------

/// "Show warning before stopping downloads" (Scheduler → Download limits): says the limit was
/// reached and when downloads go on. IDM's own wording is [unknown] (lab).
pub struct LimitNotice {
    lines: [String; 2],
}

impl LimitNotice {
    pub fn new(mb: u32, hours: u32, wait: std::time::Duration) -> Self {
        let at = crate::clock::now()
            + chrono::Duration::from_std(wait).unwrap_or_else(|_| chrono::Duration::zero());
        LimitNotice {
            lines: [
                crate::i18n::fmt(
                    crate::i18n::t(
                        "The download limit ({mb} MBytes every {hours} hours) has been reached.",
                    ),
                    &[
                        ("mb", &crate::i18n::integer(u64::from(mb))),
                        ("hours", &crate::i18n::integer(u64::from(hours))),
                    ],
                ),
                crate::i18n::fmt(
                    crate::i18n::t("Downloads stopped and will resume automatically at {time}."),
                    &[(
                        "time",
                        &at.format(if crate::i18n::code() == "en" {
                            "%-I:%M %p"
                        } else {
                            "%H:%M"
                        })
                        .to_string(),
                    )],
                ),
            ],
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        window(
            ctx,
            "limit-notice",
            "Download limits",
            [420.0, 90.0],
            false,
            winui::LIGHT_BG(),
            |ui, o| {
                let p = ui.painter().clone();
                for (i, line) in self.lines.iter().enumerate() {
                    winui::fit(
                        ui,
                        &p,
                        o.x + 11.0,
                        o.y + 21.0 + 13.0 * i as f32,
                        line,
                        winui::LIGHT_TEXT(),
                        o.x + 409.0,
                        ("notice-line", i),
                    );
                }
                let ok = winui::button_light(
                    ui,
                    r(o, 173.0, 57.0, 73.0, 21.0),
                    "OK",
                    ButtonKind::Default,
                    false,
                )
                .clicked();
                let keys = ui
                    .input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape));
                if ok || keys {
                    Outcome::Closed
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

// ---- Turn off computer countdown (light) ---------------------------------------------------

/// After a queue with "Turn off computer when done": a 30 s countdown that can be cancelled
/// (or skipped) before the computer shuts down, sleeps or hibernates. IDM's own prompt is
/// [unknown] (lab); turning a computer off unannounced is not acceptable.
pub struct TurnOffCountdown {
    mode: crate::queue::TurnOff,
    force: bool,
    deadline: std::time::Instant,
}

impl TurnOffCountdown {
    pub const SECONDS: u64 = 30;

    pub fn new(mode: crate::queue::TurnOff, force: bool) -> Self {
        TurnOffCountdown {
            mode,
            force,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(Self::SECONDS),
        }
    }

    /// `Done` when the time is up or "Turn off now" was pressed; `Closed` on Cancel.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        use crate::queue::TurnOff;
        let left = self
            .deadline
            .saturating_duration_since(std::time::Instant::now());
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        // Whole sentences: the verb's form and place differ between languages.
        let sentence = match self.mode {
            TurnOff::ShutDown => crate::i18n::t("The computer will shut down in {n} seconds."),
            TurnOff::Sleep => crate::i18n::t("The computer will go to sleep in {n} seconds."),
            TurnOff::Hibernate => crate::i18n::t("The computer will hibernate in {n} seconds."),
        };
        question(
            ctx,
            "turn-off",
            "Mister Download Manager",
            [374.0, 100.0],
            false,
            winui::LIGHT_BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let black = winui::LIGHT_TEXT();
                winui::text(
                    &p,
                    o.x + 11.0,
                    o.y + 21.0,
                    crate::i18n::t("The download queue is done."),
                    black,
                );
                let line = crate::i18n::fmt(
                    sentence,
                    &[(
                        "n",
                        &crate::i18n::integer(left.as_secs() + u64::from(left.subsec_millis() > 0)),
                    )],
                );
                winui::text(&p, o.x + 11.0, o.y + 34.0, &line, black);
                // As wide as their labels in this language (#192): a longer one pushes Cancel
                // right.
                let [now_at, cancel_at] = winui::button_row_from_left(
                    &p,
                    &[("Turn off now", 94.0, 90.0), ("Cancel", 194.0, 73.0)],
                    10.0,
                    363.0,
                    200.0,
                )[..] else {
                    unreachable!("two buttons")
                };
                let now = winui::button_light(
                    ui,
                    r(o, now_at.0, 63.0, now_at.1, 21.0),
                    "Turn off now",
                    ButtonKind::Normal,
                    false,
                )
                .clicked();
                let cancel = winui::button_light(
                    ui,
                    r(o, cancel_at.0, 63.0, cancel_at.1, 21.0),
                    "Cancel",
                    ButtonKind::Default,
                    false,
                )
                .clicked();
                // Enter: Cancel, the default button.
                if cancel || winui::escape_pressed(ui) || winui::enter_pressed(ui) {
                    Outcome::Closed
                } else if now || left.is_zero() {
                    Outcome::Done(())
                } else {
                    Outcome::Open
                }
            },
        )
    }

    /// Shut down, sleep or hibernate now.
    pub fn execute(&self) {
        use crate::queue::TurnOff;
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let mut cmd = std::process::Command::new("shutdown");
            match self.mode {
                TurnOff::ShutDown => cmd.args(["/s", "/t", "0"]),
                TurnOff::Hibernate => cmd.arg("/h"),
                TurnOff::Sleep => {
                    if !crate::win32::sleep(self.force) {
                        // i18n: why the computer could not be put to sleep (fills {error})
                        tell_turn_off_failed(crate::i18n::t("Windows refused").to_owned());
                    }
                    return;
                }
            };
            // "Force processes to terminate": don't wait for programs that hold it up.
            if self.force {
                cmd.arg("/f");
            }
            // No console window flashing up (CREATE_NO_WINDOW).
            cmd.creation_flags(0x0800_0000);
            run_or_tell(cmd);
        }
        // Linux: systemd, as the desktop's own menu does; forcing ignores the programs that ask
        // to delay it (inhibitors).
        #[cfg(target_os = "linux")]
        {
            let verb = match self.mode {
                TurnOff::ShutDown => "poweroff",
                TurnOff::Hibernate => "hibernate",
                TurnOff::Sleep => "suspend",
            };
            let mut cmd = std::process::Command::new("systemctl");
            cmd.arg(verb);
            if self.force {
                cmd.arg("--ignore-inhibitors");
            }
            run_or_tell(cmd);
        }
        // macOS: through System Events, as the Apple menu does (macOS has no separate
        // hibernate: its sleep writes memory to disk as set up); apps can still object.
        #[cfg(target_os = "macos")]
        {
            let script = match self.mode {
                TurnOff::ShutDown => "tell application \"System Events\" to shut down",
                TurnOff::Hibernate | TurnOff::Sleep => {
                    "tell application \"System Events\" to sleep"
                }
            };
            let mut cmd = std::process::Command::new("osascript");
            cmd.args(["-e", script]);
            run_or_tell(cmd);
            let _ = self.force;
        }
    }
}

// ---- Confirm deletion of downloads (client 374 × 120, light) ----------------------------------

/// "Confirm deletion of downloads": Delete / Remove / Delete All Completed. Asks whether the
/// files go too (user, 2026-10-29: IDM's checkbox; for an unfinished download its data so far),
/// for one download or several. `Done((files, dont_show_again))` on Yes.
pub struct ConfirmDelete {
    pub ids: Vec<u64>,
    /// The file name when there is one download.
    name: String,
    /// One download, not complete (IDM's wording).
    unfinished: bool,
    files: bool,
    dont_show: bool,
    /// The files box's answer becomes the next one's default (not after Shift+Delete).
    pub remember: bool,
}

impl ConfirmDelete {
    /// `files`: the checkbox's first state (the last answer, or Shift+Delete).
    pub fn new(ids: Vec<u64>, name: String, unfinished: bool, files: bool) -> Self {
        ConfirmDelete {
            ids,
            name,
            unfinished,
            files,
            dont_show: false,
            remember: true,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<(bool, bool)> {
        let n = self.ids.len() as u64;
        let extra = grow_extra(ctx, "confirm-delete");
        question(
            ctx,
            "confirm-delete",
            "Confirm deletion of downloads",
            [374.0, 146.0 + extra],
            false,
            winui::LIGHT_BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let black = winui::LIGHT_TEXT();
                let text = if n == 1 && self.unfinished {
                    crate::i18n::t(
                        "The selected download is not complete. Are you sure you want to delete
it from Mister Download Manager list of downloads?",
                    )
                    .to_owned()
                } else if n == 1 {
                    // The whole name (two similar names must stay apart): the box grows.
                    let name = self.name.clone();
                    crate::i18n::fmt(
                        crate::i18n::t(
                            "Are you sure you want to delete \"{name}\" from Mister Download Manager list of downloads?",
                        ),
                        // A file name keeps its own direction (an Arabic one reads right to left).
                        &[("name", &crate::i18n::isolate(&name))],
                    )
                } else {
                    // Two or more (one download has its own sentences above); the singular form
                    // is for languages whose "one" form also counts 21, 31…
                    crate::i18n::fmt(
                        // i18n: never shown for a single download
                        &crate::i18n::tn(
                            n,
                            "Are you sure you want to delete {n} download from Mister Download Manager list of downloads?",
                            "Are you sure you want to delete these {n} downloads from Mister Download Manager list of downloads?",
                        ),
                        &[("n", &crate::i18n::integer(n))],
                    )
                };
                let lines =
                    winui::paragraph(&p, o.x + 11.0, o.y + 21.0, &text, black, o.x + 362.0, 13.0);
                grow_to(ui.ctx(), "confirm-delete", lines, 3);
                let o = o + vec2(0.0, extra);
                let files = if n == 1 {
                    crate::i18n::t("Also delete the file from disk").to_owned()
                } else {
                    crate::i18n::fmt(
                        // i18n: never shown for a single file
                        &crate::i18n::tn(
                            n,
                            "Also delete the {n} file from disk",
                            "Also delete the {n} files from disk",
                        ),
                        &[("n", &crate::i18n::integer(n))],
                    )
                };
                winui::checkbox_light(ui, o + vec2(24.0, 60.0), &mut self.files, &files, true);
                let yes = winui::button_light(
                    ui,
                    r(o, 94.0, 86.0, 73.0, 21.0),
                    "Yes",
                    ButtonKind::Default,
                    false,
                )
                .clicked();
                let no = winui::button_light(
                    ui,
                    r(o, 207.0, 86.0, 73.0, 21.0),
                    "No",
                    ButtonKind::Normal,
                    false,
                )
                .clicked();
                winui::checkbox_light(
                    ui,
                    o + vec2(24.0, 120.0),
                    &mut self.dont_show,
                    "Don't show this dialog again",
                    true,
                );
                if no || winui::escape_pressed(ui) {
                    Outcome::Closed
                } else if yes || winui::enter_pressed(ui) {
                    Outcome::Done((self.files, self.dont_show))
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// Minimize a window as its title bar's Minimize asks (`hwnd`: the window, once known). On
/// Windows by the system's own command, as the system's title bar sends it, so a window whose
/// title bar is ours animates down to the taskbar like any other (#874).
pub fn minimize(ctx: &egui::Context, hwnd: Option<isize>) {
    #[cfg(windows)]
    if let Some(h) = hwnd {
        crate::win32::minimize(h);
        return;
    }
    let _ = hwnd;
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
}

/// Select the file in Explorer (Finder, the Linux file manager). A file that is gone: its
/// folder, if that is still there.
/// Bring an open window to the front (and back from the taskbar): asked for again, it came
/// to nothing while it sat behind the main window.
pub fn raise(ctx: &egui::Context, viewport: ViewportId) {
    ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Minimized(false));
    // Off-screen test runs never take the focus (main.rs `offscreen`), as the other paths here.
    if !crate::offscreen() {
        ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
    }
}

/// A dialog's window ([`window`], [`question`]) by its id.
pub fn dialog_viewport(id: &str) -> ViewportId {
    ViewportId::from_hash_of(id)
}

/// Run a shut down / sleep command and say when it fails (hibernation off, a program
/// holding it up): it did nothing, silently.
fn run_or_tell(mut cmd: std::process::Command) {
    std::thread::spawn(move || match cmd.output() {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            let why = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            tell_turn_off_failed(if why.is_empty() {
                out.status.to_string()
            } else {
                why
            });
        }
        Err(e) => tell_turn_off_failed(e.to_string()),
    });
}

fn tell_turn_off_failed(why: String) {
    tell(
        // i18n: title of the message when shutting down / sleep / hibernate failed
        crate::i18n::mark("Turn off computer"),
        crate::i18n::fmt(
            crate::i18n::t("The computer could not be turned off: {error}"),
            &[("error", &why)],
        ),
    );
}

/// What became of a file dragged out of Download complete (#869), when the drop target moved
/// it (a copy or a link leaves it where it was: nothing to say).
#[derive(Clone, Debug, PartialEq, Eq)]
// Made by Windows' drag only (the grip is Windows-only, see Complete::show).
#[cfg_attr(not(windows), allow(dead_code))]
pub enum DraggedOut {
    /// Moved there (the shell said where).
    Moved(PathBuf),
    /// Gone from its place, to somewhere MDM couldn't learn (a program that took it, the
    /// Recycle Bin).
    MovedAway,
}

/// Files dragged out and moved: (download, the path it was dragged from, what became of it).
static DRAGGED_OUT: Mutex<Vec<(u64, PathBuf, DraggedOut)>> = Mutex::new(Vec::new());

/// What [`DRAGGED_OUT`] was given since the last call (the app updates its downloads).
pub fn dragged_out() -> Vec<(u64, PathBuf, DraggedOut)> {
    std::mem::take(&mut *DRAGGED_OUT.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Something that failed, to tell the user (the app shows it in its notice): from any window,
/// without a handle on the app.
static TOLD: std::sync::Mutex<Vec<(&'static str, String)>> = std::sync::Mutex::new(Vec::new());

/// Tell the user `text` under the window title `title` (English, marked; the text translated).
pub fn tell(title: &'static str, text: String) {
    TOLD.lock().unwrap().push((title, text));
}

/// What [`tell`] was given since the last call (the app shows it).
pub fn told() -> Vec<(&'static str, String)> {
    std::mem::take(&mut *TOLD.lock().unwrap())
}

/// Open a downloaded file with its program, or say why not (moved or deleted since, no
/// program for it): on a thread of its own, as the file may be on a drive slow to answer
/// (#743). Returns at once.
pub fn open_file(path: &Path) {
    open_download(path, false);
}

/// [`open_file`] for a download: `moved_away`, its file was dragged out of Download complete to
/// a place MDM couldn't learn (#869), which is said when it isn't there.
pub fn open_download(path: &Path, moved_away: bool) {
    let path = path.to_owned();
    crate::launch::spawn("open file", move || {
        open_download_now(&path, moved_away);
    });
}

/// Open a file with the system default application. On Windows, uses Win32 ShellExecuteW
/// to respect user associations (such as 7-Zip for .zip archives) without explorer.exe failures (#811).
fn open_file_platform(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(windows)]
    {
        crate::win32::open_file_shell(path)
    }
    #[cfg(not(windows))]
    {
        open::that_detached(path)
    }
}

/// [`open_download`], waited for: true when the file was handed to the system. For a thread
/// other than the window's (Download complete asks it through `launch::Asked`: whether it
/// failed decides whether the box stays open).
pub fn open_download_now(path: &Path, moved_away: bool) -> bool {
    hand_over(path, moved_away, |p| {
        open_file_platform(p).map_err(|e| e.to_string())
    })
}

/// Why a download's file isn't where MDM saved it, when it was dragged out of Download complete
/// to a place MDM couldn't learn (#869).
fn moved_away_text(path: &Path) -> String {
    let name = crate::i18n::isolate(&path.file_name().unwrap_or_default().to_string_lossy());
    crate::i18n::fmt(
        crate::i18n::t(
            "{file} was moved elsewhere by dragging it out of Download complete, and MDM couldn't learn where. Look for it where you dropped it.",
        ),
        &[("file", &name)],
    )
}

/// Hand the downloaded file `path` to `hand` (which opens it, or shows a chooser for it), or say
/// why not: moved or deleted since (`moved_away`: dragged out to a place MDM couldn't learn,
/// #869), or `hand`'s error. True when it was handed over. Waits for `hand`, so it runs on a
/// thread other than the window's.
fn hand_over(
    path: &Path,
    moved_away: bool,
    hand: impl FnOnce(&Path) -> Result<(), String>,
) -> bool {
    let name = crate::i18n::isolate(&path.file_name().unwrap_or_default().to_string_lossy());
    let failed = if !path.exists() && moved_away {
        moved_away_text(path)
    } else if !path.exists() {
        crate::i18n::fmt(
            crate::i18n::t(
                "{file} is no longer where it was saved (moved or deleted). Redownload, in its right-click menu, gets it again.",
            ),
            &[("file", &name)],
        )
    } else if let Err(e) = hand(path) {
        crate::i18n::fmt(
            crate::i18n::t("{file} could not be opened: {error}"),
            &[("file", &name), ("error", &e)],
        )
    } else {
        return true;
    };
    // i18n: title of the message when a downloaded file can't be opened
    tell(crate::i18n::mark("Open"), failed);
    false
}

/// Show the file selected in the file manager (its folder, when the file is gone). Returns at
/// once: whether the file and its folder are there is looked at on a thread of its own, as a
/// sleeping hard disk, a USB drive or a network folder can take seconds to answer (#751: 8 s,
/// on the window's thread, froze it).
pub fn show_in_folder(path: &Path) {
    show_download_in_folder(path, false);
}

/// [`show_in_folder`] for a download: `moved_away`, its file was dragged out of Download
/// complete to a place MDM couldn't learn (#869): when it isn't there, that is said instead of
/// opening the folder it left.
pub fn show_download_in_folder(path: &Path, moved_away: bool) {
    let path = path.to_owned();
    crate::launch::spawn("show in folder", move || {
        if moved_away && !path.exists() {
            // i18n: title of the message when the folder of a downloaded file can't be shown
            tell(crate::i18n::mark("Open folder"), moved_away_text(&path));
            return;
        }
        show_in_folder_now(&path)
    });
}

/// [`show_in_folder`], on the thread it runs on.
fn show_in_folder_now(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if !path.exists() {
            if let Some(dir) = path.parent().filter(|d| d.is_dir()) {
                crate::launch::open(dir);
            }
            return;
        }
        // Another file manager set up to open folders (#446): Explorer's `/select` would start
        // Explorer anyway. The folder goes to the system, which runs that one; it can't be told
        // which file to select in a way every file manager reads.
        if let Some(opener) = crate::win32::folder_opener() {
            tracing::info!("show in folder: folders open with {opener}");
            if let Some(dir) = path.parent().filter(|d| d.is_dir()) {
                crate::launch::open(dir);
                return;
            }
        }
        // Explorer reads `/select,"C:\a b\c.mp4"` — quoted after the comma. Rust's own quoting
        // would wrap the whole argument ("/select,C:\a b\c.mp4"), which Explorer doesn't
        // understand: it opened its default folder for any path with a space (user report
        // 2026-10-29, a user folder with a space in its name). Windows paths can't contain a
        // quote.
        let _ = std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn();
    }
    #[cfg(unix)]
    crate::unix::show_in_folder(path);
}

/// "Open with": the system's chooser of programs for the downloaded file `path`, or a message
/// saying why not (moved or deleted since). Returns at once: the chooser is shown, and waited
/// for, on a thread of its own, as is the look at the file (#743, #751).
/// `moved_away`: see [`open_download`].
pub fn open_download_with(path: &Path, moved_away: bool) {
    let path = path.to_owned();
    crate::launch::spawn("open with", move || {
        hand_over(&path, moved_away, |p| {
            // Windows' own "How do you want to open this file?", as Explorer shows it (#720).
            #[cfg(windows)]
            {
                crate::win32::open_with_dialog(p).map_err(|hr| {
                    tracing::warn!("open with: SHOpenWithDialog failed, {hr:#010x}");
                    std::io::Error::from_raw_os_error(hr).to_string()
                })
            }
            // Linux and macOS have no chooser MDM can call for a file: the desktop's opener
            // (xdg-open, which goes through the desktop portal in a sandbox, or macOS's `open`)
            // opens it with the program set for its type, which the user changes there.
            #[cfg(not(windows))]
            {
                open::that_detached(p).map_err(|e| e.to_string())
            }
        });
    });
}

/// "Duplicate download link": the address is already in the list (Options → Downloads → "If a
/// duplicate download link is added: Show a dialog and ask what to do"). `Done` with the
/// choice, numbered as Options lists them (1 numbered name, 2 overwrite, 3 open or resume).
pub struct Duplicate {
    name: String,
    choice: usize,
}

impl Duplicate {
    pub fn new(name: String) -> Self {
        Duplicate { name, choice: 1 }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<usize> {
        question(
            ctx,
            "duplicate",
            "Duplicate download link",
            [460.0, 156.0],
            false,
            winui::LIGHT_BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let ink = winui::LIGHT_TEXT();
                winui::text(
                    &p,
                    o.x + 11.0,
                    o.y + 21.0,
                    crate::i18n::t("This address is already in the list of downloads:"),
                    ink,
                );
                let name = crate::i18n::isolate(&self.name);
                let (rect, cut) =
                    winui::text_fit(&p, o.x + 11.0, o.y + 39.0, &name, ink, o.x + 449.0);
                if cut {
                    winui::full_on_hover(ui, rect, "duplicate-name", &self.name);
                }
                let mut group = Vec::new();
                for (k, label) in crate::options::DUPLICATE.iter().enumerate().skip(1) {
                    let y = 52.0 + 20.0 * (k - 1) as f32;
                    let resp =
                        winui::radio_light(ui, o + vec2(14.0, y), self.choice == k, label, true);
                    if resp.clicked() {
                        self.choice = k;
                    }
                    group.push(resp);
                }
                if let Some(j) = winui::radio_keys(ui, &group) {
                    self.choice = j + 1;
                }
                let ok = winui::button_light(
                    ui,
                    r(o, 295.0, 124.0, 73.0, 21.0),
                    "OK",
                    ButtonKind::Default,
                    false,
                )
                .clicked();
                let cancel = winui::button_light(
                    ui,
                    r(o, 376.0, 124.0, 73.0, 21.0),
                    "Cancel",
                    ButtonKind::Normal,
                    false,
                )
                .clicked();
                if cancel || winui::escape_pressed(ui) {
                    Outcome::Closed
                } else if ok || winui::enter_pressed(ui) {
                    Outcome::Done(self.choice)
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// The queue selection panel (Options → Downloads → "Show queue selection panel on pressing
/// Download Later" / "on closing batch download dialogs"): which queue the downloads join, and
/// whether it starts now. `Done((queue, start))`; Cancel leaves them in the list, in no queue.
pub struct QueuePick {
    names: Vec<String>,
    chosen: usize,
    start: bool,
}

impl QueuePick {
    pub fn new(names: Vec<String>) -> Self {
        QueuePick {
            names,
            chosen: 0,
            start: false,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<(usize, bool)> {
        window(
            ctx,
            "queue-pick",
            "Download later",
            [340.0, 110.0],
            false,
            winui::LIGHT_BG(),
            |ui, o| {
                let p = ui.painter().clone();
                winui::text_right(
                    &p,
                    o.x + 92.0,
                    o.y + 26.0,
                    crate::i18n::t("Add to queue:"),
                    winui::LIGHT_TEXT(),
                );
                let names: Vec<&str> = self.names.iter().map(String::as_str).collect();
                if let Some(i) = winui::combo_light(
                    ui,
                    r(o, 100.0, 12.0, 228.0, 21.0),
                    &names,
                    self.chosen,
                    true,
                    "queue-pick",
                ) {
                    self.chosen = i;
                }
                winui::checkbox_light(
                    ui,
                    o + vec2(100.0, 44.0),
                    &mut self.start,
                    "Start the queue now",
                    true,
                );
                let ok = winui::button_light(
                    ui,
                    r(o, 174.0, 78.0, 73.0, 21.0),
                    "OK",
                    ButtonKind::Default,
                    false,
                )
                .clicked();
                let cancel = winui::button_light(
                    ui,
                    r(o, 255.0, 78.0, 73.0, 21.0),
                    "Cancel",
                    ButtonKind::Normal,
                    false,
                )
                .clicked();
                if cancel || winui::escape_pressed(ui) {
                    Outcome::Closed
                } else if ok || winui::enter_pressed(ui) {
                    Outcome::Done((self.chosen, self.start))
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// A question with Yes / No (deleting a queue…). `Done` on Yes.
pub struct Confirm {
    title: &'static str,
    text: String,
}

impl Confirm {
    pub fn new(title: &'static str, text: String) -> Self {
        Confirm { title, text }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let text = self.text.clone();
        let key = format!("confirm-{}", self.title);
        let extra = grow_extra(ctx, &key);
        question(
            ctx,
            &key,
            self.title,
            [400.0, 96.0 + extra],
            false,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let lines = winui::paragraph(
                    &p,
                    o.x + 11.0,
                    o.y + 22.0,
                    &text,
                    winui::TEXT(),
                    o.x + 389.0,
                    13.0,
                );
                grow_to(ui.ctx(), &key, lines, 3);
                let o = o + vec2(0.0, extra);
                let yes = winui::button(
                    ui,
                    r(o, 234.0, 64.0, 73.0, 21.0),
                    "Yes",
                    ButtonKind::Default,
                    false,
                    None,
                )
                .clicked();
                let no = winui::button(
                    ui,
                    r(o, 316.0, 64.0, 73.0, 21.0),
                    "No",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked();
                if no || winui::escape_pressed(ui) {
                    Outcome::Closed
                } else if yes || winui::enter_pressed(ui) {
                    Outcome::Done(())
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// Delete category (#93): what happens to its downloads is asked each time. Moving them to
/// General is the default; or they leave the list (a running one can't, so it moves). Files on
/// disk stay either way.
pub struct DeleteCategory {
    pub category: crate::model::Category,
    /// Remove its downloads from the list (else: move them to General).
    remove: bool,
}

impl DeleteCategory {
    pub fn new(category: crate::model::Category) -> Self {
        DeleteCategory {
            category,
            remove: false,
        }
    }

    /// Opened with the second choice made (`/show category-remove:<name>`, the checks').
    pub fn removing(mut self) -> Self {
        self.remove = true;
        self
    }

    /// Done(remove): Delete was pressed; `remove` is the second choice.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<bool> {
        let key = "delete-category";
        let extra = grow_extra(ctx, key);
        let text = crate::i18n::fmt(
            crate::i18n::t("Delete the category \"{category}\"? Its files stay where they are."),
            &[("category", &crate::i18n::isolate(&self.category.shown()))],
        );
        question(
            ctx,
            key,
            "Delete category",
            [500.0, 150.0 + extra],
            false,
            winui::LIGHT_BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let lines = winui::paragraph(
                    &p,
                    o.x + 11.0,
                    o.y + 22.0,
                    &text,
                    winui::LIGHT_TEXT(),
                    o.x + 489.0,
                    13.0,
                );
                grow_to(ui.ctx(), key, lines, 2);
                let o = o + vec2(0.0, extra);
                // i18n: choice in Delete category: what happens to the category's downloads
                let to_general = crate::i18n::mark("Move its downloads to General");
                let off_list = crate::i18n::mark(
                    // i18n: choice in Delete category: its downloads leave the list (not their files; running ones move to General)
                    "Remove its downloads from the list (running ones move to General)",
                );
                if winui::radio_light(ui, o + vec2(14.0, 52.0), !self.remove, to_general, true)
                    .clicked()
                {
                    self.remove = false;
                }
                if winui::radio_light(ui, o + vec2(14.0, 75.0), self.remove, off_list, true)
                    .clicked()
                {
                    self.remove = true;
                }
                // As wide as their labels in this language (#192).
                let [delete_at, cancel_at] = winui::buttons_to_right(
                    &p,
                    488.0,
                    8.0,
                    &[("Delete", 73.0), ("Cancel", 73.0)],
                    140.0,
                )[..] else {
                    unreachable!("two buttons")
                };
                // i18n: button: deletes the category
                let delete = winui::button_light(
                    ui,
                    r(o, delete_at.0, 118.0, delete_at.1, 21.0),
                    "Delete",
                    ButtonKind::Default,
                    false,
                )
                .clicked();
                let cancel = winui::button_light(
                    ui,
                    r(o, cancel_at.0, 118.0, cancel_at.1, 21.0),
                    "Cancel",
                    ButtonKind::Normal,
                    false,
                )
                .clicked();
                if cancel || winui::escape_pressed(ui) {
                    Outcome::Closed
                } else if delete || winui::enter_pressed(ui) {
                    Outcome::Done(self.remove)
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// "This folder does not exist … Do you want to create it?" — asked when a download is about to
/// be added to a folder that isn't there (Windows' question, Yes / No). Yes creates it.
pub struct CreateFolder {
    folder: PathBuf,
    error: Option<String>,
    /// Yes was pressed: the folder is made off the window's thread (#743), its answer to come.
    making: Option<crate::launch::Asked<PathBuf, Result<(), String>>>,
}

impl CreateFolder {
    pub fn new(folder: PathBuf) -> Self {
        CreateFolder {
            folder,
            error: None,
            making: None,
        }
    }

    /// `Done` once the folder exists (created on Yes); `Closed` on No, Escape or the close box.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let folder = self.folder.display().to_string();
        question(
            ctx,
            "create-folder",
            "Create folder",
            [420.0, 128.0],
            false,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let ink = winui::TEXT();
                winui::text(
                    &p,
                    o.x + 11.0,
                    o.y + 21.0,
                    crate::i18n::t("This folder does not exist:"),
                    ink,
                );
                let (rect, cut) =
                    winui::text_fit(&p, o.x + 11.0, o.y + 39.0, &folder, ink, o.x + 409.0);
                if cut {
                    winui::full_on_hover(ui, rect, "create-folder-path", &folder);
                }
                winui::text(
                    &p,
                    o.x + 11.0,
                    o.y + 57.0,
                    crate::i18n::t("Do you want to create it?"),
                    ink,
                );
                if let Some(e) = &self.error {
                    let (rect, cut) =
                        winui::text_fit(&p, o.x + 11.0, o.y + 75.0, e, winui::ERROR(), o.x + 409.0);
                    if cut {
                        winui::full_on_hover(ui, rect, "create-folder-error", e);
                    }
                }
                let yes = winui::button(
                    ui,
                    r(o, 254.0, 94.0, 73.0, 21.0),
                    "Yes",
                    ButtonKind::Default,
                    false,
                    None,
                )
                .clicked();
                let no = winui::button(
                    ui,
                    r(o, 336.0, 94.0, 73.0, 21.0),
                    "No",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked();
                // Made beside the window (#743): a drive slow to answer doesn't hold it.
                let made = self.making.as_ref().and_then(|m| m.answer(&self.folder));
                if made.is_some() {
                    self.making = None;
                }
                if let Some(Err(e)) = &made {
                    self.error = Some(crate::i18n::fmt(
                        crate::i18n::t("The folder could not be created: {error}"),
                        &[("error", e)],
                    ));
                }
                if no || winui::escape_pressed(ui) {
                    Outcome::Closed
                } else if made == Some(Ok(())) {
                    Outcome::Done(())
                } else {
                    if (yes || winui::enter_pressed(ui)) && self.making.is_none() {
                        let making = crate::launch::Asked::default();
                        let _ = making.get(
                            &self.folder,
                            std::time::Duration::MAX,
                            std::time::Duration::ZERO,
                            Some(ui.ctx()),
                            |f| std::fs::create_dir_all(f).map_err(|e| e.to_string()),
                        );
                        self.making = Some(making);
                    }
                    Outcome::Open
                }
            },
        )
    }
}

/// "Some of the torrent's files are already in this folder … Use them?" — asked before a torrent
/// is added where files of its names exist (#40: librqbit writes into a file that is there, so
/// an unrelated file of the same name would be overwritten). Yes keeps them for the torrent:
/// what matches it is kept, the rest replaced (how data from elsewhere is resumed or shared).
pub struct ExistingFiles {
    /// The first file found (its full path), and how many in all.
    first: String,
    count: usize,
}

impl ExistingFiles {
    pub fn new(found: &[PathBuf]) -> Self {
        ExistingFiles {
            first: found
                .first()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            count: found.len(),
        }
    }

    /// `Done` on Yes; `Closed` on No, Escape or the close box.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let first = crate::i18n::ltr(&self.first);
        // On its own line under the path, so a long path never cuts it off.
        let more = (self.count > 1).then(|| {
            // i18n: "files already there" question, under the first path: {n} = how many more
            crate::i18n::tn(
                (self.count - 1) as u64,
                "and {n} other file",
                "and {n} other files",
            )
        });
        question(
            ctx,
            "existing-files",
            "Files already there",
            [460.0, 206.0],
            false,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let ink = winui::TEXT();
                let (left, right) = (o.x + 11.0, o.x + 449.0);
                let lines = winui::paragraph(
                    &p,
                    left,
                    o.y + 21.0,
                    crate::i18n::t("Some of the torrent's files are already in this folder:"),
                    ink,
                    right,
                    18.0,
                );
                let path_y = o.y + 21.0 + lines as f32 * 18.0;
                crate::fileicons::paint_at(
                    ui,
                    &p,
                    crate::fileicons::Kind::File(&self.first),
                    egui::pos2(left, path_y - 12.0),
                    16.0,
                );
                let (rect, cut) = winui::text_fit(
                    &p,
                    left + crate::fileicons::INDENT,
                    path_y,
                    &first,
                    ink,
                    right,
                );
                if cut {
                    winui::full_on_hover(ui, rect, "existing-files-path", &first);
                }
                let mut ask_y = path_y + 24.0;
                if let Some(more) = &more {
                    winui::text(&p, left, path_y + 18.0, more, winui::MUTED());
                    ask_y += 18.0;
                }
                // The two sentences wrap (translations run longer), above the buttons.
                let asked = winui::paragraph(
                    &p,
                    left,
                    ask_y,
                    crate::i18n::t(
                        "Use them for this torrent? What matches it is kept, the rest replaced.",
                    ),
                    ink,
                    right,
                    18.0,
                );
                winui::paragraph(
                    &p,
                    left,
                    ask_y + asked as f32 * 18.0,
                    crate::i18n::t("No goes back, to choose another folder."),
                    winui::MUTED(),
                    right,
                    18.0,
                );
                let yes = winui::button(
                    ui,
                    r(o, 294.0, 172.0, 73.0, 21.0),
                    "Yes",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked();
                // No is the default: a file the user didn't mean to give up stays as it is.
                let no = winui::button(
                    ui,
                    r(o, 376.0, 172.0, 73.0, 21.0),
                    "No",
                    ButtonKind::Default,
                    false,
                    None,
                )
                .clicked();
                if yes {
                    Outcome::Done(())
                } else if no || winui::escape_pressed(ui) || winui::enter_pressed(ui) {
                    Outcome::Closed
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// A dialog's "start" (`T`) held while what it would start into is looked at on the disk, off
/// the window's thread (#743: a sleeping hard disk, a USB drive or a network folder can take
/// seconds to answer, and the window waited for it; the dialog stays as it is meanwhile), then
/// while a question about it (`Q`) is up. Lives in the dialog, across frames.
pub struct Gate<T, A, Q> {
    held: Option<T>,
    /// Where it was looked at, and the answer (`A`) to come.
    look: Option<(PathBuf, crate::launch::Asked<PathBuf, A>)>,
    question: Option<Q>,
}

/// [`ensure_folder`]'s: whether the folder is there; Create folder.
pub type FolderGate<T> = Gate<T, bool, CreateFolder>;

/// [`ensure_files_free`]'s: the torrent's files found there; Existing files.
pub type FilesGate<T> = Gate<T, Vec<PathBuf>, ExistingFiles>;

impl<T, A, Q> Default for Gate<T, A, Q> {
    fn default() -> Self {
        Gate {
            held: None,
            look: None,
            question: None,
        }
    }
}

impl<T, A: Clone + PartialEq + Send + 'static, Q> Gate<T, A, Q> {
    /// The dialog's "start", let go: the gate starts over.
    fn release(&mut self) -> Outcome<T> {
        let held = self.held.take();
        *self = Gate::default();
        held.map_or(Outcome::Open, Outcome::Done)
    }

    /// `out` held until `at` has been looked at with `look` (made only when asked; it runs on
    /// a thread of its own) and the question `ask` makes of the answer (None: nothing to ask)
    /// was answered Yes by `show`. No goes back to the dialog.
    fn pass<L: FnOnce(&PathBuf) -> A + Send + 'static>(
        &mut self,
        ctx: &egui::Context,
        out: Outcome<T>,
        at: &Path,
        look: impl FnOnce() -> L,
        ask: impl FnOnce(A) -> Option<Q>,
        show: impl FnOnce(&mut Q, &egui::Context) -> Outcome<()>,
    ) -> Outcome<T> {
        match out {
            Outcome::Closed => {
                *self = Gate::default();
                return Outcome::Closed;
            }
            Outcome::Done(v) if self.held.is_none() => self.held = Some(v),
            // While it is looked at, or the question is up, the dialog's own "start" waits.
            _ => {}
        }
        if self.held.is_none() {
            return Outcome::Open;
        }
        if self.question.is_none() {
            // Asked once for each place (the dialog's folder may change meanwhile); never waited.
            if self.look.as_ref().is_none_or(|(p, _)| p != at) {
                let asked = crate::launch::Asked::default();
                let _ = asked.get(
                    &at.to_owned(),
                    std::time::Duration::MAX,
                    std::time::Duration::ZERO,
                    Some(ctx),
                    look(),
                );
                self.look = Some((at.to_owned(), asked));
            }
            let found = self.look.as_ref().and_then(|(p, a)| a.answer(p));
            match found.map(ask) {
                None => return Outcome::Open,
                Some(None) => return self.release(),
                Some(question) => self.question = question,
            }
        }
        let Some(question) = self.question.as_mut() else {
            return Outcome::Open;
        };
        match show(question, ctx) {
            Outcome::Open => Outcome::Open,
            Outcome::Closed => {
                *self = Gate::default();
                Outcome::Open
            }
            Outcome::Done(()) => self.release(),
        }
    }
}

/// Holds a dialog's "start" (`out`) while files the torrent would write into already exist in
/// `folder` (looked for when the dialog says start, off the window's thread, by what `found`
/// makes): asks [`ExistingFiles`] and keeps the result meanwhile; No goes back to the dialog.
/// `gate` lives in the dialog, across frames.
pub fn ensure_files_free<T, F: FnOnce() -> Vec<PathBuf> + Send + 'static>(
    ctx: &egui::Context,
    gate: &mut FilesGate<T>,
    out: Outcome<T>,
    folder: &Path,
    found: impl FnOnce() -> F,
) -> Outcome<T> {
    gate.pass(
        ctx,
        out,
        folder,
        || {
            let found = found();
            move |_: &PathBuf| found()
        },
        |found| (!found.is_empty()).then(|| ExistingFiles::new(&found)),
        ExistingFiles::show,
    )
}

/// Holds a dialog's "start" (`out`) until `folder` exists: looked at off the window's thread,
/// and when it is missing, asks to create it ([`CreateFolder`]); No goes back to the dialog.
/// `gate` lives in the dialog, across frames.
pub fn ensure_folder<T>(
    ctx: &egui::Context,
    gate: &mut FolderGate<T>,
    out: Outcome<T>,
    folder: &Path,
) -> Outcome<T> {
    ensure_folder_with(ctx, gate, out, folder, |f| f.is_dir())
}

/// [`ensure_folder`], the folder looked at with `is_dir` (a test's).
fn ensure_folder_with<T>(
    ctx: &egui::Context,
    gate: &mut FolderGate<T>,
    out: Outcome<T>,
    folder: &Path,
    is_dir: impl FnOnce(&Path) -> bool + Send + 'static,
) -> Outcome<T> {
    gate.pass(
        ctx,
        out,
        folder,
        || move |f: &PathBuf| f.as_os_str().is_empty() || is_dir(f.as_path()),
        |there| (!there).then(|| CreateFolder::new(folder.to_owned())),
        CreateFolder::show,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #440: Download File Info behind the Create folder question ended without a closing pass,
    /// and the next one opened as a new, hidden window that was never told to show: later
    /// downloads waited behind it for good.
    #[test]
    fn a_dialog_gone_without_a_closing_pass_shows_when_it_opens_again() {
        egui::Context::set_immediate_viewport_renderer(move |ctx, viewport| {
            let input = egui::RawInput {
                viewport_id: viewport.ids.this,
                viewports: std::iter::once((viewport.ids.this, egui::ViewportInfo::default()))
                    .collect(),
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(300.0, 200.0))),
                ..Default::default()
            };
            let mut ui_cb = viewport.viewport_ui_cb;
            let mut out = ctx.run_ui(input, |ui| ui_cb(ui));
            out.textures_delta.clear();
        });
        // A test-only title, kept out of the catalog (extract.py reads literal titles).
        let title = String::from("test window 440");
        let title = title.as_str();
        let id = "test-440";
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        // A pass of the main window with the dialog (or not); whether it was told to show.
        let pass = |open: bool| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                if open {
                    let _ = window(
                        ui.ctx(),
                        id,
                        title,
                        [300.0, 200.0],
                        false,
                        egui::Color32::LIGHT_GRAY,
                        |_, _| Outcome::<()>::Open,
                    );
                }
            });
            out.textures_delta.clear();
            out.viewport_output
                .get(&ViewportId::from_hash_of(id))
                .is_some_and(|o| {
                    o.commands
                        .iter()
                        .any(|c| matches!(c, egui::ViewportCommand::Visible(true)))
                })
        };
        assert!(
            !pass(true),
            "the first frame only gives the window its theme"
        );
        assert!(pass(true), "the second frame shows it");
        assert!(!pass(true));
        // The dialog ended while its window was open (no pass with the Outcome that closes it),
        // and a pass without it: egui destroys that window.
        assert!(!pass(false));
        // Opened again: a new window.
        assert!(!pass(true), "again, first its theme");
        assert!(pass(true), "and then it is shown");
        assert!(!pass(true));
    }

    /// #394: the question's answers: Enter ("Yes", the default) cancels the waiting ones too,
    /// Escape ("No") skips this one alone, and with no answer nothing is decided.
    #[test]
    fn cancel_waiting_answers_enter_escape_and_nothing() {
        // The key a pass presses, delivered to the window that renders the question (a
        // thread-local: the renderer is global, and another test's window must see no key).
        thread_local! {
            static KEY: std::cell::Cell<Option<egui::Key>> = const { std::cell::Cell::new(None) };
        }
        egui::Context::set_immediate_viewport_renderer(move |ctx, viewport| {
            let events = KEY.with(|k| {
                k.take().map(|key| {
                    vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::default(),
                    }]
                })
            });
            let input = egui::RawInput {
                viewport_id: viewport.ids.this,
                events: events.unwrap_or_default(),
                viewports: std::iter::once((viewport.ids.this, egui::ViewportInfo::default()))
                    .collect(),
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(400.0, 120.0))),
                ..Default::default()
            };
            let mut ui_cb = viewport.viewport_ui_cb;
            let mut out = ctx.run_ui(input, |ui| ui_cb(ui));
            out.textures_delta.clear();
        });
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        // A pass of the question, with a key pressed in it (or not); the answer it gave.
        let pass = |key: Option<egui::Key>, answer: &std::cell::Cell<Option<Outcome<bool>>>| {
            KEY.with(|k| k.set(key));
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                answer.set(Some(cancel_waiting(ui.ctx(), 2)));
            });
            out.textures_delta.clear();
            answer.take()
        };
        let answer = std::cell::Cell::new(None);
        assert!(matches!(pass(None, &answer), Some(Outcome::Open)));
        assert!(matches!(
            pass(Some(egui::Key::Enter), &answer),
            Some(Outcome::Done(true))
        ));
        // Opened again: a new question.
        assert!(matches!(pass(None, &answer), Some(Outcome::Open)));
        assert!(matches!(
            pass(Some(egui::Key::Escape), &answer),
            Some(Outcome::Done(false))
        ));
    }

    /// Add URL opens and its address box takes the focus, frame after frame, without the app
    /// hanging (0.2.2 froze here: an editable drop-down asked egui's context inside its lock).
    #[test]
    fn add_url_opens_and_takes_the_focus() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (address, history) in [
                ("", vec![]),
                ("https://example.com/file.zip", vec![]),
                (
                    "https://example.com/file.zip",
                    vec!["https://a.example/x.zip".to_owned()],
                ),
            ] {
                let ctx = egui::Context::default();
                let mut dlg = AddUrl {
                    address: address.to_owned(),
                    history,
                    ..AddUrl::default()
                };
                for _ in 0..6 {
                    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                        let _ = dlg.show(ui.ctx(), |_| None);
                    });
                    out.textures_delta.clear();
                }
            }
            tx.send(()).unwrap();
        });
        rx.recv_timeout(std::time::Duration::from_secs(20))
            .expect("Add URL hung");
    }

    /// #812: Left / Right in Add URL's Address box move its caret, and the focus stays in the
    /// box, not on OK beside it (Save As in Download File Info keeps them the same way,
    /// `winui::keep_edit_keys`). With addresses entered before, as Up / Down step through them.
    #[test]
    fn add_url_left_and_right_stay_in_the_address_box() {
        let ctx = egui::Context::default();
        let mut dlg = AddUrl {
            address: "https://example.com/file.zip".to_owned(),
            history: vec!["https://a.example/x.zip".to_owned()],
            ..AddUrl::default()
        };
        let mut frame = |key: Option<egui::Key>| {
            let events = key
                .map(|key| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                })
                .into_iter()
                .collect();
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                let _ = dlg.show(ui.ctx(), |_| None);
            });
            out.textures_delta.clear();
        };
        for _ in 0..4 {
            frame(None);
        }
        let in_box = ctx.memory(|m| m.focused());
        assert!(in_box.is_some(), "the Address box has the focus");
        for key in [
            egui::Key::ArrowRight,
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
        ] {
            frame(Some(key));
            frame(None);
            assert_eq!(
                ctx.memory(|m| m.focused()),
                in_box,
                "{key:?} keeps the focus in the box"
            );
        }
    }

    /// #661: Add URL asks for the clipboard as it opens and draws its frames without waiting for
    /// the answer, even from a clipboard owner that never answers; the link fills the box on the
    /// frame it comes, the caret after it as when it opens with it, and never a box typed in.
    #[test]
    fn add_url_never_waits_for_the_clipboard() {
        use std::time::{Duration, Instant};
        let (release, stuck) = std::sync::mpsc::channel::<()>();
        let stuck = std::sync::Mutex::new(stuck);
        let reader: &'static crate::clipboard::Reader =
            Box::leak(Box::new(crate::clipboard::Reader::new(
                move || {
                    let _ = stuck.lock().unwrap().recv();
                    Some(
                        " https://late.example/a.zip
"
                        .to_owned(),
                    )
                },
                || {},
            )));
        let ctx = egui::Context::default();
        let frame = |dlg: &mut AddUrl, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                let _ = dlg.show(ui.ctx(), |_| None);
            });
            out.textures_delta.clear();
        };
        let mut dlg = AddUrl {
            clip: crate::clipboard::Prefill::ask_of(reader),
            ..AddUrl::default()
        };
        let mut typed = AddUrl {
            clip: crate::clipboard::Prefill::ask_of(reader),
            ..AddUrl::default()
        };
        let started = Instant::now();
        for _ in 0..10 {
            frame(&mut dlg, vec![]);
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "10 frames took {:?}",
            started.elapsed()
        );
        assert!(dlg.address.is_empty(), "nothing yet");
        typed.address = "https://typed.example/b.zip".into();
        drop(release);
        let until = Instant::now() + Duration::from_secs(5);
        while dlg.address.is_empty() && Instant::now() < until {
            frame(&mut dlg, vec![]);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(dlg.address, "https://late.example/a.zip");
        // The caret is at its end, as in a box that opened with it: typing goes after it.
        frame(&mut dlg, vec![]);
        frame(&mut dlg, vec![egui::Event::Text("x".into())]);
        assert_eq!(dlg.address, "https://late.example/a.zipx");
        while typed.clip.pending() && Instant::now() < until {
            frame(&mut typed, vec![]);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!typed.clip.pending(), "answered");
        assert_eq!(typed.address, "https://typed.example/b.zip", "typed: kept");
    }

    #[test]
    fn add_url_takes_only_an_address_it_downloads_from_the_clipboard() {
        let fill = |clip: &str| {
            let mut d = AddUrl::default();
            d.fill_from_clipboard(clip);
            d.address
        };
        assert_eq!(
            fill(
                " https://a.example/f.zip 
"
            ),
            "https://a.example/f.zip"
        );
        assert_eq!(fill("magnet:?xt=urn:btih:ab"), "magnet:?xt=urn:btih:ab");
        assert_eq!(fill("some words"), "");
        assert_eq!(fill("mailto:a@b.example"), "");
    }

    /// #511: an address with an Options → Sites Logins entry shows that login in "Use
    /// authorization" (the match the download uses), follows the address while left as filled,
    /// and keeps a login typed in the box. OK hands on only a login of the user's own.
    #[test]
    fn add_url_shows_the_sites_logins_entry_for_its_address() {
        // A made-up login; asserts don't print it.
        const PASS: &str = "dummy-pass";
        let settings = crate::settings::Settings {
            site_logins: vec![(
                "files.example.com/private".into(),
                "demo".into(),
                PASS.into(),
            )],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut dlg = AddUrl {
            address: "https://files.example.com/private/x.zip".into(),
            ..AddUrl::default()
        };
        let pass = |dlg: &mut AddUrl| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = dlg.show(ui.ctx(), |u| settings.site_login(u));
            });
            out.textures_delta.clear();
        };
        pass(&mut dlg);
        assert!(dlg.auth, "Use authorization is ticked");
        assert_eq!(dlg.login, "demo");
        assert!(dlg.password == PASS, "the password is the saved one");
        assert!(dlg.typed_login().is_none(), "the download follows Options");

        // Another address: the box filled from Options empties again.
        dlg.address = "https://files.example.com/public/x.zip".into();
        pass(&mut dlg);
        assert!(!dlg.auth);
        assert!(dlg.login.is_empty() && dlg.password.is_empty());

        // Back to the address, then a login typed over it: kept, and handed on.
        dlg.address = "https://files.example.com/private/y.zip".into();
        pass(&mut dlg);
        assert!(dlg.auth && dlg.login == "demo");
        dlg.login = "someone".into();
        dlg.address = "https://files.example.com/public/y.zip".into();
        pass(&mut dlg);
        assert!(dlg.auth && dlg.login == "someone", "a typed login stays");
        assert_eq!(dlg.typed_login().map(|l| l.0).as_deref(), Some("someone"));
    }

    /// #207: a dialog closed by the window manager (its title bar's ×) is painted in the pass
    /// that closes it. The dialog's window is rendered as eframe renders it: its own pass, after
    /// the window was cleared to transparent black. A pass that painted nothing left the window
    /// black on screen until it was destroyed (seconds, on Linux).
    #[test]
    fn a_dialog_closed_by_the_window_manager_is_painted_in_its_last_pass() {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        let close = Rc::new(Cell::new(false));
        // How many shapes each pass of the dialog's window painted.
        let painted = Rc::new(RefCell::new(Vec::new()));
        {
            let (close, painted) = (close.clone(), painted.clone());
            egui::Context::set_immediate_viewport_renderer(move |ctx, viewport| {
                let mut info = egui::ViewportInfo::default();
                if close.get() {
                    info.events.push(egui::ViewportEvent::Close);
                }
                let input = egui::RawInput {
                    viewport_id: viewport.ids.this,
                    viewports: std::iter::once((viewport.ids.this, info)).collect(),
                    screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(300.0, 200.0))),
                    ..Default::default()
                };
                let mut ui_cb = viewport.viewport_ui_cb;
                let mut out = ctx.run_ui(input, |ui| ui_cb(ui));
                out.textures_delta.clear();
                painted.borrow_mut().push(out.shapes.len());
            });
        }
        // A test-only title, kept out of the catalog (extract.py reads literal titles).
        let title = String::from("test window");
        let title = title.as_str();
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let pass = |closing: bool| {
            close.set(closing);
            let mut outcome = None;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                outcome = Some(window(
                    ui.ctx(),
                    "test-207",
                    title,
                    [300.0, 200.0],
                    false,
                    egui::Color32::LIGHT_GRAY,
                    |ui, origin| {
                        ui.painter().text(
                            origin + vec2(10.0, 10.0),
                            egui::Align2::LEFT_TOP,
                            "Mister Download Manager",
                            egui::FontId::proportional(12.0),
                            egui::Color32::BLACK,
                        );
                        Outcome::<()>::Open
                    },
                ));
            });
            out.textures_delta.clear();
            outcome.unwrap()
        };
        assert!(matches!(pass(false), Outcome::Open));
        assert!(matches!(pass(true), Outcome::Closed));
        let painted = painted.borrow();
        assert_eq!(
            painted.len(),
            2,
            "the dialog's window rendered once per pass"
        );
        assert!(painted[0] > 0, "the open dialog painted nothing");
        assert_eq!(
            painted[1], painted[0],
            "the pass that closes the dialog paints it as the one before"
        );
    }

    /// A look at the disk that waits until the test lets it go (a sleeping drive, a network
    /// folder that answers late); then answers as the engine would on `taken` names.
    fn slow_look(taken: &'static [&'static str]) -> (Look, std::sync::mpsc::Sender<()>) {
        let (release, stuck) = std::sync::mpsc::channel::<()>();
        let stuck = Mutex::new(stuck);
        let look: Look = Arc::new(move |ask: &PlaceAsk| {
            // Released once: every look after it answers at once.
            let _ = stuck.lock().unwrap().recv();
            let mut name = ask.name.clone();
            if ask.number {
                let (stem, ext) = ask.name.rsplit_once('.').unwrap_or((ask.name.as_str(), ""));
                let ext = if ext.is_empty() {
                    String::new()
                } else {
                    format!(".{ext}")
                };
                let mut n = 0;
                while taken.contains(&name.as_str()) {
                    n += 1;
                    name = format!("{stem} ({n}){ext}");
                }
            }
            Found {
                dest: ask.dir.join(name),
                folder: true,
            }
        });
        (look, release)
    }

    /// Wait (at most 5 s) until `place` answers for `save_as`.
    fn answered(place: &mut SaveAsPlace, save_as: &mut String) -> Option<bool> {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < until {
            if let Some(there) = place.follow(save_as) {
                return Some(there);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        None
    }

    /// #743: a disk that doesn't answer holds up no frame of Download File Info. Save As shows
    /// the name as given (no " (1)": not known yet), and takes the free numbered one when the
    /// answer comes at last.
    #[test]
    fn a_disk_that_never_answers_holds_up_no_frame() {
        let dir = std::env::temp_dir().join("mdm-743-slow");
        let (look, release) = slow_look(&["a.zip"]);
        let mut place = SaveAsPlace::with(look, None);
        let started = std::time::Instant::now();
        let mut save_as = place.name_in(&dir, "a.zip");
        assert_eq!(save_as, display_path(&dir.join("a.zip")), "the guess");
        for _ in 0..1000 {
            assert_eq!(place.follow(&mut save_as), None);
            assert_eq!(place.folder(&mut save_as), None, "not known yet");
        }
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "{:?} for 1000 frames",
            started.elapsed()
        );
        assert_eq!(save_as, display_path(&dir.join("a.zip")), "still the guess");
        // The disk answers: the name is taken there.
        drop(release);
        assert_eq!(answered(&mut place, &mut save_as), Some(true));
        assert_eq!(save_as, display_path(&dir.join("a (1).zip")));
        // Asked once: the next frames keep it.
        assert_eq!(place.folder(&mut save_as), Some(true));
        assert_eq!(save_as, display_path(&dir.join("a (1).zip")));
    }

    /// An answer comes for the text it was asked for only: Save As typed over meanwhile keeps
    /// what was typed, and a new name asked for (another folder) drops the old question's.
    #[test]
    fn a_late_answer_never_replaces_what_was_typed_or_picked_since() {
        let dir = std::env::temp_dir().join("mdm-743-typed");
        let (look, release) = slow_look(&["a.zip", "b.zip"]);
        let mut place = SaveAsPlace::with(look, None);
        let _guess = place.name_in(&dir, "a.zip");
        // Typed over before the disk answered.
        let mut save_as = display_path(&dir.join("mine.zip"));
        drop(release);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(place.follow(&mut save_as), None);
        assert_eq!(save_as, display_path(&dir.join("mine.zip")), "typed: kept");
        // Typed text has its folder looked at, as typed (never numbered here).
        let mut there = None;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while there.is_none() && std::time::Instant::now() < until {
            there = place.folder(&mut save_as);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(there, Some(true));
        assert_eq!(save_as, display_path(&dir.join("mine.zip")));
        // Another name: its own answer, not the last one's.
        let other = dir.join("elsewhere");
        save_as = place.name_in(&other, "b.zip");
        assert_eq!(answered(&mut place, &mut save_as), Some(true));
        assert_eq!(save_as, display_path(&other.join("b (1).zip")));
    }

    /// The real look follows the engine's rules: a name taken on the disk (a file, another
    /// download's part file) is numbered on, the folder said there or not.
    #[test]
    fn the_look_numbers_as_the_engine_does() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.zip"), b"x").unwrap();
        std::fs::write(
            mdm_engine::naming::part_path(&d.path().join("a (1).zip")),
            b"x",
        )
        .unwrap();
        let mut place = SaveAsPlace::new(None);
        let mut save_as = place.name_in(d.path(), "a.zip");
        assert_eq!(answered(&mut place, &mut save_as), Some(true));
        assert_eq!(save_as, display_path(&d.path().join("a (2).zip")));
        assert_eq!(
            resolve_path(&save_as),
            mdm_engine::naming::destination(d.path(), "a.zip")
        );
        let gone = d.path().join("not-there");
        let mut save_as = place.name_in(&gone, "a.zip");
        assert_eq!(answered(&mut place, &mut save_as), Some(false));
        assert_eq!(save_as, display_path(&gone.join("a.zip")));
    }

    /// #743: Start in a dialog whose folder is on a disk slow to answer holds up no frame: the
    /// dialog stays as it is until the answer comes, then goes on with what Start said.
    #[test]
    fn start_waits_for_the_folder_without_holding_up_a_frame() {
        let ctx = egui::Context::default();
        let (release, stuck) = std::sync::mpsc::channel::<()>();
        let mut gate = FolderGate::<u32>::default();
        let folder = std::env::temp_dir().join("mdm-743-gate");
        let started = std::time::Instant::now();
        let is_dir = move |_: &Path| {
            let _ = stuck.recv();
            true
        };
        let first = ensure_folder_with(&ctx, &mut gate, Outcome::Done(7), &folder, is_dir);
        assert!(matches!(first, Outcome::Open));
        for _ in 0..1000 {
            // A second Start meanwhile changes nothing; nor does a frame without one.
            let again = ensure_folder_with(&ctx, &mut gate, Outcome::Done(8), &folder, |_| {
                unreachable!("looked at once")
            });
            assert!(matches!(again, Outcome::Open));
        }
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
        drop(release);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let done = loop {
            match ensure_folder_with(&ctx, &mut gate, Outcome::Open, &folder, |_| {
                unreachable!("looked at once")
            }) {
                Outcome::Done(v) => break Some(v),
                _ if std::time::Instant::now() > until => break None,
                _ => std::thread::sleep(std::time::Duration::from_millis(2)),
            }
        };
        assert_eq!(done, Some(7), "the first Start's");
        // Let go: the next Start is looked at afresh.
        let next = ensure_folder_with(&ctx, &mut gate, Outcome::Done(9), &folder, |_| true);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut next = next;
        while !matches!(next, Outcome::Done(_)) && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(2));
            next = ensure_folder_with(&ctx, &mut gate, Outcome::Open, &folder, |_| true);
        }
        assert!(matches!(next, Outcome::Done(9)));
    }
}
