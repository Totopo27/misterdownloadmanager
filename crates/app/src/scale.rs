//! View → Interface size (#262): every window of MDM drawn larger, 100 % to 200 %, as a browser
//! zooms a page (Ctrl + / Ctrl − / Ctrl 0 too). It is egui's zoom factor, which multiplies the
//! display's own scale: the windows are laid out in points, so a dialog, a menu or the drop
//! target opens that much larger, and an open one is made larger with it.
//!
//! A window larger than the screen stays usable: it opens no larger than the screen's work area
//! and scrolls what doesn't fit ([`pane`]), with its title bar on the screen ([`keep_inside`]).

use egui::{Pos2, Rect, Ui, Vec2, pos2, vec2};

/// The sizes offered, in percent. 110 to 120 sit between 100 and 125, for a dense screen (1440p
/// at 100 % is small, 125 % large): the text is drawn at its own fractional size.
pub const STEPS: [u16; 8] = [100, 110, 115, 120, 125, 150, 175, 200];

/// A size as one of [`STEPS`]: the nearest (a value edited into the settings file).
pub fn step(percent: u16) -> u16 {
    STEPS
        .into_iter()
        .min_by_key(|s| (i32::from(*s) - i32::from(percent)).abs())
        .unwrap_or(100)
}

/// egui's zoom factor for a size.
pub fn factor(percent: u16) -> f32 {
    f32::from(step(percent)) / 100.0
}

/// The size a zoom factor shows: the nearest step.
pub fn of_zoom(zoom: f32) -> u16 {
    step((zoom * 100.0).round().clamp(0.0, 1000.0) as u16)
}

/// Ctrl + (`up`) or Ctrl −: the next size from `percent`, kept at either end.
pub fn next(percent: u16, up: bool) -> u16 {
    let at = STEPS.iter().position(|s| *s == step(percent)).unwrap_or(0);
    let to = if up {
        (at + 1).min(STEPS.len() - 1)
    } else {
        at.saturating_sub(1)
    };
    STEPS[to]
}

/// MDM_ZOOM=2: a test run drawn at that zoom (sharp 2× screenshots), whatever the setting,
/// which it neither shows nor changes.
pub fn test_zoom() -> Option<f32> {
    static Z: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *Z.get_or_init(|| {
        std::env::var("MDM_ZOOM")
            .ok()
            .and_then(|z| z.parse::<f32>().ok())
            .filter(|z| (0.5..=4.0).contains(z))
    })
}

/// The zoom a start draws at: MDM_ZOOM's, else the setting's.
pub fn startup(setting: u16) -> f32 {
    test_zoom().unwrap_or_else(|| factor(setting))
}

/// Draw at `zoom` from the first frame, measured at it too: set before that frame's input is
/// taken ([`set`] applies at the next frame, whose window rects are still measured at the old
/// zoom).
pub fn start(ctx: &egui::Context, zoom: f32) {
    ctx.options_mut(|o| o.zoom_factor = zoom);
}

/// Draw every window at `percent` from the next frame on.
pub fn set(ctx: &egui::Context, percent: u16) {
    ctx.set_zoom_factor(factor(percent));
}

/// The interface size keys, as a browser takes them, pressed in the window being drawn: Ctrl +
/// (or Ctrl =, without Shift), Ctrl − and Ctrl 0, the numeric keypad's too (Command on a Mac).
/// The size changes at once, in every window.
pub fn keys(ctx: &egui::Context) {
    use egui::gui_zoom::kb_shortcuts as k;
    let now = of_zoom(ctx.zoom_factor());
    let want = ctx.input_mut(|i| {
        if i.consume_shortcut(&k::ZOOM_RESET) {
            Some(100)
        } else if i.consume_shortcut(&k::ZOOM_IN) || i.consume_shortcut(&k::ZOOM_IN_SECONDARY) {
            Some(next(now, true))
        } else if i.consume_shortcut(&k::ZOOM_OUT) {
            Some(next(now, false))
        } else {
            None
        }
    });
    if let Some(p) = want
        && p != now
    {
        set(ctx, p);
    }
}

/// What a window's frame adds around its client, in screen pixels at 100 % display scale: the
/// title bar and borders (Windows draws them at the display's scale, never the interface's).
const FRAME: Vec2 = vec2(16.0, 40.0);
/// Linux and macOS, where the work area isn't known here: the frame and the desktop's panels
/// (a top bar, a dock or a taskbar) together, an estimate.
#[cfg(not(windows))]
const FRAME_AND_PANELS: Vec2 = vec2(32.0, 120.0);

/// MDM_TEST_SCREEN=1366x768, for off-screen test runs: windows are sized for a screen that size
/// (at 100 %, with a 40 px taskbar) instead of this computer's, to check them on a small one.
fn test_screen() -> Option<Vec2> {
    static S: std::sync::OnceLock<Option<Vec2>> = std::sync::OnceLock::new();
    *S.get_or_init(|| {
        let v = std::env::var("MDM_TEST_SCREEN").ok()?;
        let (w, h) = v.split_once('x')?;
        Some(vec2(w.trim().parse().ok()?, h.trim().parse().ok()?))
    })
}

/// The monitor's work area around `at` (the screen without the taskbar), in points of the
/// window being drawn; where the window manager doesn't say (Linux, macOS), and in an off-screen
/// test run (its windows are far off the screens, and must stay there), none.
pub fn work_area(ctx: &egui::Context, at: Pos2) -> Option<Rect> {
    if crate::offscreen() {
        return None;
    }
    #[cfg(windows)]
    {
        let ppp = ctx.pixels_per_point();
        let [l, t, r, b] =
            crate::win32::work_area((at.x * ppp).round() as i32, (at.y * ppp).round() as i32)?;
        Some(Rect::from_min_max(
            pos2(l as f32, t as f32) / ppp,
            pos2(r as f32, b as f32) / ppp,
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = (ctx, at);
        None
    }
}

/// The largest client a window can have on the screen around `at`, in points: the work area
/// less a frame (Windows), else the monitor less a frame and the desktop's panels. None when
/// unknown.
pub fn room(ctx: &egui::Context, at: Pos2) -> Option<Vec2> {
    let zoom = ctx.zoom_factor();
    if let Some(screen) = test_screen() {
        let ppp = ctx.pixels_per_point();
        return Some(vec2(screen.x, screen.y - 40.0) / ppp - FRAME / zoom);
    }
    #[cfg(windows)]
    {
        let ppp = ctx.pixels_per_point();
        let [l, t, r, b] =
            crate::win32::work_area((at.x * ppp).round() as i32, (at.y * ppp).round() as i32)?;
        Some(vec2((r - l) as f32, (b - t) as f32) / ppp - FRAME / zoom)
    }
    #[cfg(not(windows))]
    {
        let _ = at;
        let monitor = ctx
            .input_for(egui::ViewportId::ROOT, |i| i.viewport().monitor_size)
            .filter(|m| m.x > 0.0 && m.y > 0.0)?;
        Some(monitor - FRAME_AND_PANELS / zoom)
    }
}

/// `size` made no larger than `room` (each side at least 120 points, a usable window).
pub fn fit(size: Vec2, room: Option<Vec2>) -> Vec2 {
    match room {
        Some(r) => size.min(r.max(vec2(120.0, 120.0))),
        None => size,
    }
}

/// A fixed layout of `size` as its window can show it in `room`: [`fit`], and wider (or
/// taller) by a scroll bar when the other side is cut, so the bar that side scrolls with doesn't
/// cut this one too.
pub fn shown(size: Vec2, room: Option<Vec2>) -> Vec2 {
    let fitted = fit(size, room);
    let bar = crate::scrollbar::THICK;
    let grown = vec2(
        if fitted.y < size.y {
            size.x + bar
        } else {
            size.x
        },
        if fitted.x < size.x {
            size.y + bar
        } else {
            size.y
        },
    );
    fit(grown, room)
}

/// Where a window of `outer` size at `p` goes to have it all inside `area`, its top-left corner
/// (the title bar) first when it is larger.
pub fn keep_inside(p: Pos2, outer: Vec2, area: Rect) -> Pos2 {
    pos2(
        p.x.min(area.max.x - outer.x).max(area.min.x),
        p.y.min(area.max.y - outer.y).max(area.min.y),
    )
}

/// A window's frame in points (`outer` = client + this), for [`keep_inside`].
pub fn frame(ctx: &egui::Context) -> Vec2 {
    FRAME / ctx.zoom_factor()
}

/// The root window being drawn (the main window, the crash report window) brought inside the
/// screen once: made no larger than its work area, and moved onto it (Windows). After a start
/// at a large interface size, its window was laid out for a larger screen. `layout`: the size
/// of a fixed layout the window shows ([`pane`]), given room for a scroll bar when cut.
pub fn fit_window(ctx: &egui::Context, min: Vec2, layout: Option<Vec2>) {
    let (inner, outer, maximized) = ctx.input(|i| {
        let v = i.viewport();
        (v.inner_rect, v.outer_rect, v.maximized == Some(true))
    });
    let (Some(inner), Some(outer)) = (inner, outer) else {
        return;
    };
    if maximized {
        return;
    }
    let Some(room) = room(ctx, outer.center()) else {
        return;
    };
    let size = match layout {
        Some(l) => shown(l, Some(room)),
        None => fit(inner.size(), Some(room)),
    }
    .max(min.min(room));
    if (size - inner.size()).length() > 0.5 {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }
    if let Some(area) = work_area(ctx, outer.center()) {
        let whole = size + (outer.size() - inner.size());
        let p = keep_inside(outer.min, whole, area);
        if (p - outer.min).length() > 0.5 {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(p));
        }
    }
}

/// A layout of `content` points drawn in the space `ui` has, scrolled when that is smaller (a
/// fixed-size window larger than the screen, at a large interface size): scroll bars at the
/// trailing and bottom edges (MDM's own, `scrollbar::bar`), the wheel where nothing inside takes
/// it, and the control the keyboard focuses brought into view. `body` gets the layout's origin
/// (moved by the scrolling), as a window's client origin. Fits: `body` as is, nothing added.
pub fn pane<R>(
    ui: &mut Ui,
    id: egui::Id,
    content: Vec2,
    body: impl FnOnce(&mut Ui, Pos2) -> R,
) -> R {
    use crate::scrollbar::THICK;
    let view = ui.max_rect();
    let (need_v, need_h) = bars(content, view.size(), ui.ctx().pixels_per_point());
    if !need_v && !need_h {
        return body(ui, view.min);
    }
    let rtl = crate::i18n::rtl();
    let lead_bar = if need_v && rtl { THICK } else { 0.0 };
    let trail_bar = if need_v && !rtl { THICK } else { 0.0 };
    let port = Rect::from_min_max(
        pos2(view.min.x + lead_bar, view.min.y),
        pos2(
            view.max.x - trail_bar,
            view.max.y - if need_h { THICK } else { 0.0 },
        ),
    );
    let key = id.with("pane");
    let max = (content - port.size()).max(Vec2::ZERO);
    let mut off = ui
        .ctx()
        .data(|d| d.get_temp::<Vec2>(key))
        .unwrap_or_default()
        .clamp(Vec2::ZERO, max);
    let origin = |off: Vec2| {
        let x = if rtl {
            port.max.x - content.x + off.x
        } else {
            port.min.x - off.x
        };
        pos2(x.round(), (port.min.y - off.y).round())
    };
    let at = origin(off);
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(at, content)));
    inner.set_clip_rect(port.intersect(ui.clip_rect()));
    let out = body(&mut inner, at);

    // The control the keyboard just focused (Tab), into view.
    let focus_key = id.with("pane-focus");
    let focused = ui.ctx().memory(|m| m.focused());
    if focused
        != ui
            .ctx()
            .data(|d| d.get_temp::<Option<egui::Id>>(focus_key))
            .flatten()
    {
        ui.ctx().data_mut(|d| d.insert_temp(focus_key, focused));
        if let Some(r) = focused
            .and_then(|f| ui.ctx().read_response(f))
            .map(|r| r.rect)
        {
            off += into_view(r, port, rtl);
        }
    }
    // The wheel, unless a list inside took it.
    let hovered = ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|p| port.contains(p));
    if hovered && need_v && !crate::winui::wheel_taken(ui) {
        off.y += crate::winui::wheel_rows(ui, 16.0, (port.height() / 16.0).floor().max(1.0)) * 16.0;
    }
    // The bars, in the window's direction.
    let _surface = crate::winui::Surface::enter(view.min.x, view.max.x);
    if need_v {
        let x = if rtl { view.min.x } else { port.max.x };
        let track = Rect::from_min_max(pos2(x, view.min.y), pos2(x + THICK, port.max.y));
        crate::scrollbar::bar(
            ui,
            id.with("pane-v"),
            track,
            true,
            content.y,
            port.height(),
            16.0,
            &mut off.y,
        );
    }
    if need_h {
        let track = Rect::from_min_max(pos2(port.min.x, port.max.y), pos2(port.max.x, view.max.y));
        crate::scrollbar::bar(
            ui,
            id.with("pane-h"),
            track,
            false,
            content.x,
            port.width(),
            16.0,
            &mut off.x,
        );
    }
    if need_v && need_h {
        let x = if rtl { view.min.x } else { port.max.x };
        crate::winui::fill(
            ui.painter(),
            x,
            port.max.y,
            x + THICK - 1.0,
            view.max.y - 1.0,
            crate::scrollbar::CORNER(),
        );
    }
    let off = off.clamp(Vec2::ZERO, max);
    if origin(off) != at {
        ui.ctx().request_repaint();
    }
    ui.ctx().data_mut(|d| d.insert_temp(key, off));
    out
}

/// Which scroll bars a `content` needs in `view`: (vertical, horizontal), both in points, with
/// `ppp` device pixels to a point. A bar takes room from the other direction, which can make
/// that one needed too.
fn bars(content: Vec2, view: Vec2, ppp: f32) -> (bool, bool) {
    use crate::scrollbar::THICK;
    // One device pixel over is the window's client rounded down at the interface size (520
    // points at 125 % is 650 px, and the client came back 649), not content left out: both bars
    // showed over a window whose content was wholly visible (#672). Counted in whole device
    // pixels, so the conversion's float noise can't tip it either way.
    let over = |content: f32, view: f32| ((content - view) * ppp).round() > 1.0;
    let mut v = over(content.y, view.y);
    let h = over(content.x, view.x - if v { THICK } else { 0.0 });
    v = v || (h && over(content.y, view.y - THICK));
    (v, h)
}

/// How far to scroll (offset change) to bring `r` inside `port`, with a little margin; along x
/// from the trailing side in a right-to-left window (its offset runs leftwards).
fn into_view(r: Rect, port: Rect, rtl: bool) -> Vec2 {
    const MARGIN: f32 = 4.0;
    let along = |lo: f32, hi: f32, plo: f32, phi: f32| {
        if lo < plo {
            lo - plo - MARGIN
        } else if hi > phi {
            (hi - phi + MARGIN).min(lo - plo)
        } else {
            0.0
        }
    };
    let dx = along(r.min.x, r.max.x, port.min.x, port.max.x);
    let dy = along(r.min.y, r.max.y, port.min.y, port.max.y);
    vec2(if rtl { -dx } else { dx }, dy)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sizes offered, Ctrl + and Ctrl − through them, and a hand-edited value as the nearest.
    #[test]
    fn sizes_step_like_a_browsers_zoom() {
        assert_eq!(step(100), 100);
        assert_eq!(step(130), 125);
        assert_eq!(step(112), 110);
        assert_eq!(step(118), 120);
        assert_eq!(step(0), 100);
        assert_eq!(step(500), 200);
        assert_eq!(factor(175), 1.75);
        assert_eq!(of_zoom(1.5), 150);
        assert_eq!(of_zoom(1.3), 125);
        assert_eq!(of_zoom(1.1), 110);
        assert_eq!(next(100, true), 110);
        assert_eq!(next(120, true), 125);
        assert_eq!(next(175, true), 200);
        assert_eq!(next(200, true), 200);
        assert_eq!(next(125, false), 120);
        assert_eq!(next(110, false), 100);
        assert_eq!(next(100, false), 100);
    }

    /// Ctrl + / Ctrl = / Ctrl − / Ctrl 0 through egui as a window gets them: a step each, the
    /// zoom applied from the next frame; Shift (Ctrl + on a US keyboard) doesn't matter, and a
    /// plain + is typing, not a size.
    #[test]
    fn the_keys_step_the_size_like_a_browser() {
        let ctx = egui::Context::default();
        let press = |key: egui::Key, modifiers: egui::Modifiers| {
            let input = egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            };
            // The new zoom applies at the next frame's start. (No renderer takes the font
            // atlas here.)
            ctx.run_ui(input, |ui| keys(ui.ctx()))
                .textures_delta
                .clear();
            ctx.run_ui(Default::default(), |_| {})
                .textures_delta
                .clear();
            of_zoom(ctx.zoom_factor())
        };
        let ctrl = egui::Modifiers::COMMAND;
        assert_eq!(press(egui::Key::Plus, ctrl), 110);
        assert_eq!(press(egui::Key::Plus, ctrl), 115);
        assert_eq!(press(egui::Key::Plus, ctrl), 120);
        assert_eq!(press(egui::Key::Plus, ctrl), 125);
        assert_eq!(press(egui::Key::Equals, ctrl), 150);
        assert_eq!(press(egui::Key::Plus, ctrl | egui::Modifiers::SHIFT), 175);
        assert_eq!(press(egui::Key::Plus, ctrl), 200);
        assert_eq!(press(egui::Key::Plus, ctrl), 200, "kept at the largest");
        assert_eq!(press(egui::Key::Minus, ctrl), 175);
        assert_eq!(press(egui::Key::Plus, egui::Modifiers::NONE), 175);
        assert_eq!(press(egui::Key::Num0, ctrl), 100);
        assert_eq!(press(egui::Key::Minus, ctrl), 100, "kept at the smallest");
    }

    /// Cut in height, a fixed layout's window is a scroll bar wider (if the room allows).
    #[test]
    fn a_cut_side_makes_room_for_its_scroll_bar() {
        use crate::scrollbar::THICK;
        let room = Some(vec2(667.0, 324.0));
        assert_eq!(shown(vec2(430.0, 506.0), room), vec2(430.0 + THICK, 324.0));
        assert_eq!(shown(vec2(430.0, 300.0), room), vec2(430.0, 300.0));
        assert_eq!(shown(vec2(660.0, 506.0), room), vec2(667.0, 324.0));
        assert_eq!(shown(vec2(760.0, 300.0), room), vec2(667.0, 300.0 + THICK));
    }

    /// A window larger than the room is made the room's size; never below a usable minimum.
    #[test]
    fn a_window_is_no_larger_than_the_screen() {
        let room = Some(vec2(667.0, 324.0));
        assert_eq!(fit(vec2(430.0, 506.0), room), vec2(430.0, 324.0));
        assert_eq!(fit(vec2(430.0, 506.0), None), vec2(430.0, 506.0));
        assert_eq!(
            fit(vec2(430.0, 506.0), Some(vec2(50.0, 50.0))),
            vec2(120.0, 120.0)
        );
    }

    /// Off the bottom-right: moved back in; larger than the area: its top-left (the title bar) in.
    #[test]
    fn a_window_is_kept_inside_the_work_area() {
        let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(1366.0, 728.0));
        assert_eq!(
            keep_inside(pos2(1200.0, 600.0), vec2(400.0, 300.0), area),
            pos2(966.0, 428.0)
        );
        assert_eq!(
            keep_inside(pos2(-50.0, -80.0), vec2(1400.0, 900.0), area),
            pos2(0.0, 0.0)
        );
        assert_eq!(
            keep_inside(pos2(100.0, 100.0), vec2(400.0, 300.0), area),
            pos2(100.0, 100.0)
        );
    }

    /// A bar only where the content is larger; one bar's room can make the other needed.
    #[test]
    fn scroll_bars_only_where_needed() {
        assert_eq!(
            bars(vec2(430.0, 506.0), vec2(430.0, 506.0), 1.0),
            (false, false)
        );
        assert_eq!(
            bars(vec2(430.0, 506.0), vec2(600.0, 400.0), 1.0),
            (true, false)
        );
        assert_eq!(
            bars(vec2(760.0, 300.0), vec2(700.0, 400.0), 1.0),
            (false, true)
        );
        // Only taller, but the vertical bar leaves too little width.
        assert_eq!(
            bars(vec2(440.0, 506.0), vec2(450.0, 400.0), 1.0),
            (true, true)
        );
    }

    /// The focused control brought into view: below the port scrolls down, above scrolls up,
    /// inside nothing; right-to-left runs the other way along x.
    #[test]
    fn the_focused_control_is_scrolled_into_view() {
        let port = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 300.0));
        let below = Rect::from_min_max(pos2(10.0, 320.0), pos2(80.0, 341.0));
        assert_eq!(into_view(below, port, false), vec2(0.0, 45.0));
        let above = Rect::from_min_max(pos2(10.0, -30.0), pos2(80.0, -9.0));
        assert_eq!(into_view(above, port, false), vec2(0.0, -34.0));
        let inside = Rect::from_min_max(pos2(10.0, 10.0), pos2(80.0, 31.0));
        assert_eq!(into_view(inside, port, false), Vec2::ZERO);
        let right = Rect::from_min_max(pos2(420.0, 10.0), pos2(480.0, 31.0));
        assert_eq!(into_view(right, port, false), vec2(84.0, 0.0));
        assert_eq!(into_view(right, port, true), vec2(-84.0, 0.0));
    }

    /// #672: a client one device pixel short of the content is the window's size rounded down
    /// at the interface size, not a line of the content left out; the old half-point allowance
    /// put both bars up here.
    #[test]
    fn a_pixel_rounded_off_needs_no_bar() {
        assert_eq!(
            bars(vec2(650.0, 500.0), vec2(649.0, 500.0), 1.0),
            (false, false)
        );
        assert_eq!(
            bars(vec2(650.0, 500.0), vec2(650.0, 499.0), 1.0),
            (false, false)
        );
    }

    /// #672 at 125 %, where a device pixel is 0.8 points: the 649 px client of a 650 px
    /// (520-point) window needs no bar, one two pixels short does.
    #[test]
    fn a_device_pixel_at_125_percent() {
        let view = |px: f32| vec2(px / 1.25, 400.0);
        assert_eq!(bars(vec2(520.0, 300.0), view(649.0), 1.25), (false, false));
        assert_eq!(bars(vec2(520.0, 300.0), view(648.0), 1.25), (false, true));
        // At 200 % a point is two pixels: a point short is content left out.
        assert_eq!(
            bars(vec2(520.0, 300.0), vec2(519.0, 400.0), 2.0),
            (false, true)
        );
    }

    /// Content two pixels over is content to scroll to, and the bar it puts up takes room from the
    /// other direction, as Win32's do.
    #[test]
    fn content_left_out_needs_its_bar() {
        // Two pixels wider, well short of the height: the horizontal bar only.
        assert_eq!(
            bars(vec2(651.0, 480.0), vec2(649.0, 500.0), 1.0),
            (false, true)
        );
        // Two pixels taller, 20 narrower than the view: the vertical bar only.
        assert_eq!(
            bars(vec2(630.0, 502.0), vec2(650.0, 500.0), 1.0),
            (true, false)
        );
        // Filling the width and two pixels wider: the horizontal bar's 17 px also leave it too
        // tall, so both, as Win32 decides them.
        assert_eq!(
            bars(vec2(651.0, 500.0), vec2(649.0, 500.0), 1.0),
            (true, true)
        );
    }
}
