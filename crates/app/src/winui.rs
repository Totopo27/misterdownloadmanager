//! Windows dark-dialog controls, painted to IDM 6.42's measured pixels
//! (notes/ui-spec/dialogs.md "Dark dialog control styles"). Coordinates are client pixels;
//! "baseline" is the last pixel row of cap glyphs, as in the spec.

use egui::{
    Color32, FontFamily, FontId, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui,
    pos2, vec2,
};
use unicode_segmentation::UnicodeSegmentation;

// Every colour comes from the active theme (crate::palette, #45): these names are its roles.
crate::theme::themed! {
    /// Dialog background (light: light-dialogs.md).
    pub BG => window;
    pub TEXT => text;
    /// A group box's frame (it was #dcdcdc in dark too: brighter than the separators in it).
    pub GROUP_LINE => group_line;
    /// A list's column headers (the main list's): every list and table has the same (they had
    /// four: none, #f3f3f3, white).
    pub LIST_HEADER => header;
    /// Disabled text, the one grey of every control and label (there were five).
    pub TEXT_DISABLED => text_disabled;
    /// A greyed text box's text.
    pub EDIT_TEXT_DISABLED => text_disabled;
    /// A selection's fill (rows, a chosen day, selected text): Windows' accent by default.
    pub SELECTION => selection;
    /// Text on the selection.
    pub SELECTION_TEXT => selection_text;
}

/// Tahoma −11 ("MS Shell Dlg 2", 8 pt): every dialog body text.
pub fn tahoma() -> FontId {
    FontId::new(
        crate::theme::font_size(crate::theme::metrics().font_dialog),
        FontFamily::Name("tahoma".into()),
    )
}

/// Tahoma Bold −13: the Options pages' header titles.
pub fn tahoma_bold() -> FontId {
    FontId::new(
        crate::theme::font_size(crate::theme::metrics().font_dialog_bold),
        FontFamily::Name("tahoma-bold".into()),
    )
}

/// Tahoma Bold −16: a window's headline (the browser extension's, the helpers').
pub fn tahoma_title() -> FontId {
    FontId::new(
        crate::theme::font_size(crate::theme::metrics().font_title),
        FontFamily::Name("tahoma-bold".into()),
    )
}

/// Tahoma (IDM's dialogs), Tahoma Bold and Arial Bold from Windows' fonts folder. A family
/// whose file isn't there (macOS, Linux) is the interface font's, so drawing with it never fails.
pub fn install_fonts(defs: &mut egui::FontDefinitions, font_dir: Option<&std::path::Path>) {
    let ui = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for (family, file) in [
        ("tahoma", "tahoma.ttf"),
        ("tahoma-bold", "tahomabd.ttf"),
        ("arial-bold", "arialbd.ttf"),
    ] {
        let fonts = match font_dir.and_then(|d| std::fs::read(d.join(file)).ok()) {
            Some(bytes) => {
                defs.font_data.insert(
                    family.into(),
                    std::sync::Arc::new(egui::FontData::from_owned(bytes)),
                );
                vec![family.to_owned()]
            }
            _ => ui.clone(),
        };
        defs.families.insert(FontFamily::Name(family.into()), fonts);
    }
}

// ---- right to left (D-019) -----------------------------------------------------------------
//
// Every window is laid out to IDM's English pixels. In a right-to-left interface (Arabic,
// Persian, Hebrew, Urdu, Uyghur) each window's client area is a mirrored *surface*: what is made
// for it is made at its mirrored place, so it is drawn and hit-tested there.
//
// - `r` and the rect helpers (`rect`) give a rect where it is on screen: `x → L + R − x − w`.
//   Everything that takes a `Rect` takes such a rect, and puts its insides by their leading
//   (start) and trailing (end) edges (`lead`, `trail`, `fwd`).
// - Text helpers take an English pen x: the *start* of the text. In RTL the text box is placed
//   with its start at the mirrored edge — right-aligned there; a right-aligned text
//   (`text_right`) ends left-aligned; a centred one stays centred. Text itself is never
//   mirrored, and is laid out in the interface's direction (`direction`), so a mixed line reads in
//   order; data goes in as isolates (`i18n::ltr`, `i18n::isolate`).
// - Points given to widgets (a check box's glyph) are English points.
// - The pixel painters (`fill`, `px`, `solid`) paint where they are told; `fill_l` takes English
//   pixels.

thread_local! {
    /// The surfaces being laid out (left and right edge of each), innermost last; only in a
    /// right-to-left interface.
    static SURFACES: std::cell::RefCell<Vec<(f32, f32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A surface laid out while this lives: a window's client area from `left` to `right`, mirrored
/// in a right-to-left interface (see above). A left-to-right one needs nothing.
#[must_use]
pub struct Surface(());

impl Surface {
    pub fn enter(left: f32, right: f32) -> Self {
        Self::enter_if(crate::i18n::rtl(), left, right)
    }

    /// [`Surface::enter`] with the direction given (tests; a surface that is never mirrored).
    pub fn enter_if(rtl: bool, left: f32, right: f32) -> Self {
        SURFACES.with(|s| {
            s.borrow_mut().push(if rtl {
                (left, right)
            } else {
                (f32::NAN, f32::NAN)
            })
        });
        Self(())
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        SURFACES.with(|s| s.borrow_mut().pop());
    }
}

/// The mirrored surface being laid out: its left + right edge.
fn axis() -> Option<f32> {
    SURFACES.with(|s| {
        s.borrow()
            .last()
            .filter(|(l, _)| !l.is_nan())
            .map(|(l, r)| l + r)
    })
}

/// Is the surface being laid out mirrored (a right-to-left interface)?
pub fn rtl() -> bool {
    axis().is_some()
}

/// The screen x of an English edge x (a rect's side, a text's pen).
pub fn mx(x: f32) -> f32 {
    axis().map_or(x, |a| a - x)
}

/// The screen column of an English pixel column (inclusive pixel coordinates).
pub fn mcol(x: f32) -> f32 {
    axis().map_or(x, |a| a - 1.0 - x)
}

/// The screen rect of an English rect.
pub fn rect(rc: Rect) -> Rect {
    match axis() {
        Some(a) => Rect::from_min_max(pos2(a - rc.max.x, rc.min.y), pos2(a - rc.min.x, rc.max.y)),
        None => rc,
    }
}

/// The screen point of an English point that starts something `w` wide (its top-left in English
/// is its top-right in RTL): the top-left of the screen rect.
pub fn at(p: Pos2, w: f32) -> Pos2 {
    rect(Rect::from_min_size(p, vec2(w, 0.0))).min
}

/// A screen rect's leading (start) edge: left, or right in RTL.
pub fn lead(rc: Rect) -> f32 {
    if rtl() { rc.right() } else { rc.left() }
}

/// The reading direction along x: +1, or −1 in RTL.
pub fn fwd() -> f32 {
    if rtl() { -1.0 } else { 1.0 }
}

/// `caption` as it fits from its English pen `x` to the far edge of what the painter may draw on
/// (the window, or the box it sits in), keeping a 6 px margin, and ending 5 px before English x
/// `until` (a control after the caption): whole, or cut with "..." (true).
fn fit_caption_until(p: &Painter, x: f32, caption: &str, until: Option<f32>) -> (String, bool) {
    let clip = p.clip_rect();
    // The far edge in English coordinates: the screen's left edge when mirrored.
    let edge = if rtl() { mx(clip.left()) } else { clip.right() };
    let end = until.map_or(edge - 6.0, |u| (u - 5.0).min(edge - 6.0));
    let room = end - x;
    if text_width(p, caption) <= room {
        (caption.to_owned(), false)
    } else {
        (ellipsize(p, &tahoma(), caption, room.max(0.0)), true)
    }
}

/// A widget's id: `key` within its window. Ids are made from a widget's label and place, so the
/// viewport is part of them: two windows with the same widget in the same place (two progress
/// windows' Pause buttons) must not share one.
fn widget_id(ui: &Ui, key: impl std::hash::Hash + std::fmt::Debug) -> egui::Id {
    egui::Id::unique((ui.ctx().viewport_id(), key))
}

/// Pixel rect from spec numbers (x, y, w, h) relative to `origin`, where it is on screen.
pub fn r(origin: Pos2, x: f32, y: f32, w: f32, h: f32) -> Rect {
    rect(Rect::from_min_size(origin + vec2(x, y), vec2(w, h)))
}

/// The paragraph direction of interface text: the interface's.
pub fn direction() -> egui::epaint::text::TextDirection {
    use egui::epaint::text::TextDirection;
    if crate::i18n::rtl() {
        TextDirection::RightToLeft
    } else {
        TextDirection::LeftToRight
    }
}

/// `s` laid out on one line in the interface's direction.
pub fn galley(p: &Painter, font: FontId, s: &str, color: Color32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(s.to_owned(), font, color, f32::INFINITY);
    job.direction = direction();
    p.layout_job(job)
}

/// The banner atop a first-start window (the browser extension's offer, the video helpers):
/// MDM's logo at the start side, a bold headline, and a muted subline that wraps. 84 px tall, and
/// taller by what a long translation's extra lines need: that extra is returned, for the window
/// to add to its height and to move what is below the banner down by (it is known from the frame
/// before: [`crate::dialogs::grow_extra`] under `key`).
#[allow(clippy::too_many_arguments)]
pub fn banner(
    ui: &Ui,
    p: &Painter,
    o: Pos2,
    w: f32,
    key: &str,
    headline: &str,
    subline: &str,
) -> f32 {
    const PLANNED: f32 = 84.0;
    let extra = crate::dialogs::grow_extra(ui.ctx(), key);
    let h = PLANNED + extra;
    // `r` gives the rect where it is on screen (mirrored in RTL): the logo at the start side.
    p.rect_filled(r(o, 0.0, 0.0, w, h), 0.0, FIELD());
    fill_l(p, o.x, o.y + h, o.x + w - 1.0, o.y + h, FIELD_EDGE());
    egui::Image::new(egui::include_image!("../assets/icons/app/mdm-48.png"))
        .paint_at(ui, r(o, 18.0, 18.0, 48.0, 48.0));
    // The headline on one line: cut with "..." at the far edge (a long translation), whole on
    // hover.
    let bold = tahoma_title();
    let shown = ellipsize(p, &bold, headline, w - 18.0 - 80.0);
    let drawn = text_font(p, bold, o.x + 80.0, o.y + 36.0, &shown, TEXT());
    if shown != headline {
        full_on_hover(ui, drawn, (key, "headline"), headline);
    }
    let lines = paragraph(
        p,
        o.x + 80.0,
        o.y + 57.0,
        subline,
        MUTED(),
        o.x + w - 18.0,
        14.0,
    );
    // As planned, two lines: the second one's baseline at 71, then 13 px of descent and margin.
    let need = 57.0 + 14.0 * (lines.max(2) as f32 - 1.0) + 13.0;
    crate::dialogs::grow_px(ui.ctx(), key, need - PLANNED);
    extra
}

/// A separator dot between two parts of a line ("Germany · about 35 ms"), at English `x`: a
/// round 3 px dot on the middle of the lowercase letters. The font's "·" is a thin tick sitting
/// high at small sizes, which reads as an apostrophe. Its rect on screen.
pub fn separator_dot(p: &Painter, x: f32, baseline: f32, color: Color32) -> Rect {
    let rc = rect(Rect::from_center_size(
        pos2(x + 1.5, baseline - 2.5),
        vec2(3.0, 3.0),
    ));
    p.circle_filled(rc.center(), 1.5, color);
    rc
}

/// Draw `s` with its pen (its start) at English `x` and its baseline (last cap row) at
/// `baseline`. Returns the text's advance rect on screen.
pub fn text(p: &Painter, x: f32, baseline: f32, s: &str, color: Color32) -> Rect {
    let drawn = text_font(p, tahoma(), x, baseline, s, color);
    note_caption(p, s, drawn);
    drawn
}

/// The caption painted last in this window (text, where, which pass): the next text box or
/// drop-down beside or under it takes it as its name for screen readers ([`caption_for`]).
fn caption_key(ctx: &egui::Context) -> egui::Id {
    egui::Id::unique(("last-caption", ctx.viewport_id()))
}

fn note_caption(p: &Painter, s: &str, drawn: Rect) {
    let s = s.trim();
    if s.is_empty() || s.chars().count() > 80 {
        return;
    }
    let ctx = p.ctx();
    // The key first: asking the context inside its lock deadlocks.
    let (key, pass) = (caption_key(ctx), ctx.cumulative_pass_nr());
    ctx.data_mut(|d| d.insert_temp(key, (s.to_owned(), drawn, pass)));
}

/// The caption that names `control` (screen rect): painted this pass just before it, at its
/// leading side on the same row or right above it. Taken once.
fn caption_for(ctx: &egui::Context, control: Rect) -> Option<String> {
    let key = caption_key(ctx);
    let (s, at, pass) = ctx.data(|d| d.get_temp::<(String, Rect, u64)>(key))?;
    if pass != ctx.cumulative_pass_nr() {
        return None;
    }
    // In English (a right-to-left window has its captions at the right).
    let (c, b) = (rect(control), rect(at));
    let same_row = b.right() <= c.left() + 4.0 && b.bottom() > c.top() && b.top() < c.bottom();
    let above = b.bottom() <= c.top() + 3.0 && b.bottom() >= c.top() - 24.0 && b.left() < c.right();
    if !(same_row || above) {
        return None;
    }
    ctx.data_mut(|d| d.remove::<(String, Rect, u64)>(key));
    Some(s.trim_end_matches(':').trim().replace('&', ""))
}

/// A text box's or drop-down's name for screen readers, from its caption ([`caption_for`]).
fn name_from_caption(resp: &Response) {
    if let Some(name) = caption_for(&resp.ctx, resp.rect) {
        resp.ctx
            .accesskit_node_builder(resp.id, |node| node.set_label(name));
    }
}

/// Segoe UI −12: menu bar, toolbar labels.
pub fn segoe() -> FontId {
    FontId::proportional(crate::theme::font_size(crate::theme::metrics().font_ui))
}

/// Arial Bold −13: the Categories close "x".
pub fn arial_bold() -> FontId {
    FontId::new(
        crate::theme::font_size(crate::theme::metrics().font_ui_bold),
        FontFamily::Name("arial-bold".into()),
    )
}

/// Like [`text`] in any font.
pub fn text_font(
    p: &Painter,
    font: FontId,
    x: f32,
    baseline: f32,
    s: &str,
    color: Color32,
) -> Rect {
    let g = galley(p, font, s, color);
    // The text's box in English, then on screen (its start at the mirrored edge in RTL).
    let left = rect(Rect::from_min_size(
        pos2(x.round(), 0.0),
        vec2(g.size().x, 0.0),
    ))
    .left();
    paint_galley(p, left, baseline, g, color)
}

/// Like [`text_font`] with its start at screen x `start` (a widget's inside, measured from its
/// leading edge): the text runs right from it, or left in RTL.
pub fn text_start(
    p: &Painter,
    font: FontId,
    start: f32,
    baseline: f32,
    s: &str,
    color: Color32,
) -> Rect {
    let g = galley(p, font, s, color);
    let left = if rtl() { start - g.size().x } else { start };
    paint_galley(p, left.round(), baseline, g, color)
}

/// Like [`text_font`] with its left at screen x `left` (text centred in a screen rect).
pub fn text_at(
    p: &Painter,
    font: FontId,
    left: f32,
    baseline: f32,
    s: &str,
    color: Color32,
) -> Rect {
    paint_galley(p, left, baseline, galley(p, font, s, color), color)
}

/// Paint `g` with its left at screen x `left` and its baseline (last cap row) at `baseline`.
fn paint_galley(
    p: &Painter,
    left: f32,
    baseline: f32,
    g: std::sync::Arc<egui::Galley>,
    color: Color32,
) -> Rect {
    let asc = g
        .rows
        .first()
        .map(|r| r.pos.y + r.row.glyphs.first().map_or(0.0, |g| g.pos.y))
        .unwrap_or(0.0);
    // The baseline line sits under the last cap row.
    let top = (baseline + 1.0 - asc).round();
    let size = g.size();
    let left = left.round();
    let drawn = Rect::from_min_size(pos2(left, top), size);
    fit_audit(p, drawn, g.text());
    p.galley(pos2(left, top), g, color);
    drawn
}

/// `MDM_FIT_AUDIT=<file>` (a check for translations, beyond IDM): every distinct text drawn past
/// the edge of what its painter may show (the window, a box), and every two texts drawn over each
/// other in one window, are appended to the file, so a run through the windows in each language
/// lists what doesn't fit. Off otherwise (one env lookup).
fn fit_audit(p: &Painter, drawn: Rect, text: &str) {
    use std::collections::{HashMap, HashSet};
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};
    /// What one window (viewport) has drawn in its current pass: (layer, visible rect, text).
    type Pass = (u64, Vec<(egui::LayerId, Rect, String)>);
    /// The audit file, the lines already in it, and each window's texts this pass.
    type Audit = Mutex<(
        std::fs::File,
        HashSet<String>,
        HashMap<egui::ViewportId, Pass>,
    )>;
    static AUDIT: OnceLock<Option<Audit>> = OnceLock::new();
    let Some(audit) = AUDIT.get_or_init(|| {
        let path = std::env::var_os("MDM_FIT_AUDIT")?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        Some(Mutex::new((file, Default::default(), Default::default())))
    }) else {
        return;
    };
    let mut audit = audit.lock().unwrap();
    let (file, seen, passes) = &mut *audit;
    let lang = crate::i18n::code();
    let clip = p.clip_rect();
    if (drawn.left() < clip.left() - 1.0 || drawn.right() > clip.right() + 1.0)
        && seen.insert(text.to_owned())
    {
        let _ = writeln!(
            file,
            "{}\t{:.0}..{:.0} in {:.0}..{:.0}\t{}", // not translated: a check's log line
            lang,
            drawn.left(),
            drawn.right(),
            clip.left(),
            clip.right(),
            text.replace('\n', " / ")
        );
    }
    // Over another text: what shows of each (inside its clip) crosses by more than 2 px across
    // and half a row down (rows a line apart may cross by their padding). The same text twice
    // (an embossed disabled label) is one text.
    let ctx = p.ctx();
    let viewport = ctx.viewport_id();
    let pass_nr = ctx.cumulative_pass_nr_for(viewport);
    let pass = passes.entry(viewport).or_insert((pass_nr, Vec::new()));
    if pass.0 != pass_nr {
        *pass = (pass_nr, Vec::new());
    }
    let shown = drawn.intersect(clip);
    if shown.width() <= 0.0 || shown.height() <= 0.0 {
        return;
    }
    for (layer, other, other_text) in &pass.1 {
        let both = shown.intersect(*other);
        if *layer == p.layer_id()
            && other_text != text
            && both.width() > 2.0
            && both.height() > 0.5 * shown.height().min(other.height())
        {
            let line = format!(
                "{lang}\toverlap\t{} | {}", // not translated: a check's log line
                other_text.replace('\n', " / "),
                text.replace('\n', " / ")
            );
            if seen.insert(line.clone()) {
                let _ = writeln!(file, "{line}");
            }
        }
    }
    pass.1.push((p.layer_id(), shown, text.to_owned()));
}

/// The screen x range of the `n`-th character of `s` drawn in `drawn` (a mnemonic's underline):
/// by the width of what comes before it in a left-to-right interface (as Windows does), by the
/// glyph itself in RTL, where the letters before it are to its right.
pub fn char_span(p: &Painter, font: &FontId, s: &str, n: usize, drawn: Rect) -> (f32, f32) {
    let before: String = s.chars().take(n).collect();
    let ch: String = s.chars().skip(n).take(1).collect();
    let cw = text_width_font(p, font.clone(), &ch);
    if !crate::i18n::rtl() {
        let x0 = drawn.left() + text_width_font(p, font.clone(), &before);
        return (x0, x0 + cw - 1.0);
    }
    // The glyph itself, in the form it takes in the word (a joined Arabic letter is narrower
    // than alone); the row keeps its glyphs in reading order.
    let g = galley(p, font.clone(), s, TEXT());
    let c = ch.chars().next();
    let glyph = g.rows.first().and_then(|r| {
        let k = before.chars().filter(|b| Some(*b) == c).count();
        r.row.glyphs.iter().filter(|gl| Some(gl.chr) == c).nth(k)
    });
    match glyph {
        Some(gl) => {
            let x0 = drawn.left() + gl.pos.x;
            (x0, x0 + gl.advance_width.round().max(1.0) - 1.0)
        }
        None => (drawn.left(), drawn.left() + cw - 1.0),
    }
}

pub fn text_width(p: &Painter, s: &str) -> f32 {
    text_width_font(p, tahoma(), s)
}

pub fn text_width_font(p: &Painter, font: FontId, s: &str) -> f32 {
    p.layout_no_wrap(s.to_owned(), font, TEXT()).size().x
}

/// Where the fields of a column of labelled rows start (offset from the dialog's left), for
/// labels drawn at `label_x`: `field_x` as laid out for English, or further right when a
/// translated label is longer, so no label runs under its field. Every row shares it.
pub fn label_column(p: &Painter, label_x: f32, labels: &[&str], field_x: f32) -> f32 {
    let widest = labels
        .iter()
        .map(|l| text_width(p, l))
        .fold(0.0_f32, f32::max);
    field_x.max((label_x + widest + 6.0).ceil())
}

/// `s` cut with "..." (three periods, as Windows' DT_END_ELLIPSIS) to fit `max` px.
pub fn ellipsize(p: &Painter, font: &FontId, s: &str, max: f32) -> String {
    if text_width_font(p, font.clone(), s) <= max {
        return s.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    let cut = |n: usize| chars[..n].iter().collect::<String>() + "...";
    // The longest prefix that fits, by binary search (widths grow with the prefix).
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if text_width_font(p, font.clone(), &cut(mid)) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    cut(lo)
}

/// `s` cut with "..." in the middle to fit `max` px (Windows' DT_PATH_ELLIPSIS): for a path or
/// an address, whose start (the folder) and end (the file's name) both matter. The end keeps
/// three parts in five of what fits.
pub fn ellipsize_middle(p: &Painter, font: &FontId, s: &str, max: f32) -> String {
    if text_width_font(p, font.clone(), s) <= max {
        return s.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    let cut = |n: usize| {
        let head = n * 2 / 5;
        let tail = n - head;
        chars[..head].iter().collect::<String>()
            + "..."
            + &chars[chars.len() - tail..].iter().collect::<String>()
    };
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if text_width_font(p, font.clone(), &cut(mid)) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    cut(lo)
}

/// [`text`] on one line that never runs past `right`: cut with "..." when too long, line breaks
/// shown as spaces. Returns the drawn rect and whether the text was cut (then show it whole with
/// [`full_on_hover`]).
pub fn text_fit(
    p: &Painter,
    x: f32,
    baseline: f32,
    s: &str,
    color: Color32,
    right: f32,
) -> (Rect, bool) {
    // A one-line label, though the text may come from elsewhere (a site's title, an error): a
    // line break is a space here, not another row drawn down over the window. Cut means the
    // whole original, line breaks and all, is shown on hover.
    let flat;
    let line = if s.contains(['\n', '\r']) {
        flat = s.replace("\r\n", " ").replace(['\n', '\r'], " ");
        flat.as_str()
    } else {
        s
    };
    let shown = ellipsize(p, &tahoma(), line, (right - x).max(0.0));
    let cut = shown != s;
    (text(p, x, baseline, &shown, color), cut)
}

/// [`text`] that never runs past the window's far edge: cut with "..." and shown whole on hover.
pub fn text_in_window(
    ui: &Ui,
    p: &Painter,
    x: f32,
    baseline: f32,
    s: &str,
    color: Color32,
) -> Rect {
    let (rc, cut) = text_fit(p, x, baseline, s, color, window_edges(p).1);
    if cut {
        full_on_hover(ui, rc, ("in-window", s), s);
    }
    rc
}

/// A label that leads into a control whose box starts at English `before`: at its pen `x` (IDM's
/// place) when it fits, else ending 5 px before the box and starting earlier, down to `min_x`;
/// cut with "..." only past that. Returns the drawn rect and whether it was cut.
pub fn label_into(
    p: &Painter,
    x: f32,
    baseline: f32,
    s: &str,
    color: Color32,
    before: f32,
    min_x: f32,
) -> (Rect, bool) {
    let end = before - 5.0;
    let w = text_width(p, s);
    if x + w <= end {
        return (text(p, x, baseline, s, color), false);
    }
    let start = (end - w).max(min_x);
    text_fit(p, start, baseline, s, color, end)
}

/// `s` with its single line breaks joined (a space, or nothing between letters of scripts
/// written without spaces: Chinese, Japanese, Thai…), blank lines kept as paragraph breaks.
fn reflow(s: &str) -> String {
    let spaceless = |c: char| {
        matches!(c as u32,
            0x0E00..=0x0EFF   // Thai, Lao
            | 0x1780..=0x17FF // Khmer
            | 0x3040..=0x30FF // Hiragana, Katakana
            | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF // Han
            | 0x3000..=0x303F | 0xFF00..=0xFFEF) // CJK punctuation, full-width forms
    };
    s.split("\n\n")
        .map(|para| {
            let mut out = String::new();
            for (k, line) in para.split('\n').enumerate() {
                let line = line.trim();
                if k > 0 && !out.is_empty() && !line.is_empty() {
                    let before = out.chars().last().is_some_and(spaceless);
                    let after = line.chars().next().is_some_and(spaceless);
                    if !(before && after) {
                        out.push(' ');
                    }
                }
                out.push_str(line);
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod reflow_tests {
    #[test]
    fn hard_breaks_flow_again() {
        assert_eq!(
            super::reflow("one two\nthree\n\nnew"),
            "one two three\n\nnew"
        );
        assert_eq!(super::reflow("日本語の\n文章"), "日本語の文章");
    }
}

/// Chinese and Japanese (Han, kana, their punctuation and full-width forms): broken between any
/// two characters.
fn cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
        | 0x3000..=0x303F | 0xFF00..=0xFFEF)
}

/// Closing punctuation that a line broken between characters must not start with.
fn no_line_start(c: char) -> bool {
    "、。，．・：；？！ー）」』】〕｝〉》々ゝゞぁぃぅぇぉっゃゅょァィゥェォッャュョ,.:;?!)]}%"
        .contains(c)
}

/// A Windows 11 progress bar in `rc` (a few pixels tall): a thin track and the accent filled to
/// `fraction`; while the total is unknown (`None`) a segment runs along it (`time`: seconds,
/// for its place).
pub fn progress_bar(p: &Painter, rc: Rect, fraction: Option<f32>, time: f64) {
    let c = crate::theme::colors();
    // The era themes: XP's blocks, the glossy bars of Windows 7 and Mac OS X.
    let (from, to) = match fraction {
        Some(f) => (0.0, f.clamp(0.0, 1.0)),
        None => {
            let t = (time * 0.6).fract() as f32;
            ((t * 1.4 - 0.4).max(0.0), (t * 1.4).min(1.0))
        }
    };
    if era_bar(p, rect(rc), from, to, c.progress_bar, rtl()) {
        return;
    }
    // Windows 8: a bordered bar, filled.
    if crate::theme::current().controls.progress == crate::palette::ProgressLook::Bar {
        let border = crate::theme::metrics().border;
        p.rect_filled(rect(rc), 0.0, c.meter_track);
        let inner = rc.shrink(border);
        if to > from {
            let w = inner.width();
            p.rect_filled(
                rect(Rect::from_min_max(
                    pos2(inner.left() + from * w, inner.top()),
                    pos2(inner.left() + to * w, inner.bottom()),
                )),
                0.0,
                c.progress_bar,
            );
        }
        p.rect_stroke(
            rect(rc),
            0.0,
            Stroke::new(border, c.meter_edge),
            StrokeKind::Inside,
        );
        return;
    }
    let (track, accent) = (c.progress_track, c.progress_bar);
    let r = rc.height() / 2.0;
    // The track is a 1 px line through the middle, as Windows 11 draws it.
    let mid = rc.center().y;
    p.rect_filled(
        rect(Rect::from_min_max(
            pos2(rc.left(), mid - 0.5),
            pos2(rc.right(), mid + 0.5),
        )),
        0.5,
        track,
    );
    if to > from {
        let w = rc.width();
        p.rect_filled(
            rect(Rect::from_min_max(
                pos2(rc.left() + from * w, rc.top()),
                pos2(rc.left() + to * w, rc.bottom()),
            )),
            r,
            accent,
        );
    }
}

/// Read-only data that scrolls inside `area` (English coordinates, the box's own frame drawn by
/// the caller): the wheel or a trackpad over it, its scroll bar when the text is longer than the
/// box, and with `keys` the window's Up, Down, Page Up, Page Down, Home and End. Where it is
/// scrolled to is kept under `id`. The text is data (a crash report, release notes): its line
/// breaks are kept and it reads left to right in every language ([`data_paragraph`]).
#[allow(clippy::too_many_arguments)]
pub fn scroll_text(
    ui: &mut Ui,
    id: egui::Id,
    area: Rect,
    s: &str,
    color: Color32,
    line_h: f32,
    keys: bool,
) {
    use crate::scrollbar;
    const PAD: f32 = 6.0;
    let p = ui.painter().clone();
    let visible = ((area.height() - 6.0) / line_h).floor().max(1.0);
    // Laid out first where nothing shows: the bar takes room only when the text needs it.
    let measure = p.with_clip_rect(Rect::NOTHING);
    let lines =
        |right: f32| data_paragraph(&measure, area.left() + PAD, 0.0, s, color, right, line_h);
    let mut total = lines(area.right() - PAD) as f32;
    let bar = total > visible;
    let text_right = if bar {
        let right = area.right() - 1.0 - scrollbar::THICK - 3.0;
        total = lines(right) as f32;
        right
    } else {
        area.right() - PAD
    };
    let max_top = (total - visible).max(0.0);
    let mut top: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
    // The wheel scrolls the system's lines a notch (three unless changed), as text boxes do.
    let area_resp = ui.interact(rect(area), id.with("area"), pointer_only());
    if area_resp.hovered() {
        top += wheel_rows(ui, line_h, visible);
    }
    // The whole text to the clipboard from its right-click menu.
    copy_menu(&area_resp, s);
    if keys {
        let page = (visible - 1.0).max(1.0);
        ui.input(|i| {
            for (key, by) in [
                (egui::Key::ArrowUp, -1.0),
                (egui::Key::ArrowDown, 1.0),
                (egui::Key::PageUp, -page),
                (egui::Key::PageDown, page),
                (egui::Key::Home, -total),
                (egui::Key::End, total),
            ] {
                if i.key_pressed(key) {
                    top += by;
                }
            }
        });
    }
    top = top.clamp(0.0, max_top);
    // Moved by the wheel or a key: draw the new place now, not at the next event.
    if ui
        .data(|d| d.get_temp::<f32>(id))
        .is_some_and(|was| was != top)
    {
        ui.ctx().request_repaint();
    }
    let clip = p.with_clip_rect(rect(area.shrink(2.0)));
    data_paragraph(
        &clip,
        area.left() + PAD,
        area.top() + 16.0 - top * line_h,
        s,
        color,
        text_right,
        line_h,
    );
    if bar {
        let track = rect(Rect::from_min_max(
            pos2(area.right() - 1.0 - scrollbar::THICK, area.top() + 1.0),
            pos2(area.right() - 1.0, area.bottom() - 1.0),
        ));
        if scrollbar::bar(
            ui,
            id.with("bar"),
            track,
            true,
            total,
            visible,
            1.0,
            &mut top,
        ) {
            ui.ctx().request_repaint();
        }
    }
    ui.data_mut(|d| d.insert_temp(id, top));
}

/// A paragraph from `x` to `right`, first baseline at `baseline`, `line_h` apart; returns the
/// lines drawn. English keeps its line breaks (broken where IDM breaks it); a translation's
/// copy of them is flowed again (`reflow`). A line too long for the room wraps at spaces, or
/// between any two characters in scripts written without spaces (Chinese, Japanese, Thai…).
/// (Data, whose line breaks are its own: [`data_paragraph`].)
pub fn paragraph(
    p: &Painter,
    x: f32,
    baseline: f32,
    s: &str,
    color: Color32,
    right: f32,
    line_h: f32,
) -> usize {
    // IDM's English is hard-wrapped; a translation's copy of those breaks is flowed again (a
    // blank line still starts a new paragraph). English keeps its own lines.
    let flowed;
    let s = if crate::i18n::code() == "en" {
        s
    } else {
        flowed = reflow(s);
        flowed.as_str()
    };
    let lines = wrap_lines(p, s, (right - x).max(1.0));
    for (i, l) in lines.iter().enumerate() {
        text(p, x, baseline + line_h * i as f32, l, color);
    }
    lines.len()
}

/// Data shown as it is (a crash report, release notes), from English `x` to `right`, first
/// baseline at `baseline`, `line_h` apart; returns the lines drawn. Its line breaks are its own
/// in every language (a line longer than the room wraps, as in [`paragraph`]), and it reads left
/// to right from the left edge of its room, inside a right-to-left window too: data keeps its
/// direction there (the RTL notes above).
pub fn data_paragraph(
    p: &Painter,
    x: f32,
    baseline: f32,
    s: &str,
    color: Color32,
    right: f32,
    line_h: f32,
) -> usize {
    let lines = wrap_lines(p, s, (right - x).max(1.0));
    let left = data_left(x, right);
    for (i, l) in lines.iter().enumerate() {
        let mut job = egui::text::LayoutJob::simple(l.clone(), tahoma(), color, f32::INFINITY);
        job.direction = egui::epaint::text::TextDirection::LeftToRight;
        paint_galley(
            p,
            left,
            baseline + line_h * i as f32,
            p.layout_job(job),
            color,
        );
    }
    lines.len()
}

/// Where data from English `x` to `right` starts on screen: its room's left edge (the mirror of
/// its English right edge in RTL).
fn data_left(x: f32, right: f32) -> f32 {
    rect(Rect::from_min_max(pos2(x, 0.0), pos2(right, 0.0)))
        .left()
        .round()
}

/// `s` broken into the lines it shows in `room` px: at its own line breaks, and where a line is
/// too long, at spaces, or between any two characters in scripts written without spaces.
fn wrap_lines(p: &Painter, s: &str, room: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for given in s.split('\n') {
        // A Windows line end (a report written there) breaks the line too.
        let given = given.strip_suffix('\r').unwrap_or(given);
        let mut line = String::new();
        for word in given.split_inclusive(' ') {
            let candidate = format!("{line}{word}");
            if line.is_empty()
                || text_width(p, candidate.trim_end()) <= room
                // Chinese and Japanese fill the line: broken between characters below.
                || word.chars().next().is_some_and(cjk)
            {
                line = candidate;
            } else {
                lines.push(std::mem::take(&mut line).trim_end().to_owned());
                line = word.to_owned();
            }
            // Wider than the room (a script without spaces), wherever it stands in the line:
            // break between characters, a letter and its marks kept together.
            while text_width(p, line.trim_end()) > room {
                let g: Vec<&str> = line.graphemes(true).collect();
                let mut n = 0;
                let mut w = String::new();
                for (k, gr) in g.iter().enumerate() {
                    w.push_str(gr);
                    if text_width(p, &w) > room {
                        break;
                    }
                    n = k + 1;
                }
                let latin = |k: usize| g[k].chars().all(|c| c.is_ascii_alphanumeric());
                // Not inside a Latin word ("Mister Download Manager" in Japanese): before it.
                if n > 0
                    && n < g.len()
                    && latin(n - 1)
                    && latin(n)
                    && let Some(space) = g[..n].iter().rposition(|gr| *gr == " ")
                {
                    n = space + 1;
                }
                // A closing mark never starts a line (Japanese kinsoku): the character before
                // it goes down with it.
                if n > 1 && g.get(n).is_some_and(|gr| gr.chars().all(no_line_start)) {
                    n -= 1;
                }
                if n == 0 {
                    break;
                }
                lines.push(g[..n].concat().trim_end().to_owned());
                line = g[n..].concat().trim_start().to_owned();
            }
        }
        lines.push(line.trim_end().to_owned());
    }
    lines
}

/// Where the text before a line's first input box goes in a [`sentence`].
#[derive(Clone, Copy, Debug)]
pub enum Lead {
    /// Starts at this x; the box moves right when the text is too long for the room.
    Left(f32),
    /// Ends at this x (a column of labels right-aligned to their boxes); longer text grows left.
    Right(f32),
}

/// One line of a [`sentence`], placed as the English has it.
#[derive(Clone, Copy, Debug)]
pub struct SentenceLine {
    pub lead: Lead,
    pub lead_baseline: f32,
    /// The top-left of the line's first box.
    pub boxes: Pos2,
    /// From a box to the text after it.
    pub gap: f32,
    /// The baseline of the text after the first box.
    pub baseline: f32,
}

/// A piece of a [`sentence`] template.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece<'a> {
    Text(&'a str),
    /// A `{name}` that is one of the sentence's input boxes.
    Slot(&'a str),
}

/// `template`'s lines (`\n`), each split at the `{name}` placeholders that are in `slots`;
/// text pieces trimmed (the layout spaces them), empty ones dropped.
pub fn sentence_pieces<'a>(template: &'a str, slots: &[&str]) -> Vec<Vec<Piece<'a>>> {
    template
        .split('\n')
        .map(|line| {
            let mut out = Vec::new();
            let (mut from, mut at) = (0, 0);
            while let Some(open) = line[at..].find('{').map(|i| at + i) {
                let Some(close) = line[open..].find('}').map(|i| open + i) else {
                    break;
                };
                let name = &line[open + 1..close];
                if slots.contains(&name) {
                    out.push(Piece::Text(line[from..open].trim()));
                    out.push(Piece::Slot(name));
                    from = close + 1;
                }
                at = close + 1;
            }
            out.push(Piece::Text(line[from..].trim()));
            out.retain(|p| *p != Piece::Text(""));
            out
        })
        .collect()
}

/// Where [`sentence`] puts things: (x, baseline, text) for each text piece, and each slot's
/// rect in `slots` order. `width` measures a text. A right-aligned lead starts no further out
/// than `near` (the window's edge); the box after it moves on instead.
pub fn sentence_layout(
    template: &str,
    lines: &[SentenceLine],
    slots: &[(&str, egui::Vec2)],
    width: impl Fn(&str) -> f32,
    near: f32,
) -> (Vec<(f32, f32, String)>, Vec<Rect>) {
    let names: Vec<&str> = slots.iter().map(|(n, _)| *n).collect();
    let mut rows = sentence_pieces(template, &names);
    // More lines than the English has: the rest joins the last one.
    if rows.len() > lines.len() && !lines.is_empty() {
        let extra: Vec<Piece> = rows.drain(lines.len()..).flatten().collect();
        rows[lines.len() - 1].extend(extra);
    }
    let mut texts = Vec::new();
    let mut rects: Vec<Option<Rect>> = vec![None; slots.len()];
    // Where the last line ends: (x, box top, gap).
    let mut end = (0.0, 0.0, 8.0);
    for (row, spec) in rows.iter().zip(lines) {
        let mut x = None; // the next x after a box (or text after one)
        let mut first_box = true;
        let mut lead_end = None;
        for piece in row {
            match *piece {
                Piece::Text(s) => {
                    let w = width(s);
                    let (tx, base) = match x {
                        Some(x) => (x, spec.baseline),
                        None => match spec.lead {
                            Lead::Left(l) => (l, spec.lead_baseline),
                            Lead::Right(r) => ((r - w).max(near), spec.lead_baseline),
                        },
                    };
                    if x.is_none() {
                        lead_end = Some(tx + w);
                    }
                    texts.push((tx, base, s.to_owned()));
                    x = Some(tx + w + spec.gap);
                }
                Piece::Slot(name) => {
                    let i = names.iter().position(|n| *n == name).unwrap_or(0);
                    let size = slots[i].1;
                    let left = if first_box {
                        // Room for the lead text, at least 3 px before the box (a right-aligned
                        // one only needs it when it reached `near`).
                        lead_end.map_or(spec.boxes.x, |e| spec.boxes.x.max(e + 3.0))
                    } else {
                        x.unwrap_or(spec.boxes.x)
                    };
                    first_box = false;
                    let rc = Rect::from_min_size(pos2(left, spec.boxes.y), size);
                    if rects[i].is_none() {
                        rects[i] = Some(rc);
                    }
                    x = Some(rc.right() + spec.gap);
                }
            }
        }
        end = (x.unwrap_or(spec.boxes.x), spec.boxes.y, spec.gap);
    }
    // A box the translation left out still gets a place: after the last line.
    let rects = rects
        .into_iter()
        .zip(slots)
        .map(|(rc, (_, size))| {
            rc.unwrap_or_else(|| {
                let rc = Rect::from_min_size(pos2(end.0, end.1), *size);
                end.0 = rc.right() + end.2;
                rc
            })
        })
        .collect();
    (texts, rects)
}

/// A translated sentence with input boxes in it ("Download {files} files at the same time"):
/// the text pieces drawn with `draw(painter, x, baseline, text)` around the boxes, in the
/// translation's order, so a translator can move a box. `lines` place each line as the
/// English has it (one entry per `\n`-separated line); `slots` name the boxes and their sizes.
/// Returns each box's rect on screen, in `slots` order, for the caller to draw its edit there.
/// `draw` gets English pens (draw with [`text`]); `lines` are English too: in RTL the sentence
/// runs from the right, its first piece at the mirrored lead. Nothing starts or ends outside the
/// window: a text that would run past its far edge is cut with "..." and shown whole on hover.
pub fn sentence(
    ui: &Ui,
    p: &Painter,
    template: &str,
    lines: &[SentenceLine],
    slots: &[(&str, egui::Vec2)],
    mut draw: impl FnMut(&Painter, f32, f32, &str),
) -> Vec<Rect> {
    let (near, far) = window_edges(p);
    let (texts, rects) = sentence_layout(template, lines, slots, |s| text_width(p, s), near);
    for (i, (x, baseline, s)) in texts.iter().enumerate() {
        let shown = ellipsize(p, &tahoma(), s, (far - x).max(0.0));
        draw(p, *x, *baseline, &shown);
        if shown != *s {
            let w = text_width(p, &shown);
            let at = rect(Rect::from_min_max(
                pos2(*x, baseline - 11.0),
                pos2(x + w, baseline + 3.0),
            ));
            full_on_hover(ui, at, ("sentence", template, i), s);
        }
    }
    rects.into_iter().map(rect).collect()
}

/// The English x of the window's near and far edges, 6 px in: where a text may start and end.
fn window_edges(p: &Painter) -> (f32, f32) {
    let clip = p.clip_rect();
    if rtl() {
        (mx(clip.right()) + 6.0, mx(clip.left()) - 6.0)
    } else {
        (clip.left() + 6.0, clip.right() - 6.0)
    }
}

/// A tooltip with the whole of a cut text while the pointer is over it (as Windows shows
/// for truncated list items).
pub fn full_on_hover(
    ui: &Ui,
    rect: Rect,
    salt: impl std::hash::Hash + std::fmt::Debug,
    full: &str,
) {
    let id = ui.scope_id().with(("full-text", salt));
    ui.interact(rect, id, egui::Sense::hover())
        .on_hover_text(full);
}

// ---- row list -------------------------------------------------------------------------------

/// Where a [`row_list`] is: its first row shown and its selected row (kept by the caller).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RowList {
    pub first: usize,
    pub selected: Option<usize>,
    /// What a screen reader calls the list ("Quality", "Files"), translated: set by the window
    /// that owns it. Empty: unnamed (a caption near the list is often a column title).
    pub name: &'static str,
    /// The selection was set from outside (typing, the caller): scrolled into view next draw.
    pending: bool,
    /// Wheel travel not yet a whole row.
    wheel: f32,
}

impl RowList {
    /// Typing selects the row whose `label` starts with what was typed ([`typed_match`]);
    /// call after [`row_list`] while it has the keys (`RowEvents::focused`).
    pub fn type_to(
        &mut self,
        ui: &Ui,
        list: &str,
        n: usize,
        label: impl Fn(usize) -> String,
    ) -> bool {
        let key = widget_id(ui, ("row-list-typed", list));
        match typed_match(ui, key, n, self.selected, label) {
            Some(i) => {
                self.selected = Some(i);
                self.pending = true;
                true
            }
            None => false,
        }
    }

    /// Scroll so row `i` shows (whole), `page` rows being visible.
    pub fn show_row(&mut self, i: usize, page: usize) {
        if i < self.first {
            self.first = i;
        } else if i >= self.first + page {
            self.first = i + 1 - page;
        }
    }
}

/// What happened in a [`row_list`] this frame (row indexes).
#[derive(Default, Debug)]
pub struct RowEvents {
    /// The selection moved to this row (a click, the keys).
    pub picked: Option<usize>,
    pub double_clicked: Option<usize>,
    /// Enter on the selected row (not taken: [`take_key`] it to keep it from the dialog).
    pub entered: Option<usize>,
    /// Space on the selected row (a list of check boxes ticks it).
    pub space: Option<usize>,
    /// Delete on the selected row.
    pub delete: Option<usize>,
    /// Right-click on a row (selected first), at this screen point.
    pub context: Option<(usize, Pos2)>,
    /// The list has the keys (a tree adds Left / Right).
    pub focused: bool,
}

/// How [`row_list`] drew a row's background: the row's text goes in `ink`.
pub struct RowLook {
    pub selected: bool,
    pub ink: Color32,
    /// What a screen reader says for the row: the row's painter sets it (its text, e.g. a
    /// file's name and size). Left empty, the row is read without a name.
    pub name: String,
}

/// Rows of a virtual list a screen reader reaches (#111): every row of a list up to
/// `A11Y_ROWS`, else that many around the view (`first`, `page` rows), so a list of thousands
/// costs no more than a short one. Further rows come within reach as the list scrolls (the keys
/// scroll it, as a list view's own).
pub fn reachable_rows(n: usize, first: usize, page: usize) -> std::ops::Range<usize> {
    if n <= A11Y_ROWS {
        return 0..n;
    }
    let before = A11Y_ROWS.saturating_sub(page) / 2;
    let start = first.saturating_sub(before).min(n - A11Y_ROWS);
    start..start + A11Y_ROWS
}

/// How many rows of a list a screen reader reaches at most ([`reachable_rows`]).
pub const A11Y_ROWS: usize = 300;

/// The row of `rows` (widget ids `id_of(i)`) a screen reader asked to scroll into view, or to
/// select (AccessKit's click), this frame.
pub fn row_asked_for(
    ui: &Ui,
    rows: std::ops::Range<usize>,
    id_of: impl Fn(usize) -> egui::Id,
) -> Option<usize> {
    use egui::accesskit::Action;
    let targets: Vec<egui::accesskit::NodeId> = ui.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::AccessKitActionRequest(r)
                    if matches!(r.action, Action::ScrollIntoView | Action::Click) =>
                {
                    Some(r.target_node)
                }
                _ => None,
            })
            .collect()
    });
    if targets.is_empty() {
        return None;
    }
    rows.into_iter()
        .find(|i| targets.contains(&id_of(*i).accesskit_id()))
}

/// The one list of whole rows in a box (a dialog's list, a tree drawn as rows): its frame,
/// rows scrolled whole by the wheel (travel kept until it makes a row) and by a scroll bar
/// when they don't all fit, the row under the pointer tinted, the selected row in the
/// selection colour (grey while the list hasn't the keys), and the keys while it has them:
/// Up, Down, Home, End, Page Up, Page Down move the selection; Enter, Space and Delete are
/// reported. `rc` is where the box is on screen ([`r`]); `paint` draws row `i`'s contents in
/// its English rect, after its background, and may put controls in it (they take their own
/// clicks).
pub fn row_list(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    rc: Rect,
    n: usize,
    row_h: f32,
    st: &mut RowList,
    mut paint: impl FnMut(&mut Ui, &Painter, usize, Rect, &mut RowLook),
) -> RowEvents {
    let mut ev = RowEvents::default();
    let id = widget_id(ui, ("row-list", id));
    let p = ui.painter().clone();
    solid(&p, rc, FIELD_EDGE());
    p.rect_filled(rc.shrink(1.0), 0.0, FIELD());
    // Laid out in English, made where it is on screen.
    let inner = rect(rc).shrink(1.0);
    let page = (((inner.height() - 2.0) / row_h).floor() as usize).max(1);
    let max_first = n.saturating_sub(page);
    let bar = n > page;
    let right = inner.right() - if bar { crate::scrollbar::THICK } else { 0.0 };
    let area = Rect::from_min_max(inner.min, pos2(right, inner.bottom()));
    let resp = ui.interact(rect(inner), id, Sense::click());
    // Named by its owner (#95), not by a caption found near it: that is often a column title
    // ("Size"), and a wrong name is worse than none.
    expose(&resp, egui::Role::ListBox, st.name, true, None);
    // Its arrows (a tree's Left / Right too) stay with it.
    keep_keys(ui, &resp, true, true, false);
    let focused = resp.has_focus();
    // The wheel: whole rows, the rest kept for the next turn of it.
    if ui.rect_contains_pointer(rect(inner)) {
        st.wheel += wheel_rows(ui, row_h, page as f32);
        let whole = st.wheel.trunc();
        st.wheel -= whole;
        st.first = (st.first as f32 + whole).max(0.0) as usize;
    }
    if focused && n > 0 {
        let at = st.selected.map_or(-1, |s| s as isize);
        let last = n as isize - 1;
        let step = page as isize;
        let keys = [
            (egui::Key::ArrowDown, at + 1),
            (egui::Key::ArrowUp, (at - 1).max(0)),
            (egui::Key::Home, 0),
            (egui::Key::End, last),
            // As Explorer: first to the last (first) row in view, then a page on.
            (
                egui::Key::PageDown,
                if at < (st.first + page) as isize - 1 {
                    (st.first + page) as isize - 1
                } else {
                    at + step
                },
            ),
            (
                egui::Key::PageUp,
                if at > st.first as isize {
                    st.first as isize
                } else {
                    at - step
                },
            ),
        ];
        for (key, to) in keys {
            if take_key(ui, key) {
                let to = to.clamp(0, last) as usize;
                st.selected = Some(to);
                st.show_row(to, page);
                ev.picked = Some(to);
            }
        }
        if let Some(s) = st.selected {
            // Reported, not taken: a list that ignores it leaves Enter to the dialog's
            // default button (a caller that acts on it takes it).
            if ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.is_none()) {
                ev.entered = Some(s);
            }
            if take_key(ui, egui::Key::Space) {
                ev.space = Some(s);
            }
            if take_key(ui, egui::Key::Delete) {
                ev.delete = Some(s);
            }
        }
    }
    if std::mem::take(&mut st.pending)
        && let Some(s) = st.selected
    {
        st.show_row(s, page);
    }
    // Rows out of view are there for screen readers too (#111): those `reachable_rows` gives
    // (every row of a list up to a few hundred, else that many around the view) while a
    // screen reader is on. A row brought into view by one (Scroll into view, or Select, which
    // AccessKit sends as a click) shows from this frame on.
    // The number of rows is the list's (UI Automation reads a row's "of M" from its list).
    let a11y = ui
        .ctx()
        .accesskit_node_builder(resp.id, |node| node.set_size_of_set(n))
        .is_some();
    let reach = if a11y {
        reachable_rows(n, st.first, page)
    } else {
        st.first..n.min(st.first + page)
    };
    if a11y && let Some(i) = row_asked_for(ui, reach.clone(), |i| id.with(i)) {
        st.show_row(i, page);
    }
    st.first = st.first.min(max_first);
    ev.focused = focused;
    let clip = p.with_clip_rect(rect(area));
    // The rows (and what their painters put in them: a tree's check boxes) are the list's
    // children in the accessibility tree, not its siblings (#102): a screen reader then says
    // "4 of 12", and UI Automation's Selection finds them under the list.
    let mut rows_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect(inner))
            .accessibility_parent(resp.id),
    );
    // Rows out of view lie above or below it, clipped: nothing of them shows or takes a click.
    rows_ui.set_clip_rect(rect(area).intersect(ui.clip_rect()));
    let shown = st.first..n.min(st.first + page);
    for i in reach {
        let in_view = shown.contains(&i);
        let top = inner.top() + 1.0 + row_h * (i as f32 - st.first as f32);
        let row = Rect::from_min_max(pos2(inner.left(), top), pos2(right, top + row_h));
        let r = rows_ui.interact(rect(row), id.with(i), pointer_only());
        let selected = st.selected == Some(i);
        let fill = if !in_view {
            None
        } else if selected && focused {
            Some(ROW_SELECTED())
        } else if selected {
            Some(SELECTED_INACTIVE())
        } else {
            // Hot, and pressed lighter still on a dark field (subtle_fill's pressed grey was the
            // dark field's own).
            let c = crate::theme::colors();
            match State::of(&r, true) {
                State::Pressed => Some(c.row_pressed),
                state => subtle_fill(state, c),
            }
        };
        if let Some(f) = fill {
            clip.rect_filled(rect(row), 0.0, f);
        }
        if r.clicked() || r.secondary_clicked() || r.double_clicked() {
            resp.request_focus();
            if st.selected != Some(i) {
                st.selected = Some(i);
                ev.picked = Some(i);
                // Selected out of view (a screen reader's Select): brought into view.
                st.pending |= !in_view;
                // This frame drew the old selection; draw (and expose) the new one now, not on
                // the next mouse move: a screen reader's click moves no mouse.
                ui.ctx().request_repaint();
            }
        }
        if r.double_clicked() {
            ev.double_clicked = Some(i);
        }
        if r.secondary_clicked()
            && let Some(at) = r.interact_pointer_pos()
        {
            ev.context = Some((i, at));
        }
        let mut look = RowLook {
            selected,
            ink: if selected && focused {
                SELECTION_TEXT()
            } else {
                TEXT()
            },
            name: String::new(),
        };
        // What the painter puts in the row (a tree's check box) belongs to the row: the row's
        // node made first (named below, once its painter has said what it shows), so those
        // controls find it as their parent.
        ui.ctx().accesskit_node_builder(r.id, |_| {});
        let mut row_ui = rows_ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect(row))
                .accessibility_parent(r.id),
        );
        paint(&mut row_ui, &clip, i, row, &mut look);
        // Named by what its painter drew (#95: rows were read as blank items, and alike rows
        // couldn't be told apart by UI Automation).
        expose(&r, egui::Role::ListBoxOption, &look.name, true, None);
        // Selected as a list's item (UI Automation's SelectionItem), as the download list does;
        // expose's `on` would make it "toggled".
        // Its place among all the rows, not only those drawn (only the rows in view exist).
        ui.ctx().accesskit_node_builder(r.id, |node| {
            node.set_selected(selected);
            // AccessKit counts from 0 (UI Automation and AT-SPI say i + 1).
            node.set_position_in_set(i);
            node.set_size_of_set(n);
            node.add_action(egui::accesskit::Action::ScrollIntoView);
        });
        if selected && focused && in_view {
            focus_ring(&clip, rect(row));
        }
    }
    if focused
        && st
            .selected
            .is_none_or(|s| s < st.first || s >= st.first + page)
    {
        focus_ring(&p, rect(area));
    }
    if bar {
        let track = Rect::from_min_max(pos2(right, inner.top()), inner.max);
        let mut pos = st.first as f32;
        if crate::scrollbar::bar(
            ui,
            id.with("bar"),
            rect(track),
            true,
            n as f32,
            page as f32,
            1.0,
            &mut pos,
        ) {
            st.first = (pos.round().max(0.0) as usize).min(max_first);
        }
    }
    ev
}

/// Text that opens or does something when clicked: link colour, underlined under the pointer.
/// `label` is English (translated here).
pub fn link(ui: &Ui, p: &Painter, x: f32, baseline: f32, label: &str, salt: &str) -> bool {
    let rect = text(p, x, baseline, crate::i18n::t(label), LINK());
    let resp = ui
        .interact(rect, ui.scope_id().with(("link", salt)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    expose(&resp, egui::Role::Link, crate::i18n::t(label), true, None);
    // A Tab stop shows that it has the keys.
    show_focus(ui, &resp, rect.expand(2.0));
    if resp.hovered() {
        let y = rect.bottom() + 0.5;
        p.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            egui::Stroke::new(1.0, LINK()),
        );
    }
    resp.clicked()
}

/// A note (`s`, translated already) from `x` to `right` on at most two lines `line_h` apart,
/// first baseline `baseline`, with a [`link`] after it (`label`, English, translated here): on
/// the note's line when they fit together, else the note wraps and the link ends its second
/// line. A note longer than that is cut, and shown whole on hover. True when the link is
/// clicked; `salt` tells notes apart.
#[allow(clippy::too_many_arguments)]
pub fn note_with_link(
    ui: &Ui,
    p: &Painter,
    x: f32,
    baseline: f32,
    line_h: f32,
    s: &str,
    color: Color32,
    right: f32,
    label: &str,
    salt: &str,
) -> bool {
    const GAP: f32 = 8.0;
    let link_w = text_width(p, crate::i18n::t(label));
    if text_width(p, s) + GAP + link_w <= right - x {
        text(p, x, baseline, s, color);
        return link(ui, p, x + text_width(p, s) + GAP, baseline, label, salt);
    }
    let lines = wrap_lines(p, s, (right - x).max(1.0));
    let first = lines.first().map_or("", String::as_str);
    let first_rect = text(p, x, baseline, first, color);
    let rest = lines.get(1..).unwrap_or_default().join(" ");
    let second = baseline + line_h;
    let (rest_rect, cut) = text_fit(p, x, second, &rest, color, right - GAP - link_w);
    if cut {
        full_on_hover(ui, first_rect.union(rest_rect), (salt, "note"), s);
    }
    let rest_w = text_width(p, &rest).min(right - GAP - link_w - x);
    link(ui, p, x + rest_w + GAP, second, label, salt)
}

/// A disabled control says why on hover: `reason` (English, translated here) while `disabled`.
pub fn why_disabled(resp: Response, disabled: bool, reason: &str) -> Response {
    if disabled {
        resp.on_hover_text(crate::i18n::t(reason))
    } else {
        resp
    }
}

/// [`text_fit`] that shows the whole text on hover whenever it cut it: the one way to draw a
/// line that may not fit (no caller can forget the hover). `salt` tells the lines apart.
#[allow(clippy::too_many_arguments)]
pub fn fit(
    ui: &Ui,
    p: &Painter,
    x: f32,
    baseline: f32,
    s: &str,
    color: Color32,
    right: f32,
    salt: impl std::hash::Hash + std::fmt::Debug,
) -> Rect {
    let (rect, cut) = text_fit(p, x, baseline, s, color, right);
    if cut {
        full_on_hover(ui, rect, salt, s);
    }
    rect
}

/// [`fit`] for a path or an address: shortened in the middle ([`ellipsize_middle`]) so the
/// file's name stays in sight, shown whole on hover when cut.
#[allow(clippy::too_many_arguments)]
pub fn fit_middle(
    ui: &Ui,
    p: &Painter,
    x: f32,
    baseline: f32,
    s: &str,
    color: Color32,
    right: f32,
    salt: impl std::hash::Hash + std::fmt::Debug,
) -> Rect {
    let shown = ellipsize_middle(p, &tahoma(), s, (right - x).max(0.0));
    let rect = text(p, x, baseline, &shown, color);
    if shown != s {
        full_on_hover(ui, rect, salt, s);
    }
    rect
}

/// Text already drawn in `rect` that the user may want to copy (an address, a hash, a path, a
/// version): its right-click menu copies it whole ([`copy_menu`]), and it shows whole on hover
/// when `cut`. Taken by the pointer only (labels are no Tab stops).
pub fn copyable(
    ui: &Ui,
    rect: Rect,
    salt: impl std::hash::Hash + std::fmt::Debug,
    text: &str,
    cut: bool,
) -> Response {
    let resp = ui.interact(rect, ui.scope_id().with(("copyable", salt)), pointer_only());
    copy_menu(&resp, text);
    if cut { resp.on_hover_text(text) } else { resp }
}

/// Right-aligned label whose ink ends at `right`.
pub fn text_right(p: &Painter, right: f32, baseline: f32, s: &str, color: Color32) -> (Rect, bool) {
    // A text longer than the room to the window's near edge (6 px margin) is cut with "...":
    // it never starts outside the window. The caller shows it whole on hover when cut.
    text_right_within(p, right, baseline, s, color, window_edges(p).0)
}

/// [`text_right`] that starts no further out than English x `near` (inside a group box).
pub fn text_right_within(
    p: &Painter,
    right: f32,
    baseline: f32,
    s: &str,
    color: Color32,
    near: f32,
) -> (Rect, bool) {
    let room = right + 1.0 - near;
    let (shown, cut) = if text_width(p, s) <= room {
        (s.to_owned(), false)
    } else {
        (ellipsize(p, &tahoma(), s, room.max(0.0)), true)
    };
    let w = text_width(p, &shown);
    (text(p, right + 1.0 - w, baseline, &shown, color), cut)
}

/// Fill an exact pixel rect with no anti-aliasing (egui feathers `rect_filled` by half a pixel,
/// which erases 1 × 1 rects and softens 1 px lines).
pub fn solid(p: &Painter, rc: Rect, c: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.add_colored_rect(snap(p.pixels_per_point(), rc), c);
    p.add(egui::Shape::mesh(mesh));
}

/// Fills `rc`, its corners rounded by `radius`, shading from top to bottom through `stops`
/// (each a place in the height, 0 at the top and 1 at the bottom, and its colour, in order).
/// Two stops at one place make a sharp change (Windows 7's glossy halves). The era themes'
/// buttons, boxes, bars and title bars are drawn with it (#45).
pub fn fill_shaded(p: &Painter, rc: Rect, radius: f32, stops: &[(f32, Color32)]) {
    fill_shaded_corners(p, rc, radius, radius, stops);
}

/// [`fill_shaded`] with the top corners rounded by `top` and the bottom ones by `bottom` (a
/// button hung from a title bar's edge).
pub fn fill_shaded_corners(p: &Painter, rc: Rect, top: f32, bottom: f32, stops: &[(f32, Color32)]) {
    let (Some(first), Some(last)) = (stops.first(), stops.last()) else {
        return;
    };
    if rc.width() <= 0.0 || rc.height() <= 0.0 {
        return;
    }
    let most = rc.width().min(rc.height()) / 2.0;
    let (rt, rb) = (top.clamp(0.0, most), bottom.clamp(0.0, most));
    // How far in from the sides the shape is at `y`: its corners' curve.
    let inset = |y: f32| {
        let (r, d) = if y < rc.top() + rt {
            (rt, rc.top() + rt - y)
        } else if y > rc.bottom() - rb {
            (rb, y - (rc.bottom() - rb))
        } else {
            return 0.0;
        };
        r - (r * r - d * d).max(0.0).sqrt()
    };
    let lerp = |a: Color32, b: Color32, t: f32| {
        let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
        Color32::from_rgba_premultiplied(
            m(a.r(), b.r()),
            m(a.g(), b.g()),
            m(a.b(), b.b()),
            m(a.a(), b.a()),
        )
    };
    let mut all = Vec::with_capacity(stops.len() + 2);
    all.push((0.0, first.1));
    all.extend(stops.iter().map(|&(t, c)| (t.clamp(0.0, 1.0), c)));
    all.push((1.0, last.1));
    let h = rc.height();
    let mut mesh = egui::Mesh::default();
    for w in all.windows(2) {
        let ((t0, c0), (t1, c1)) = (w[0], w[1]);
        if t1 <= t0 {
            continue;
        }
        let (y0, y1) = (rc.top() + t0 * h, rc.top() + t1 * h);
        // A row a pixel high where a corner curves, else the band whole.
        let mut ys = vec![y0];
        let mut y = y0.floor() + 1.0;
        while y < y1 {
            if y < rc.top() + rt || y > rc.bottom() - rb {
                ys.push(y);
            }
            y += 1.0;
        }
        for edge in [rc.top() + rt, rc.bottom() - rb] {
            if edge > y0 && edge < y1 {
                ys.push(edge);
            }
        }
        ys.push(y1);
        ys.sort_by(f32::total_cmp);
        ys.dedup();
        for pair in ys.windows(2) {
            let (ya, yb) = (pair[0], pair[1]);
            let ca = lerp(c0, c1, (ya - y0) / (y1 - y0));
            let cb = lerp(c0, c1, (yb - y0) / (y1 - y0));
            let (ia, ib) = (inset(ya), inset(yb));
            let i = mesh.vertices.len() as u32;
            mesh.colored_vertex(pos2(rc.left() + ia, ya), ca);
            mesh.colored_vertex(pos2(rc.right() - ia, ya), ca);
            mesh.colored_vertex(pos2(rc.left() + ib, yb), cb);
            mesh.colored_vertex(pos2(rc.right() - ib, yb), cb);
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i + 1, i + 3, i + 2);
        }
    }
    p.add(egui::Shape::mesh(mesh));
}

/// How an era theme's glossy parts shade (#45): Windows XP's soft fall from white, Windows 7's
/// two halves, Mac OS X's gel (a bright top, a dark middle, light coming back at the bottom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shine {
    Luna,
    Aero,
    Aqua,
}

impl Shine {
    /// The theme's shine, from its button style; None for the flat looks.
    pub fn of_theme() -> Option<Self> {
        Self::of_button(crate::theme::current().controls.button)
    }

    fn of_button(look: crate::palette::ButtonLook) -> Option<Self> {
        use crate::palette::ButtonLook as B;
        match look {
            B::Fluent | B::Breeze => None,
            B::Luna => Some(Shine::Luna),
            B::Aero => Some(Shine::Aero),
            B::Aqua => Some(Shine::Aqua),
        }
    }

    /// A face of colour `f` shaded toward the theme's shine `g` (`gloss`), top to bottom, for
    /// [`fill_shaded`].
    pub fn stops(self, f: Color32, g: Color32) -> [(f32, Color32); 4] {
        use crate::derive::mix;
        match self {
            Shine::Luna => [
                (0.0, mix(f, g, 0.85)),
                (0.4, mix(f, g, 0.6)),
                (0.85, mix(f, g, 0.2)),
                (1.0, f),
            ],
            Shine::Aero => [
                (0.0, mix(f, g, 0.75)),
                (0.5, mix(f, g, 0.5)),
                (0.5, mix(f, g, 0.25)),
                (1.0, f),
            ],
            Shine::Aqua => [
                (0.0, mix(f, g, 0.85)),
                (0.5, mix(f, g, 0.4)),
                (0.5, f),
                (1.0, mix(f, g, 0.45)),
            ],
        }
    }
}

/// `rc` on whole screen pixels at `ppp` pixels a point (125 %, 150 % scaling put a 1 pt line
/// across two pixels, blurred or uneven), never thinner than one pixel. At 100 % it is `rc`.
fn snap(ppp: f32, rc: Rect) -> Rect {
    let round = |v: f32| (v * ppp).round() / ppp;
    let (x0, y0) = (round(rc.min.x), round(rc.min.y));
    let one = 1.0 / ppp;
    let x1 = round(rc.max.x).max(x0 + if rc.width() > 0.0 { one } else { 0.0 });
    let y1 = round(rc.max.y).max(y0 + if rc.height() > 0.0 { one } else { 0.0 });
    Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
}

/// Fill inclusive pixel coordinates `x0..=x1, y0..=y1`.
pub fn fill(p: &Painter, x0: f32, y0: f32, x1: f32, y1: f32, c: Color32) {
    solid(
        p,
        Rect::from_min_max(pos2(x0, y0), pos2(x1 + 1.0, y1 + 1.0)),
        c,
    );
}

pub fn px(p: &Painter, x: f32, y: f32, c: Color32) {
    fill(p, x, y, x, y, c);
}
fn hline(p: &Painter, x0: f32, x1: f32, y: f32, c: Color32) {
    fill(p, x0, y, x1, y, c);
}
fn vline(p: &Painter, x: f32, y0: f32, y1: f32, c: Color32) {
    fill(p, x, y0, x, y1, c);
}

/// The rounding of a control's corners (buttons, text boxes, drop-downs), in both modes.
pub fn control_radius() -> f32 {
    crate::theme::metrics().control_radius
}

/// What a text box allows: typing, nothing (greyed), or reading and copying.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldLook {
    Editable,
    Disabled,
    ReadOnly,
}

/// A text box's frame, in both themes Windows 11's: rounded as the buttons are, a quiet 1 px
/// border and, while it can be typed in, a stronger bottom line (the focus turns it into the
/// accent: [`focus_bottom`]). Disabled and read-only boxes are flat: a disabled one dims its
/// text, a read-only one keeps it (it can be read and copied) on a light grey. The frame paints
/// the box's inside too, so every kind of text box agrees (they had three insides between them,
/// and dark mode drew IDM's bright 2 px border round every box, disabled ones included).
pub fn field_frame(p: &Painter, rc: Rect, look: FieldLook) {
    let radius = control_radius();
    let c = crate::theme::colors();
    // Disabled and read-only boxes are flat: their bottom line is their border.
    let (inside, border, bottom) = match look {
        FieldLook::Editable => (c.field, c.field_border, c.field_bottom),
        FieldLook::Disabled => (
            c.field_disabled,
            c.field_disabled_border,
            c.field_disabled_border,
        ),
        FieldLook::ReadOnly => (
            c.field_readonly,
            c.field_readonly_border,
            c.field_readonly_border,
        ),
    };
    p.rect_filled(rc, radius, inside);
    p.rect_stroke(
        rc,
        radius,
        Stroke::new(crate::theme::metrics().border, border),
        StrokeKind::Inside,
    );
    hline(
        p,
        rc.left() + radius,
        rc.right() - radius - 1.0,
        rc.bottom() - 1.0,
        bottom,
    );
}

/// How far along something is, as IDM shows a download: its green fill on a track (themed: the
/// track was light in dark mode too), the controls' rounded corners. `fraction` in 0..=1.
pub fn meter(p: &Painter, rc: Rect, fraction: f32) {
    meter_in(p, rc, fraction, crate::theme::colors().meter_fill);
}

/// [`meter`] filled in `fill` (the progress window's check or merge after the last byte, #736).
pub fn meter_in(p: &Painter, rc: Rect, fraction: f32, fill: Color32) {
    let radius = control_radius().min(rc.height() / 4.0).floor();
    let c = crate::theme::colors();
    if era_bar(p, rc, 0.0, fraction.clamp(0.0, 1.0), fill, false) {
        return;
    }
    let border = crate::theme::metrics().border;
    p.rect_filled(rc, radius, c.meter_track);
    let inner = rc.shrink(border);
    let w = (fraction.clamp(0.0, 1.0) * inner.width()).floor();
    if w > 0.0 {
        let done = Rect::from_min_size(inner.min, vec2(w, inner.height()));
        p.rect_filled(done, (radius - border).max(0.0), fill);
    }
    p.rect_stroke(
        rc,
        radius,
        Stroke::new(border, c.meter_edge),
        StrokeKind::Inside,
    );
}

/// An era theme's bar (`palette::ProgressLook::Blocks` or `Gloss`) in `rc` (on screen): a
/// shaded track inside its edge, filled from `from` to `to` (fractions of its inside, from its
/// left, or from its right when `mirrored`) in `fill`: in blocks (Windows XP), or glossy as
/// the theme's buttons (Windows 7, Mac OS X). False, drawing nothing, for the other looks.
fn era_bar(p: &Painter, rc: Rect, from: f32, to: f32, fill: Color32, mirrored: bool) -> bool {
    use crate::derive::mix;
    use crate::palette::ProgressLook;
    let look = crate::theme::current().controls.progress;
    if !matches!(look, ProgressLook::Blocks | ProgressLook::Gloss) {
        return false;
    }
    let c = crate::theme::colors();
    let m = crate::theme::metrics();
    let blocks = look == ProgressLook::Blocks;
    let radius = if blocks {
        m.button_radius
    } else {
        control_radius()
    }
    .min((rc.height() / 2.0).floor());
    fill_shaded(
        p,
        rc,
        radius,
        &[
            (0.0, mix(c.meter_track, c.meter_edge, 0.3)),
            (0.35, c.meter_track),
            (1.0, c.meter_track),
        ],
    );
    let inner = rc.shrink(if blocks { 2.0 + m.border } else { m.border });
    let w = inner.width();
    // A part of the inside, `a` to `b` of it from the bar's start.
    let span = |a: f32, b: f32| {
        if mirrored {
            Rect::from_min_max(
                pos2(inner.right() - b, inner.top()),
                pos2(inner.right() - a, inner.bottom()),
            )
        } else {
            Rect::from_min_max(
                pos2(inner.left() + a, inner.top()),
                pos2(inner.left() + b, inner.bottom()),
            )
        }
    };
    let (a, b) = ((from * w).round(), (to * w).round());
    if b > a && w > 0.0 {
        if blocks {
            // Windows XP: blocks 2 points apart, lighter across their middle; whole ones, the
            // last cut at the bar's end.
            let stops = [(0.0, fill), (0.45, mix(fill, c.gloss, 0.55)), (1.0, fill)];
            let bw = (inner.height() * 0.6).round().max(4.0);
            let mut x = (a / (bw + 2.0)).floor() * (bw + 2.0);
            while x < b {
                let block = span(x.max(a), (x + bw).min(w));
                if block.width() > 0.0 {
                    fill_shaded(p, block, 0.0, &stops);
                }
                x += bw + 2.0;
            }
        } else {
            let shine = Shine::of_theme().unwrap_or(Shine::Aero);
            fill_shaded(
                p,
                span(a, b),
                (radius - m.border).max(0.0),
                &shine.stops(fill, c.gloss),
            );
        }
    }
    p.rect_stroke(
        rc,
        radius,
        Stroke::new(m.border, c.meter_edge),
        StrokeKind::Inside,
    );
    true
}

/// A box whose value is wrong (a number out of range, an address that isn't one, a schedule
/// that can't start): the error colour round it, on the control's own corners (it was drawn
/// three ways).
pub fn error_frame(p: &Painter, rc: Rect) {
    p.rect_stroke(
        rc,
        control_radius(),
        Stroke::new(crate::theme::metrics().border, ERROR()),
        StrokeKind::Inside,
    );
}

/// A control's caption or a label beside it: the text colour, or the one disabled grey (no
/// classic embossed shadow: Windows 11 has none).
pub fn label(p: &Painter, x: f32, baseline: f32, s: &str, enabled: bool) -> Rect {
    text(
        p,
        x,
        baseline,
        s,
        if enabled {
            LIGHT_TEXT()
        } else {
            TEXT_DISABLED()
        },
    )
}

/// [`field_frame`] of a box that can be typed in, or not.
pub fn edit_frame(p: &Painter, rc: Rect, enabled: bool) {
    field_frame(
        p,
        rc,
        if enabled {
            FieldLook::Editable
        } else {
            FieldLook::Disabled
        },
    );
}

/// A single-line edit: frame + egui text editing positioned so text starts at x+5 with its
/// baseline at top+13.
pub fn edit(ui: &mut Ui, rc: Rect, value: &mut String, enabled: bool, password: bool) -> Response {
    edit_frame(ui.painter(), rc, enabled);
    let resp = edit_text(ui, rc, value, enabled, password, 5.0);
    edit_focus_line(ui, &resp, rc);
    resp
}

/// A text box's focus, as Windows 11 shows it: an accent line along its bottom edge.
pub fn edit_focus_line(ui: &Ui, resp: &Response, rc: Rect) {
    if resp.has_focus() {
        focus_bottom(ui.painter(), rc);
    }
}

/// The 2 px accent along a focused box's bottom edge. Light: the bottom of its rounded
/// frame, curving into the corners as Windows 11 draws it; dark: IDM's straight line.
pub fn focus_bottom(p: &Painter, rc: Rect) {
    let h = crate::theme::metrics().focus_line;
    // GNOME: a ring inside the box, the accent half seen through.
    if crate::theme::current().controls.field_focus == crate::palette::FieldFocus::Ring {
        p.rect_stroke(
            rc,
            control_radius(),
            Stroke::new(2.0, accent().gamma_multiply(0.5)),
            StrokeKind::Inside,
        );
        return;
    }
    // Windows 8 and XP: the whole border in the accent, not a line along the bottom.
    if crate::theme::current().controls.field_focus == crate::palette::FieldFocus::Border {
        p.rect_stroke(
            rc,
            control_radius(),
            Stroke::new(crate::theme::metrics().border, accent()),
            StrokeKind::Inside,
        );
        return;
    }
    if crate::theme::colors().focus_line_straight {
        fill(
            p,
            rc.left() + 1.0,
            rc.bottom() - h,
            rc.right() - 2.0,
            rc.bottom() - 1.0,
            accent(),
        );
    } else {
        let strip = Rect::from_min_max(pos2(rc.left(), rc.bottom() - h), rc.max);
        p.with_clip_rect(strip)
            .rect_filled(rc, control_radius(), accent());
    }
}

fn edit_text(
    ui: &mut Ui,
    rc: Rect,
    value: &mut String,
    enabled: bool,
    password: bool,
    inset: f32,
) -> Response {
    let color = if enabled {
        TEXT()
    } else {
        EDIT_TEXT_DISABLED()
    };
    edit_text_with(
        ui,
        rc,
        value,
        enabled,
        password,
        inset,
        color,
        start_align(),
    )
}

/// Text aligned to its start: left, right in RTL.
pub fn start_align() -> egui::Align {
    if rtl() {
        egui::Align::RIGHT
    } else {
        egui::Align::LEFT
    }
}

/// Text aligned to its end (numbers in a box): right, left in RTL.
pub fn end_align() -> egui::Align {
    if rtl() {
        egui::Align::LEFT
    } else {
        egui::Align::RIGHT
    }
}

#[allow(clippy::too_many_arguments)]
fn edit_text_with(
    ui: &mut Ui,
    rc: Rect,
    value: &mut String,
    enabled: bool,
    password: bool,
    inset: f32,
    color: Color32,
    align: egui::Align,
) -> Response {
    // Tahoma −11: ascent 11, so the text cell top is baseline − 10 (top + 3 in a 21 px box),
    // the line centred in the box at any height (baseline_in).
    let base = baseline_in(rc.top(), rc.height());
    let inner = Rect::from_min_max(
        pos2(rc.left() + inset, base - 10.0),
        pos2(rc.right() - inset, base + 3.0),
    );
    // The whole drawn box takes clicks (the text itself fills only a strip inside it): first,
    // so the text on top still gets its own; a click elsewhere in the box focuses it.
    // Keyed by its window's opening too: a box shown again for another record (New login after
    // Edit login) starts without the last one's text in its undo.
    // (The key first: asking the context inside its own lock deadlocks.)
    let opening = crate::dialogs::opening_key(ui.ctx());
    let opened = ui.data(|d| d.get_temp::<u64>(opening)).unwrap_or(0);
    let edit_id = widget_id(ui, ("edit-box", rc.min.x as i32, rc.min.y as i32, opened));
    if enabled {
        let frame = ui.interact(rc, edit_id.with("frame"), pointer_only());
        if frame.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        if frame.clicked() {
            ui.memory_mut(|m| m.request_focus(edit_id));
        }
    }
    // A box that was empty and is then filled by its window, not typed in (a link from the
    // clipboard, read off the window's thread, comes a frame or two after the window opens:
    // #661), has its caret where it would be had it opened with the text (at its end), not left
    // at its start.
    let was_empty = edit_id.with("was-empty");
    if !value.is_empty() && ui.data(|d| d.get_temp::<bool>(was_empty)).is_some() {
        ui.data_mut(|d| d.remove::<bool>(was_empty));
        if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), edit_id) {
            state.cursor.set_char_range(None);
            state.store(ui.ctx(), edit_id);
        }
    }
    let te = egui::TextEdit::singleline(value)
        .id(edit_id)
        .align(egui::Align2([align, egui::Align::TOP]))
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .font(tahoma())
        .text_color(color)
        .password(password)
        // Enter keeps the focus in the box (the dialog's default button answers it; a box that
        // lost the keys on Enter took the next typing nowhere).
        .return_key(None)
        .desired_width(inner.width());
    if ui.memory(|m| m.has_focus(edit_id)) {
        ui.input_mut(|i| {
            let mut events = Vec::with_capacity(i.events.len());
            for e in i.events.drain(..) {
                match e {
                    // Ctrl+V: one clean line (see single_line).
                    egui::Event::Paste(t) => events.push(egui::Event::Paste(single_line(&t))),
                    // A password's word moves would show where its words end: whole-box
                    // moves instead (Ctrl+Left / Right to its ends, Ctrl+Backspace / Delete
                    // to them).
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        physical_key,
                        repeat,
                    } if password
                        && modifiers.command
                        && matches!(
                            key,
                            egui::Key::ArrowLeft
                                | egui::Key::ArrowRight
                                | egui::Key::Backspace
                                | egui::Key::Delete
                        ) =>
                    {
                        let plain = egui::Modifiers {
                            shift: modifiers.shift,
                            ..Default::default()
                        };
                        let select = egui::Modifiers::SHIFT;
                        let press = |key, modifiers| egui::Event::Key {
                            key,
                            physical_key,
                            pressed: true,
                            repeat,
                            modifiers,
                        };
                        match key {
                            egui::Key::ArrowLeft => events.push(press(egui::Key::Home, plain)),
                            egui::Key::ArrowRight => events.push(press(egui::Key::End, plain)),
                            egui::Key::Backspace => {
                                events.push(press(egui::Key::Home, select));
                                events.push(press(egui::Key::Backspace, Default::default()));
                            }
                            _ => {
                                events.push(press(egui::Key::End, select));
                                events.push(press(egui::Key::Delete, Default::default()));
                            }
                        }
                    }
                    other => events.push(other),
                }
            }
            i.events = events;
        });
    }
    let resp = ui.put(inner, |ui: &mut Ui| {
        // The caret in the box's own ink: egui's default (a pale blue in its dark style) was
        // all but invisible on a light box.
        ui.visuals_mut().text_cursor.stroke.color = color;
        ui.add_enabled_ui(enabled, |ui| ui.add(te)).inner
    });
    pass_focus_on(ui, &resp, enabled);
    // Named for screen readers by its caption.
    name_from_caption(&resp);
    // Tab into a box selects its text, as in Windows (a click places the caret instead).
    if resp.gained_focus() && !ui.input(|i| i.pointer.any_down()) {
        select_all_text(ui.ctx(), edit_id, value);
    }
    if value.is_empty() && ui.data(|d| d.get_temp::<bool>(was_empty)).is_none() {
        ui.data_mut(|d| d.insert_temp(was_empty, true));
    }
    if enabled {
        edit_menu(&resp, value, password, false);
    }
    resp
}

/// A text box's right-click menu, each item enabled or not.
fn edit_entries(
    undo: bool,
    copy: bool,
    paste: bool,
    delete: bool,
    select_all: bool,
) -> Vec<crate::chrome::MenuEntry<EditCmd>> {
    use crate::chrome::MenuEntry;
    vec![
        MenuEntry::Item("&Undo", EditCmd::Undo, undo, false),
        MenuEntry::Sep,
        MenuEntry::Item("Cu&t", EditCmd::Cut, copy, false),
        MenuEntry::Item("&Copy", EditCmd::Copy, copy, false),
        MenuEntry::Item("&Paste", EditCmd::Paste, paste, false),
        MenuEntry::Item("&Delete", EditCmd::Delete, delete, false),
        MenuEntry::Sep,
        MenuEntry::Item("Select &all", EditCmd::SelectAll, select_all, false),
    ]
}

/// A text box's right-click menu, for the access keys' test (app.rs).
#[cfg(test)]
pub fn text_box_menu() -> Vec<crate::chrome::MenuEntry<()>> {
    use crate::chrome::MenuEntry;
    edit_entries(true, true, true, true, true)
        .into_iter()
        .map(|e| match e {
            MenuEntry::Item(l, _, en, on) => MenuEntry::Item(l, (), en, on),
            _ => MenuEntry::Sep,
        })
        .collect()
}

#[derive(Clone, Copy)]
enum EditCmd {
    Undo,
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
}

/// A text box's right-click menu: Cut, Copy, Paste, Delete and Select all, on the box's own
/// selection, as every Windows, macOS and Linux text box has (Cut and Copy not for passwords). A
/// menu window of its own (`chrome::popup`), so short dialogs never cut it off.
/// A right-click (or Shift+F10) menu with Copy for read-only text `text` shown by `resp` (a
/// value, a version, an address): the whole text goes to the clipboard. Ctrl+C does the same
/// while `resp` has the focus.
pub fn copy_menu(resp: &Response, text: &str) {
    use crate::chrome::{MenuEntry, PopupResult};
    let ctx = resp.ctx.clone();
    let key = resp.id.with("copy-menu");
    if resp.has_focus() && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
        crate::copy_text(&ctx, text.to_owned());
    }
    if resp.secondary_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        ctx.data_mut(|d| d.insert_temp(key, p));
    }
    if resp.has_focus() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F10))
    {
        ctx.data_mut(|d| d.insert_temp(key, resp.rect.left_bottom()));
    }
    let Some(at) = ctx.data(|d| d.get_temp::<Pos2>(key)) else {
        return;
    };
    let entries = [MenuEntry::Item("&Copy", (), !text.is_empty(), false)];
    match crate::chrome::popup(
        &ctx,
        crate::chrome::MenuKind::Context,
        &format!("copy-{:?}", resp.id),
        at,
        &entries,
        &|_| None,
    ) {
        PopupResult::Open | PopupResult::Neighbor(_) => {}
        PopupResult::Chosen(()) => {
            crate::copy_text(&ctx, text.to_owned());
            ctx.data_mut(|d| d.remove::<Pos2>(key));
        }
        PopupResult::Dismissed => ctx.data_mut(|d| d.remove::<Pos2>(key)),
    }
}

/// A read-only value in a box (a path, an address): cut to fit with "..." and whole on hover,
/// on `fill`; it takes the focus (Tab), and Ctrl+C or its right-click menu copies the whole
/// value ([`copy_menu`]). `ltr`: the value reads left to right in every language (paths,
/// addresses keep their "..." at their end in a right-to-left interface).
pub fn readonly_field(
    ui: &mut Ui,
    rc: Rect,
    value: &str,
    fill: Color32,
    ltr: bool,
    id: impl std::hash::Hash + std::fmt::Debug,
) -> Response {
    // `fill`: the surface it sits on (kept for its callers; the read-only look has its own).
    let _ = fill;
    readonly_with_icon(ui, rc, value, ltr, id, None)
}

/// [`readonly_field`] holding a file name, after its type's icon (the shared file icons).
pub fn readonly_file(
    ui: &mut Ui,
    rc: Rect,
    name: &str,
    id: impl std::hash::Hash + std::fmt::Debug,
) -> Response {
    let icon = (!name.is_empty()).then_some(crate::fileicons::Kind::File(name));
    readonly_with_icon(ui, rc, name, true, id, icon)
}

fn readonly_with_icon(
    ui: &mut Ui,
    rc: Rect,
    value: &str,
    ltr: bool,
    id: impl std::hash::Hash + std::fmt::Debug,
    icon: Option<crate::fileicons::Kind>,
) -> Response {
    field_frame(ui.painter(), rc, FieldLook::ReadOnly);
    let p = ui.painter().clone();
    // From the field's leading edge (an English pen in its English rect).
    let mut e = rect(rc);
    if let Some(kind) = icon {
        let top = e.top() + ((e.height() - 16.0) / 2.0).floor();
        crate::fileicons::paint_at(ui, &p, kind, pos2(e.left() + 3.0, top), 16.0);
        e.min.x += crate::fileicons::INDENT - 2.0;
    }
    let shown = ellipsize(&p, &tahoma(), value, e.width() - 9.0);
    let cut = shown != value;
    let shown = if ltr && rtl() {
        crate::i18n::ltr(&shown)
    } else {
        shown
    };
    text(
        &p,
        e.left() + 5.0,
        baseline_in(e.top(), e.height()),
        &shown,
        LIGHT_TEXT(),
    );
    let resp = ui.interact(rc, widget_id(ui, ("readonly", id)), Sense::click());
    focus_on_press(&resp);
    expose(&resp, egui::Role::TextInput, value, true, None);
    // Focused as every text box shows it: the accent along its bottom.
    if resp.has_focus() {
        focus_bottom(&p, rc);
    }
    copy_menu(&resp, value);
    if cut { resp.on_hover_text(value) } else { resp }
}

pub fn edit_menu(resp: &Response, value: &mut String, password: bool, multiline: bool) {
    use crate::chrome::PopupResult;
    use egui::text::{CCursor, CCursorRange};
    use egui::text_edit::TextEditState;
    let ctx = resp.ctx.clone();
    let id = resp.id;
    let key = id.with("edit-menu");
    // Paste is offered when the clipboard has text: asked as the menu opens, read off the
    // window's thread (clipboard.rs; read here at every frame, a clipboard owner that hung froze
    // the window, #661).
    let clip_key = key.with("clipboard");
    let open_at = |p: Pos2| {
        let ask = crate::clipboard::ask();
        ctx.data_mut(|d| {
            d.insert_temp(key, p);
            d.insert_temp(clip_key, ask);
        });
    };
    if resp.secondary_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        open_at(p);
    }
    // Shift+F10 (the keyboard's way to a right-click menu) opens it under the box.
    if resp.has_focus() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F10))
    {
        open_at(resp.rect.left_bottom());
    }
    let Some(at) = ctx.data(|d| d.get_temp::<Pos2>(key)) else {
        return;
    };
    let mut state = TextEditState::load(&ctx, id).unwrap_or_default();
    let n = value.chars().count();
    let (a, b) = state
        .cursor
        .char_range()
        .map(|r| {
            let (x, y) = (r.primary.index.0.min(n), r.secondary.index.0.min(n));
            (x.min(y), x.max(y))
        })
        .unwrap_or((n, n));
    let has = a < b;
    // Until the answer comes (a frame or two; never, from an owner that hung), what the
    // clipboard last held.
    let clip = ctx
        .data(|d| d.get_temp::<crate::clipboard::Ask>(clip_key))
        .and_then(crate::clipboard::answer)
        .unwrap_or_else(crate::clipboard::latest)
        .filter(|t| !t.is_empty());
    // egui's own undo history (Ctrl+Z uses it too).
    let mut undoer = state.undoer();
    let now = (state.cursor.char_range().unwrap_or_default(), value.clone());
    let entries = edit_entries(
        undoer.has_undo(&now),
        has && !password,
        clip.is_some(),
        has,
        n > 0,
    );
    let cmd = match crate::chrome::popup(
        &ctx,
        crate::chrome::MenuKind::Context,
        &format!("edit-{id:?}"),
        at,
        &entries,
        &|_| None,
    ) {
        PopupResult::Open | PopupResult::Neighbor(_) => return,
        PopupResult::Dismissed => None,
        PopupResult::Chosen(c) => Some(c),
    };
    ctx.data_mut(|d| {
        d.remove::<Pos2>(key);
        d.remove::<crate::clipboard::Ask>(clip_key);
    });
    let Some(cmd) = cmd else { return };
    let byte = |c: usize| value.char_indices().nth(c).map_or(value.len(), |(i, _)| i);
    let (ba, bb) = (byte(a), byte(b));
    let caret = match cmd {
        EditCmd::Undo => {
            let Some((range, text)) = undoer.undo(&now).cloned() else {
                return;
            };
            *value = text;
            state.set_undoer(undoer);
            state.cursor.set_char_range(Some(range));
            state.store(&ctx, id);
            ctx.memory_mut(|m| m.request_focus(id));
            return;
        }
        // Each change an undo step of its own (Undo after the menu's Paste did nothing).
        EditCmd::Cut => {
            crate::copy_text(&ctx, value[ba..bb].to_owned());
            undoer.add_undo(&now);
            value.replace_range(ba..bb, "");
            (a, a)
        }
        EditCmd::Copy => {
            crate::copy_text(&ctx, value[ba..bb].to_owned());
            (a, b)
        }
        EditCmd::Paste => {
            // A single-line box takes one line; a list box keeps the lines.
            let text = clip.unwrap_or_default();
            let text = if multiline {
                text.replace("\r\n", "\n")
            } else {
                single_line(&text)
            };
            undoer.add_undo(&now);
            value.replace_range(ba..bb, &text);
            let end = a + text.chars().count();
            (end, end)
        }
        EditCmd::Delete => {
            undoer.add_undo(&now);
            value.replace_range(ba..bb, "");
            (a, a)
        }
        EditCmd::SelectAll => (0, value.chars().count()),
    };
    state.cursor.set_char_range(Some(CCursorRange::two(
        CCursor::new(caret.0),
        CCursor::new(caret.1),
    )));
    state.set_undoer(undoer);
    state.store(&ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
}

/// The whole text of the box `id` (`value`) selected, so what is typed next replaces it (a box
/// given the keys by its window as it opens, as Tab into a box does).
pub fn select_all_text(ctx: &egui::Context, id: egui::Id, value: &str) {
    let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(value.chars().count()),
        )));
    state.store(ctx, id);
}

/// A dark edit with right-aligned text (numbers, e.g. the Speed Limiter value).
pub fn edit_right(ui: &mut Ui, rc: Rect, value: &mut String, enabled: bool) -> Response {
    edit_frame(ui.painter(), rc, enabled);
    let color = if enabled {
        TEXT()
    } else {
        EDIT_TEXT_DISABLED()
    };
    let resp = edit_text_with(ui, rc, value, enabled, false, 5.0, color, end_align());
    // Its focus shows as every text box's does.
    edit_focus_line(ui, &resp, rc);
    resp
}

// ---- control states -------------------------------------------------------------------------

/// What a control shows this frame: at rest, under the pointer (hot), with the mouse button held
/// on it (pressed), with its list or menu open, or disabled. Every control takes its look from
/// one of these, so they all answer the pointer the same way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Normal,
    Hot,
    Pressed,
    Open,
    Disabled,
}

impl State {
    /// From the control's response this frame.
    pub fn of(resp: &Response, enabled: bool) -> Self {
        if !enabled {
            State::Disabled
        } else if resp.is_pointer_button_down_on() && resp.contains_pointer() {
            // Held, it looks pressed only while the pointer is on it (as in Windows: moved
            // off, it lets go without a click).
            State::Pressed
        } else if resp.hovered() {
            State::Hot
        } else {
            State::Normal
        }
    }

    /// Shown open while `open` (a combo's list, a drop-down's menu); a press still shows.
    pub fn open_if(self, open: bool) -> Self {
        if open && matches!(self, State::Normal | State::Hot) {
            State::Open
        } else {
            self
        }
    }
}

/// The fill of a part with no face of its own (a combo's arrow, a glyph button, a column
/// header) in `state`: none at rest, a tint under the pointer, a deeper one pressed or open.
/// Colours from `c`, a theme's variant (the active one: `crate::theme::colors()`).
pub fn subtle_fill(state: State, c: &crate::palette::Palette) -> Option<Color32> {
    match state {
        State::Hot => Some(c.subtle_hot),
        State::Pressed | State::Open => Some(c.subtle_pressed),
        _ => None,
    }
}

/// The blue highlight of a calendar's days and arrows (Win32's month calendar) in `state`:
/// a light accent tint under the pointer, a deeper one pressed (from `c`).
pub fn highlight_fill(state: State, c: &crate::palette::Palette) -> Option<Color32> {
    match state {
        State::Hot => Some(c.highlight_hot),
        State::Pressed | State::Open => Some(c.highlight_pressed),
        _ => None,
    }
}

/// Tints `zone` (a part already drawn at rest) with `tint` (none at rest: [`subtle_fill`]) and
/// draws its glyph again on top.
pub fn tint_part(p: &Painter, zone: Rect, tint: Option<Color32>, glyph: impl FnOnce(&Painter)) {
    if let Some(c) = tint {
        p.rect_filled(zone, crate::theme::metrics().part_radius, c);
        glyph(p);
    }
}

/// An editable or drop-list combo's arrow zone (inside its border, at the trailing edge).
fn combo_zone(rc: Rect) -> Rect {
    if rtl() {
        Rect::from_min_max(
            rc.min + vec2(1.0, 1.0),
            pos2(rc.left() + 20.0, rc.bottom() - 1.0),
        )
    } else {
        Rect::from_min_max(
            pos2(rc.right() - 20.0, rc.top() + 1.0),
            rc.max - vec2(1.0, 1.0),
        )
    }
}

/// A combo's chevron: 8 × 6 "v" at the arrow zone's left + 6, top + 8.
fn combo_chevron(p: &Painter, rc: Rect) {
    let arrow_left = if rtl() {
        rc.left() + 1.0
    } else {
        rc.right() - 1.0 - 19.0
    };
    let (cx, cy) = (arrow_left + 6.0, rc.top() + 8.0);
    p.line(
        vec![
            pos2(cx, cy + 0.5),
            pos2(cx + 4.0, cy + 4.5),
            pos2(cx + 8.0, cy + 0.5),
        ],
        Stroke::new(
            crate::theme::metrics().chevron_width,
            crate::theme::colors().combo_chevron,
        ),
    );
}

/// Combo frame. Dark: 1 px border (#8d8d8d, or #0078d4 when focused), 2 px white ring, dark
/// text area, white 19 px arrow zone with a grey chevron. Height 21.
pub fn combo_frame(p: &Painter, rc: Rect, focused: bool) {
    // A text box with the drop-down's chevron (an editable drop-down is one), in both themes.
    edit_frame(p, rc, true);
    if focused {
        focus_bottom(p, rc);
    }
    combo_chevron(p, rc);
}

/// Drop-list combo showing `value`; returns the click response.
pub fn combo(ui: &mut Ui, rc: Rect, value: &str, id: &str) -> Response {
    let resp = ui.interact(rc, widget_id(ui, ("combo", id)), Sense::click());
    let resp = opener_click(ui, resp);
    expose_value(&resp, egui::Role::ComboBox, value, true);
    // A drop-down list is drawn as Windows 11 draws them (and as combo_light does): a button's
    // face, its state across the whole of it, focus shown as a button's.
    dropdown_frame_light(ui.painter(), rc, State::of(&resp, true));
    combo_chevron(ui.painter(), rc);
    show_focus(ui, &resp, rc.shrink(2.0));
    // Short of the arrow zone: a longer value ends in "..." and shows whole on hover.
    let shown = ellipsize(ui.painter(), &tahoma(), value, rc.width() - 5.0 - 22.0);
    text_start(
        ui.painter(),
        tahoma(),
        lead(rc) + 5.0 * fwd(),
        rc.top() + 13.0,
        &shown,
        TEXT(),
    );
    if shown != value {
        resp.on_hover_text(value)
    } else {
        resp
    }
}

/// Editable combo (Add URL "Address", Save As): frame + edit child at +3,+3 .. right−20 (the
/// arrow zone at the trailing edge).
/// Its blue focus border while its box has the keyboard focus.
pub fn combo_edit(ui: &mut Ui, rc: Rect, value: &mut String) -> Response {
    // The frame goes under the box, so it is drawn first, with the box's focus as last frame
    // left it (a change repaints at once).
    let key = widget_id(ui, ("combo-edit-focus", rc.min.x as i32, rc.min.y as i32));
    let was = ui.data(|d| d.get_temp::<bool>(key).unwrap_or(false));
    combo_frame(ui.painter(), rc, was);
    let child = if rtl() {
        Rect::from_min_max(pos2(rc.left() + 20.0, rc.top()), rc.max - vec2(3.0, 0.0))
    } else {
        Rect::from_min_max(
            rc.min + vec2(3.0, 0.0),
            pos2(rc.right() - 20.0, rc.bottom()),
        )
    };
    let resp = edit_text(ui, child, value, true, false, 2.0);
    // Read before the data lock: has_focus asks the context too, and inside it hung the app
    // the moment the box took the focus (0.2.2: Add URL froze as it opened).
    let focused = resp.has_focus();
    if focused != was {
        ui.data_mut(|d| d.insert_temp(key, focused));
        ui.ctx().request_repaint();
    }
    resp
}

/// The arrow zone of an editable combo (`combo_edit`, at the trailing edge): a click on it
/// opens the combo's drop list (`drop_list`); it shows hot, pressed, and pressed while `open`.
pub fn combo_arrow(ui: &mut Ui, rc: Rect, id: &str, open: bool) -> Response {
    let zone = combo_zone(rc);
    let resp = ui.interact(zone, widget_id(ui, ("combo-arrow", id)), pointer_only());
    let resp = opener_click(ui, resp);
    let state = State::of(&resp, true).open_if(open);
    let c = crate::theme::colors();
    let tint = match state {
        State::Hot => Some(c.combo_arrow_hot),
        State::Pressed | State::Open => Some(c.combo_arrow_pressed),
        _ => None,
    };
    tint_part(ui.painter(), zone, tint, |p| combo_chevron(p, rc));
    resp
}

/// The keys that open an editable combo's drop list while its box has the focus: Alt+Down or
/// F4, as Windows' combo boxes.
/// Up / Down in an editable drop-down's box (the list closed): the next / previous of its
/// `n` items, as Windows' editable combos step through theirs. `current`: the one the box
/// shows, if any. Returns the item to put in the box.
pub fn history_keys(ui: &Ui, edit: &Response, n: usize, current: Option<usize>) -> Option<usize> {
    if !edit.has_focus() || n == 0 {
        return None;
    }
    keep_keys(ui, edit, false, true, false);
    if take_key(ui, egui::Key::ArrowDown) {
        Some(current.map_or(0, |c| (c + 1).min(n - 1)))
    } else if take_key(ui, egui::Key::ArrowUp) {
        Some(current.map_or(0, |c| c.saturating_sub(1)))
    } else {
        None
    }
}

/// Text pasted into a one-line box: a trailing line break dropped, line breaks and tabs inside
/// it one space each, other control characters out ("url\r\n" was "url" and two spaces).
pub fn single_line(text: &str) -> String {
    let t = text.trim_end_matches(['\r', '\n']);
    let t = t.replace("\r\n", " ").replace(['\r', '\n', '\t'], " ");
    t.chars().filter(|c| !c.is_control()).collect()
}

pub fn combo_keys(ui: &mut Ui, edit: &Response) -> bool {
    edit.has_focus()
        && ui.input_mut(|i| {
            i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowDown)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::F4)
        })
}

/// An editable combo's drop list under `anchor`: `items` newest first, or `empty` greyed when
/// there are none. A window of its own, as the menus are (`chrome::popup`), so a list longer
/// than the dialog is never cut off, and the arrow keys, Enter and Escape work in it. Returns the
/// index picked; `open` turns false on a pick, on Escape or on a click anywhere else.
pub fn drop_list(
    ui: &mut Ui,
    id: &str,
    anchor: Rect,
    items: &[String],
    empty: &str,
    open: &mut bool,
) -> Option<usize> {
    use crate::chrome::{MenuEntry, PopupResult, label};
    // A menu label: long addresses shortened in the middle, "&" kept as text (not an access key).
    let shown = |t: &str| {
        let n = t.chars().count();
        let short: String = if n > 72 {
            t.chars()
                .take(36)
                .chain(std::iter::once('\u{2026}'))
                .chain(t.chars().skip(n - 35))
                .collect()
        } else {
            t.to_owned()
        };
        label(&short.replace('&', "&&"))
    };
    let entries: Vec<MenuEntry<usize>> = if items.is_empty() {
        vec![MenuEntry::Item(label(empty), usize::MAX, false, false)]
    } else {
        items
            .iter()
            .take(crate::settings::HISTORY)
            .enumerate()
            .map(|(i, t)| MenuEntry::Item(shown(t), i, true, false))
            .collect()
    };
    // Under the box's leading edge (its right one in a right-to-left interface, where menus
    // open leftwards from their top-right).
    let at = if rtl() {
        anchor.right_bottom()
    } else {
        anchor.left_bottom()
    };
    let menu = crate::chrome::popup(
        ui.ctx(),
        crate::chrome::MenuKind::List,
        &format!("drop-{id}"),
        at,
        &entries,
        &|_| None,
    );
    match menu {
        PopupResult::Chosen(i) => {
            *open = false;
            Some(i)
        }
        PopupResult::Dismissed => {
            *open = false;
            None
        }
        PopupResult::Open | PopupResult::Neighbor(_) => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Normal,
    Default,
    Disabled,
    /// The one answer a window recommends (the default too), filled with the accent as Windows
    /// 11's accent buttons: the first start's "Yes, download them".
    Accent,
}

/// Push button: #333333 fill, 1 px rounded (r 4) border by state, text centred with its
/// baseline at top+14 (h 21) or top+13 (h 19). `mnemonic` underlines that char index.
pub fn button(
    ui: &mut Ui,
    rc: Rect,
    label: &str,
    kind: ButtonKind,
    focus_rect: bool,
    mnemonic: Option<usize>,
) -> Response {
    // The label is translated here (M16); a mnemonic index points into the English text only,
    // and becomes the "&" form every button uses.
    let translated = crate::i18n::t(label);
    let label = match mnemonic.filter(|_| translated == label) {
        Some(i) => {
            let at = label.char_indices().nth(i).map_or(label.len(), |(b, _)| b);
            format!("{}&{}", &label[..at], &label[at..])
        }
        None => translated.to_owned(),
    };
    button_face(ui, rc, &label, kind, focus_rect)
}

/// A button's rect `rc` widened to the left (its right edge stays: a button against the right
/// side, with room before it) to fit its translated `label` whole, with the room [`button`]
/// keeps inside the border, at most to `max_width`. English labels fit IDM's buttons, so
/// English never changes; a label that would still not fit ends in "..." and shows whole on
/// hover, as on every button. (A row of buttons: [`buttons_from_left`], [`buttons_to_right`].)
pub fn button_rect_for(p: &Painter, rc: Rect, label: &str, max_width: f32) -> Rect {
    let w = button_fit_width(p, label, rc.width(), max_width);
    // Widened in English, where its right edge stays; on screen in RTL its left edge stays and
    // it widens to the right, into the room before it (`rect` is its own inverse).
    let english = rect(rc);
    rect(Rect::from_min_max(
        pos2(english.right() - w, english.top()),
        english.max,
    ))
}

/// How wide a [`button`] must be for its translated `label` to show whole, with the room it
/// keeps inside its border: at least `min` (its English width: English labels fit it, so English
/// never changes), at most `max` (past it the label ends in "..." and shows whole on hover, as on
/// every button).
pub fn button_fit_width(p: &Painter, label: &str, min: f32, max: f32) -> f32 {
    label_width(p, label, min).clamp(min, max.max(min))
}

/// The English x where the translated `caption` of a checkbox whose glyph is at English x `x`
/// ends ([`checkbox_light_until`]'s caption starts 16 px after its glyph): what a control after
/// it may come up to.
pub fn checkbox_caption_end(p: &Painter, x: f32, caption: &str) -> f32 {
    x + 16.0 + text_width(p, crate::i18n::t(caption))
}

/// `s` in one line where it is no wider than `room` px in `font`, else in two, broken where the
/// wider of the two is narrowest: at a space, after a slash or a hyphen, or between two
/// characters of Chinese or Japanese (a title given a second line where a control beside it
/// needs the room). Still one line where it has no such place.
pub fn fit_lines(p: &Painter, font: FontId, s: &str, room: f32) -> Vec<String> {
    let width = |t: &str| text_width_font(p, font.clone(), t);
    if width(s) <= room {
        return vec![s.to_owned()];
    }
    let g: Vec<(usize, &str)> = s.grapheme_indices(true).collect();
    let mut best: Option<(f32, String, String)> = None;
    for k in 1..g.len() {
        let (at, here) = g[k];
        let before = g[k - 1].1;
        let first = |gr: &str| gr.chars().next().unwrap_or(' ');
        let (a, b) = if before == " " {
            (&s[..g[k - 1].0], &s[at..])
        } else if here != " "
            && (before == "/"
                || before == "-"
                || ((cjk(first(before)) || cjk(first(here))) && !here.chars().all(no_line_start)))
        {
            (&s[..at], &s[at..])
        } else {
            continue;
        };
        let (a, b) = (a.trim_end(), b.trim_start());
        if a.is_empty() || b.is_empty() {
            continue;
        }
        let w = width(a).max(width(b));
        if best.as_ref().is_none_or(|(bw, ..)| w < *bw) {
            best = Some((w, a.to_owned(), b.to_owned()));
        }
    }
    match best {
        Some((_, a, b)) => vec![a, b],
        None => vec![s.to_owned()],
    }
}

/// How wide a [`combo_light`] must be for each of its translated `items` to show whole before
/// its chevron (the widest decides, so the box keeps its size as the choice changes): at least
/// `min` (its English width: English items fit it, so English never changes), at most `max`
/// (past it the choice ends in "..." and shows whole on hover, as before).
pub fn combo_fit_width(p: &Painter, items: &[&str], min: f32, max: f32) -> f32 {
    let widest = items
        .iter()
        .map(|item| text_width_font(p, tahoma(), crate::i18n::t(item)))
        .fold(0.0, f32::max);
    // combo_light's room for its label: 5 px before it, 19 px for the chevron after it.
    (widest + 5.0 + 19.0).ceil().clamp(min, max.max(min))
}

/// The width a [`button`] `english` px wide wants for its translated `label`: the label and the
/// room inside the border; no wider than `english` where the label shows whole in it (a label
/// that fits as tightly as some English ones do keeps the button as it is), and narrower where the
/// label needs less.
fn label_width(p: &Painter, label: &str, english: f32) -> f32 {
    let shown = crate::i18n::t(label).replace('&', "");
    let text = text_width(p, &shown);
    let want = (text + 4.0 + 12.0).ceil();
    // Whole on the button as it is ([`fit_label`]): it stays.
    if text <= english - 4.0 {
        want.min(english)
    } else {
        want
    }
}

/// A row of [`button`]s laid out for their translated labels, `gap` apart, the first starting
/// at English x `left`: each `(label, English width)` as wide as [`button_fit_width`] says (at
/// most `max`). Their English (x, width) in the order given, for [`r`]; in English every button
/// keeps its place.
pub fn buttons_from_left(
    p: &Painter,
    left: f32,
    gap: f32,
    buttons: &[(&str, f32)],
    max: f32,
) -> Vec<(f32, f32)> {
    let mut x = left;
    buttons
        .iter()
        .map(|(label, min)| {
            let w = button_fit_width(p, label, *min, max);
            let at = (x, w);
            x += w + gap;
            at
        })
        .collect()
}

/// [`buttons_from_left`] for a row that ends at English x `right` (the last button's right
/// edge): it grows to the left.
pub fn buttons_to_right(
    p: &Painter,
    right: f32,
    gap: f32,
    buttons: &[(&str, f32)],
    max: f32,
) -> Vec<(f32, f32)> {
    let mut x = right + gap;
    let mut out: Vec<(f32, f32)> = buttons
        .iter()
        .rev()
        .map(|(label, min)| {
            let w = button_fit_width(p, label, *min, max);
            x -= gap + w;
            (x, w)
        })
        .collect();
    out.reverse();
    out
}

/// A row of [`button`]s at their English places, `(label, English x, English width)` from left
/// to right (IDM's rows, their gaps not all alike), laid out for their translated labels: the row
/// keeps its right end, and starts no further left than English x `from` (what is beside it).
/// Each button is as wide as [`button_fit_width`] says (at most `max`), widening to the left; one
/// that comes closer than `gap` to the one before it (its English gap, if smaller) pushes it left,
/// and one pushed past `from` pushes those after it back right, each moved no further than it
/// must. When the row would not fit, the buttons wider than their label needs give back that room
/// first (to their label, at least 73 px, Windows' button), then the widest are narrowed, their
/// labels cut with "..." (whole on hover). Their English (x, width) in the order given, for
/// [`r`]; in English every button keeps its place. ([`button_row_from_left`]: a row that keeps
/// its left end.)
pub fn button_row(
    p: &Painter,
    buttons: &[(&str, f32, f32)],
    gap: f32,
    from: f32,
    max: f32,
) -> Vec<(f32, f32)> {
    let row: Vec<(f32, f32, f32)> = buttons
        .iter()
        .map(|(label, x, w)| (*x, *w, label_width(p, label, *w)))
        .collect();
    place_row(&row, gap, from, max)
}

/// [`button_row`] for a row that keeps its left end and ends no further right than English x
/// `to`: its buttons widen to the right.
pub fn button_row_from_left(
    p: &Painter,
    buttons: &[(&str, f32, f32)],
    gap: f32,
    to: f32,
    max: f32,
) -> Vec<(f32, f32)> {
    // The same row seen in a mirror (x → −x): its left end becomes the right one.
    let mirrored: Vec<(f32, f32, f32)> = buttons
        .iter()
        .rev()
        .map(|(label, x, w)| (-(x + w), *w, label_width(p, label, *w)))
        .collect();
    place_row(&mirrored, gap, -to, max)
        .into_iter()
        .rev()
        .map(|(x, w)| (-(x + w), w))
        .collect()
}

/// [`button_row`]'s arithmetic: each button's English (x, width) and the width its label needs.
fn place_row(row: &[(f32, f32, f32)], gap: f32, from: f32, max: f32) -> Vec<(f32, f32)> {
    let n = row.len();
    if n == 0 {
        return Vec::new();
    }
    // Each at least `gap` (or its English gap) from the next.
    let gaps: Vec<f32> = (1..n)
        .map(|i| (row[i].0 - (row[i - 1].0 + row[i - 1].1)).min(gap))
        .collect();
    // As wide as the label needs, never narrower than in English.
    let mut widths: Vec<f32> = row
        .iter()
        .map(|&(_, w, fit)| fit.clamp(w, max.max(w)))
        .collect();
    // Too long for its room even side by side: the overflow taken back one pixel at a time,
    // first from the room labels don't need (down to their label, or 73 px), then from the widest
    // of the widened buttons (down to their English width).
    let right = row[n - 1].0 + row[n - 1].1;
    let room = right - from - gaps.iter().sum::<f32>();
    // Never narrower than this: its English width, or less where its label needs less (73 px
    // at least, Windows' button).
    let least: Vec<f32> = row
        .iter()
        .map(|&(_, w, fit)| w.min(fit.max(73.0)))
        .collect();
    // What its label needs (at least `least`).
    let needs: Vec<f32> = row
        .iter()
        .zip(&least)
        .map(|(&(_, _, fit), &l)| fit.max(l))
        .collect();
    let mut over = (widths.iter().sum::<f32>() - room).ceil();
    // The button with the most room above `low` (or the widest with some), if any; the last
    // first when alike.
    let most = |widths: &[f32], low: &[f32], by_width: bool| -> Option<usize> {
        (0..n)
            .filter(|&i| widths[i] - low[i] >= 1.0)
            .max_by(|&a, &b| {
                let key = |i: usize| {
                    if by_width {
                        widths[i]
                    } else {
                        widths[i] - low[i]
                    }
                };
                key(a).total_cmp(&key(b)).then(a.cmp(&b))
            })
    };
    while over > 0.0 {
        let Some(i) = most(&widths, &needs, false).or_else(|| most(&widths, &least, true)) else {
            break;
        };
        widths[i] -= 1.0;
        over -= 1.0;
    }
    // From the right end: each keeps its English right edge while it can, else stands its gap
    // before the next.
    let mut xs = vec![0.0; n];
    let mut limit = f32::INFINITY;
    for i in (0..n).rev() {
        xs[i] = (row[i].0 + row[i].1).min(limit) - widths[i];
        if i > 0 {
            limit = xs[i] - gaps[i - 1];
        }
    }
    // Then from `from`: those pushed past it come back right, as little as they must.
    let mut least_x = from;
    for i in 0..n {
        xs[i] = xs[i].max(least_x);
        least_x = xs[i] + widths[i] + gaps.get(i).copied().unwrap_or(0.0);
    }
    xs.into_iter().zip(widths).collect()
}

/// A lone [`button`] at English `x`, `w` wide, widened for its translated `label` around its
/// centre, between English x `from` and `to` (what is beside it), moved inwards where it would
/// reach either. Its English (x, width); in English, its place.
pub fn button_around(p: &Painter, label: &str, x: f32, w: f32, from: f32, to: f32) -> (f32, f32) {
    let width = button_fit_width(p, label, w, (to - from).max(w));
    let left = (x + (w - width) / 2.0).floor();
    (left.min(to - width).max(from), width)
}

/// Of a button's labels in turn (Pause and Resume), the one whose translation is the widest: the
/// one to lay the button out for, so it keeps its size as its label changes.
pub fn widest_label<'a>(p: &Painter, labels: &[&'a str]) -> &'a str {
    labels
        .iter()
        .copied()
        .max_by(|a, b| {
            let shown_width = |l: &str| text_width(p, &crate::i18n::t(l).replace('&', ""));
            shown_width(a).total_cmp(&shown_width(b))
        })
        .unwrap_or("")
}

/// `label` as it fits on a button `rc`: whole, or cut with "..." (true) when it would reach the
/// border (English labels fit IDM's buttons, so English never changes).
fn fit_label(p: &Painter, label: &str, rc: Rect) -> (String, bool) {
    let room = rc.width() - 4.0;
    if text_width(p, label) <= room {
        (label.to_owned(), false)
    } else {
        (ellipsize(p, &tahoma(), label, room), true)
    }
}

/// Classic sunken 13 × 13 checkbox with its caption 5 px to the right (baseline box top + 10).
pub fn checkbox(
    ui: &mut Ui,
    box_min: Pos2,
    checked: &mut bool,
    caption: &str,
    enabled: bool,
) -> Response {
    checkbox_until(ui, box_min, checked, caption, enabled, None)
}

/// A checkbox's caption drawn apart from it (two lines, a paragraph) in English `caption`: a
/// click on it ticks the box too, as on a caption the box draws itself.
pub fn caption_click(
    ui: &mut Ui,
    caption: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    checked: &mut bool,
    enabled: bool,
) -> Response {
    let sense = if enabled {
        pointer_only()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(rect(caption), widget_id(ui, ("caption", id)), sense);
    pass_focus_on(ui, &resp, enabled);
    if resp.clicked() {
        *checked = !*checked;
    }
    resp
}

/// [`checkbox`] whose caption also stops before English x `until` (a control after it).
pub fn checkbox_until(
    ui: &mut Ui,
    box_min: Pos2,
    checked: &mut bool,
    caption: &str,
    enabled: bool,
    until: Option<f32>,
) -> Response {
    // Windows 11's check box in both themes (dark mode drew a classic white 3-D box beside the
    // Windows 11 ones of the same window).
    checkbox_light_until(
        ui,
        box_min,
        checked,
        crate::i18n::t(caption),
        enabled,
        until,
    )
}

/// The gap a check box at English `x` leaves in a group box's top line for its caption (English
/// pixels): IDM's measured end (`english_end`) in English, else 2 px past the translated caption
/// (`light`: a [`checkbox_light`], caption at +16; else a [`checkbox`], at +18).
pub fn caption_gap(
    p: &Painter,
    x: f32,
    english_end: f32,
    caption: &str,
    light: bool,
) -> (f32, f32) {
    let shown = crate::i18n::t(caption);
    if shown == caption {
        return (x, english_end);
    }
    // Every check box's caption starts 16 px after it (`light`: kept for its callers).
    let _ = light;
    let lead = 16.0;
    (x, x + lead + text_width(p, shown) + 2.0)
}

/// Group box frame (`rc` on screen) with a gap in its top line for a checkbox caption (`gap` =
/// English x range, inclusive pixels).
pub fn group_box(p: &Painter, rc: Rect, gap: Option<(f32, f32)>) {
    let (l, t, rr, b) = (rc.left(), rc.top(), rc.right() - 1.0, rc.bottom() - 1.0);
    let gap = gap.map(|(g0, g1)| {
        let (a, b) = (mcol(g0), mcol(g1));
        (a.min(b), a.max(b))
    });
    match gap {
        Some((g0, g1)) => {
            hline(p, l, g0 - 1.0, t, GROUP_LINE());
            hline(p, g1 + 1.0, rr, t, GROUP_LINE());
        }
        None => hline(p, l, rr, t, GROUP_LINE()),
    }
    vline(p, l, t, b, GROUP_LINE());
    vline(p, rr, t, b, GROUP_LINE());
    hline(p, l, rr, b, GROUP_LINE());
}

/// The 22 × 21 "+" / "..." buttons.
pub fn small_button(ui: &mut Ui, rc: Rect, glyph: &str) -> Response {
    button(ui, rc, glyph, ButtonKind::Normal, false, None)
}

// ---- light dialogs (Speed Limiter, Scheduler, Options) ------------------------------------
// notes/ui-spec/scheduler-and-misc.md "Light dialog control styles".

crate::theme::themed! {
    /// The "light dialogs" (Options, Scheduler, File Properties…): IDM draws them light in both
    /// modes; here they follow the mode, dark the dark dialogs' colours.
    pub LIGHT_BG => window;
    pub LIGHT_TEXT => text;
    pub LIGHT_DISABLED => text_disabled;
    /// A text box's or a list's inside.
    pub FIELD => field;
    /// A list's or a box's outline.
    pub FIELD_EDGE => list_frame;
    /// Grid lines between a list's rows, and header dividers.
    pub GRID => grid;
    pub HEADER_LINE => header_line;
    /// Etched separators: the shadow line and the highlight beside it.
    pub ETCH_SHADOW => etch_shadow;
    pub ETCH_LIGHT => etch_light;
    /// A selected row in a focused list ([`row_list`]; the download list's colours), white
    /// text on it.
    pub ROW_SELECTED => selection;
    /// Text that opens something ([`link`]).
    pub LINK => link;
    /// A selected row in a list without the focus.
    pub SELECTED_INACTIVE => selection_inactive;
    /// Error text.
    pub ERROR => error;
    /// Secondary text (notes, sizes, versions).
    pub MUTED => muted;
    /// Success text (a matching checksum).
    pub SUCCESS => success;
}

/// Light edit: the same Win11 border, white fill, black text.
pub fn edit_light(
    ui: &mut Ui,
    rc: Rect,
    value: &mut String,
    enabled: bool,
    align_right: bool,
) -> Response {
    edit_light_on(ui, rc, value, enabled, align_right, LIGHT_BG())
}

/// A whole number in `min..=max` typed in a box (the one number box of the dialogs): digits
/// only, no longer than `max` has; Up / Down step it by 1, Page Up / Down by 10; out of range,
/// its frame is red while typed and it comes back into range, visibly, when the box is left.
/// [`number_in`] reads it.
#[allow(clippy::too_many_arguments)]
pub fn number_field(
    ui: &mut Ui,
    rc: Rect,
    value: &mut String,
    min: u64,
    max: u64,
    enabled: bool,
    parent: Color32,
) -> Response {
    let _ = parent;
    edit_frame(ui.painter(), rc, enabled);
    let resp = number_text(ui, rc, value, min, max, enabled);
    let ok = value.parse::<u64>().is_ok_and(|n| (min..=max).contains(&n));
    if enabled && !ok {
        error_frame(ui.painter(), rc);
    }
    edit_focus_line(ui, &resp, rc);
    resp
}

/// A spin box (Windows 11's number box): a whole number in `min..=max` with its up and down
/// arrows inside the box's trailing end, one control (they were two buttons beside a box).
/// Typing, the keys and the range as [`number_field`]; the arrows step by 1, held they repeat.
#[allow(clippy::too_many_arguments)]
pub fn spin_box(
    ui: &mut Ui,
    rc: Rect,
    value: &mut String,
    min: u64,
    max: u64,
    enabled: bool,
    id: &str,
) -> Response {
    let (resp, step) = spin_frame(ui, rc, enabled, id, |ui, text| {
        number_text(ui, text, value, min, max, enabled)
    });
    if step != 0 {
        let n = value.parse::<i64>().unwrap_or(min as i64) + step as i64;
        *value = n.clamp(min as i64, max as i64).to_string();
    }
    let ok = value.parse::<u64>().is_ok_and(|n| (min..=max).contains(&n));
    if enabled && !ok {
        error_frame(ui.painter(), rc);
    }
    resp
}

/// A spin box's frame and arrows round `content` (its text, drawn in the room left of the
/// arrows: given that rect): the one look of every box with arrows (numbers, a letter, a
/// time). Returns the content's response and the arrows' step (+1, −1, 0).
pub fn spin_frame(
    ui: &mut Ui,
    rc: Rect,
    enabled: bool,
    id: &str,
    content: impl FnOnce(&mut Ui, Rect) -> Response,
) -> (Response, i32) {
    edit_frame(ui.painter(), rc, enabled);
    let zone = spin_zone(rc);
    let text = if rtl() {
        Rect::from_min_max(pos2(zone.right(), rc.top()), rc.max)
    } else {
        Rect::from_min_max(rc.min, pos2(zone.left(), rc.bottom()))
    };
    let resp = content(ui, text);
    let step = spin_arrows(ui, zone, enabled, id);
    edit_focus_line(ui, &resp, rc);
    (resp, step)
}

/// The arrows' place inside a spin box (on screen): its trailing 18 px within the border (the
/// leading ones in a right-to-left interface).
pub fn spin_zone(rc: Rect) -> Rect {
    if rtl() {
        Rect::from_min_max(
            pos2(rc.left() + 1.0, rc.top() + 1.0),
            pos2(rc.left() + 19.0, rc.bottom() - 1.0),
        )
    } else {
        Rect::from_min_max(
            pos2(rc.right() - 19.0, rc.top() + 1.0),
            pos2(rc.right() - 1.0, rc.bottom() - 1.0),
        )
    }
}

/// A plain line of text in a box drawn by its caller (a spin box's letter): the text boxes'
/// typing, caret and menu without a frame of its own.
pub fn field_text(ui: &mut Ui, rc: Rect, value: &mut String, enabled: bool) -> Response {
    let color = if enabled {
        LIGHT_TEXT()
    } else {
        LIGHT_DISABLED()
    };
    edit_text_with(ui, rc, value, enabled, false, 5.0, color, start_align())
}

/// A number box's text and keys: digits only, no longer than `max` has; Up / Down step it by 1,
/// Page Up / Down by 10; left, it comes back into `min..=max`.
fn number_text(
    ui: &mut Ui,
    rc: Rect,
    value: &mut String,
    min: u64,
    max: u64,
    enabled: bool,
) -> Response {
    let resp = field_text(ui, rc, value, enabled);
    value.retain(|c| c.is_ascii_digit());
    value.truncate(max.to_string().len());
    if resp.has_focus() {
        keep_keys(ui, &resp, false, true, false);
        let n = value.parse::<u64>().unwrap_or(min) as i64;
        let step = if take_key(ui, egui::Key::ArrowUp) {
            1
        } else if take_key(ui, egui::Key::ArrowDown) {
            -1
        } else if take_key(ui, egui::Key::PageUp) {
            10
        } else if take_key(ui, egui::Key::PageDown) {
            -10
        } else {
            0
        };
        if step != 0 {
            *value = (n + step).clamp(min as i64, max as i64).to_string();
        }
    }
    if resp.lost_focus() {
        *value = number_in(value, min, max).to_string();
    }
    resp
}

/// The number in a [`number_field`], brought into `min..=max` (empty: `min`).
pub fn number_in(value: &str, min: u64, max: u64) -> u64 {
    value.parse::<u64>().unwrap_or(min).clamp(min, max)
}

/// [`edit_light`] on a surface `parent` (a disabled edit shows it: `#f9f9f9` on a tab page).
pub fn edit_light_on(
    ui: &mut Ui,
    rc: Rect,
    value: &mut String,
    enabled: bool,
    align_right: bool,
    parent: Color32,
) -> Response {
    edit_light_with(ui, rc, value, enabled, align_right, false, parent)
}

fn edit_light_with(
    ui: &mut Ui,
    rc: Rect,
    value: &mut String,
    enabled: bool,
    align_right: bool,
    password: bool,
    parent: Color32,
) -> Response {
    // `parent`: the surface a box sits on (kept for its callers; the frame paints the inside).
    let _ = parent;
    edit_frame(ui.painter(), rc, enabled);
    let color = if enabled {
        LIGHT_TEXT()
    } else {
        LIGHT_DISABLED()
    };
    // A number keeps to the end (the left in RTL), text to the start.
    let align = if align_right {
        end_align()
    } else {
        start_align()
    };
    let resp = edit_text_with(ui, rc, value, enabled, password, 5.0, color, align);
    edit_focus_line(ui, &resp, rc);
    resp
}

/// A light password box: dots, and no Cut or Copy in its menu.
pub fn password_light(ui: &mut Ui, rc: Rect, value: &mut String, enabled: bool) -> Response {
    password_light_on(ui, rc, value, enabled, LIGHT_BG())
}

/// [`password_light`] on a surface `parent` (shown inside it while disabled).
pub fn password_light_on(
    ui: &mut Ui,
    rc: Rect,
    value: &mut String,
    enabled: bool,
    parent: Color32,
) -> Response {
    edit_light_with(ui, rc, value, enabled, false, true, parent)
}

/// A multi-line text box (lists of sites, notes) that wraps its text and scrolls, with the
/// shared frame, caret, focus line and edit menu. `id` keeps its scroll place.
/// `enabled`: off, it is greyed and can't be typed in (its switch is off).
pub fn multiline_light(
    ui: &mut Ui,
    rc: Rect,
    value: &mut String,
    id: &str,
    enabled: bool,
) -> Response {
    edit_frame(ui.painter(), rc, enabled);
    let ink = if enabled {
        LIGHT_TEXT()
    } else {
        LIGHT_DISABLED()
    };
    // The text left of the shared scroll bar (egui's own was a thin overlay unlike every other
    // list's), which sits inside the frame when the text is taller than the box.
    let track = Rect::from_min_max(
        pos2(rc.right() - 1.0 - crate::scrollbar::THICK, rc.top() + 1.0),
        pos2(rc.right() - 1.0, rc.bottom() - 1.0),
    );
    let inner = Rect::from_min_max(
        rc.min + vec2(4.0, 2.0),
        pos2(track.left() - 2.0, rc.bottom() - 3.0),
    );
    let off_key = egui::Id::unique(("multiline-scroll", id));
    let mut off: f32 = ui.data(|d| d.get_temp(off_key)).unwrap_or(0.0);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt(id));
    // Inside the frame only (the last line showed under the box, over the control below).
    child.set_clip_rect(rc.shrink(2.0));
    child.visuals_mut().text_cursor.stroke.color = LIGHT_TEXT();
    if !enabled {
        child.disable();
    }
    let out = egui::ScrollArea::vertical()
        .id_salt((id, "scroll"))
        .max_height(inner.height())
        // egui's least height (64) is taller than most boxes.
        .min_scrolled_height(inner.height())
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .scroll_source(egui::scroll_area::ScrollSource {
            scroll_bar: false,
            mouse_wheel: false,
            ..Default::default()
        })
        .vertical_scroll_offset(off)
        .show(&mut child, |ui| {
            let resp = ui.add(
                egui::TextEdit::multiline(value)
                    .font(tahoma())
                    .text_color(ink)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .desired_width(inner.width())
                    .desired_rows(3),
            );
            edit_menu(&resp, value, false, true);
            // Enter here is a new line, not the dialog's OK.
            if resp.has_focus() {
                take_key(ui, egui::Key::Enter);
            }
            resp
        });
    let resp = out.inner;
    // Where the caret took it (typing past the bottom), then the wheel and the bar.
    off = out.state.clamped_offset().y;
    let (total, page) = (out.content_size.y, inner.height());
    let line = ui.fonts_mut(|f| f.row_height(&tahoma()));
    if ui.rect_contains_pointer(rect(rc)) {
        off += wheel_rows(ui, line, (page / line).floor().max(1.0)) * line;
    }
    if total > page && enabled {
        crate::scrollbar::bar(
            ui,
            off_key.with("bar"),
            track,
            true,
            total,
            page,
            line,
            &mut off,
        );
    }
    let off_now = off.clamp(0.0, (total - page).max(0.0));
    if off_now != out.state.clamped_offset().y {
        ui.ctx().request_repaint();
    }
    ui.data_mut(|d| d.insert_temp(off_key, off_now));
    edit_focus_line(ui, &resp, rc);
    resp
}

/// The baseline (last row of the capitals) of Tahoma −11 text centred in a control `height` tall
/// at `top`: its 8-row capitals start at `top + floor((height − 8) / 2)`. At 21 px that is the
/// measured `top + 13` of Windows' controls; every painter that centres a line of text in its box
/// (buttons, text boxes, read-only fields) uses it, so they agree at any height (a fixed `+ 13`
/// put text 1–3 px high in the 22–26 px ones).
pub fn baseline_in(top: f32, height: f32) -> f32 {
    top + ((height - 8.0) / 2.0).floor() + 7.0
}

/// A button label's baseline below the button's top: as measured on Windows' buttons at their
/// usual heights (13), centred ([`baseline_in`]) in taller ones; both modes alike.
fn button_baseline(height: f32) -> f32 {
    match height {
        h if h <= 21.0 => 13.0,
        h => baseline_in(0.0, h),
    }
}

/// Light push button: #fdfdfd fill, radius 3; border #d0d0d0 (bottom #bababa), default
/// #0078d4 (bottom #006bbe), disabled #e9e9e9 with #838383 text; dotted focus rect.
pub fn button_light(
    ui: &mut Ui,
    rc: Rect,
    label: &str,
    kind: ButtonKind,
    focus_rect: bool,
) -> Response {
    button_face(ui, rc, crate::i18n::t(label), kind, focus_rect)
}

/// How wide a light button must be for its (translated) `label`: at least `min` (Windows'
/// 73 px), else the label and its margins, so a longer translation is never cut.
pub fn button_width(p: &Painter, label: &str, min: f32) -> f32 {
    (text_width(p, crate::i18n::t(label)) + 24.0)
        .ceil()
        .max(min)
}

/// Light buttons side by side, the row ending at English x `right` (window-relative to `o`),
/// each as wide as [`button_width`] says, 8 px apart, in the order given. Their responses, in
/// that order.
pub fn buttons_light_right(
    ui: &mut Ui,
    o: Pos2,
    right: f32,
    top: f32,
    buttons: &[(&str, ButtonKind)],
) -> Vec<Response> {
    let p = ui.painter().clone();
    let widths: Vec<f32> = buttons
        .iter()
        .map(|(l, _)| button_width(&p, l, 73.0))
        .collect();
    let mut x = right - widths.iter().sum::<f32>() - 8.0 * (buttons.len().saturating_sub(1)) as f32;
    let mut out = Vec::new();
    for ((label, kind), w) in buttons.iter().zip(widths) {
        out.push(button_light(ui, r(o, x, top, w, 21.0), label, *kind, false));
        x += w + 8.0;
    }
    out
}

/// A push button in the current theme (dark: IDM's dark buttons; light: Windows 11's), its
/// (translated) `label` with "&" before the access letter: the letter is underlined and
/// Alt+letter presses it. Hot and pressed as Windows 11 draws them: a pressed button's label
/// dims and its darker bottom edge flattens. `default_focus`: the dialog's default button, whose
/// focus rectangle shows while no other control has the keyboard focus; any control with the
/// focus shows it.
fn button_face(
    ui: &mut Ui,
    rc: Rect,
    label: &str,
    kind: ButtonKind,
    default_focus: bool,
) -> Response {
    let enabled = kind != ButtonKind::Disabled;
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    // Keyed by its place, not its label: Pause becoming Resume keeps the focus.
    let resp = ui.interact(
        rc,
        widget_id(ui, ("button", rc.min.x as i32, rc.min.y as i32)),
        sense,
    );
    pass_focus_on(ui, &resp, enabled);
    expose(
        &resp,
        egui::Role::Button,
        crate::i18n::t(label),
        enabled,
        None,
    );
    focus_on_press(&resp);
    // A focused button answers Enter itself (egui clicks it): the dialog's default button,
    // read after it, doesn't run as well.
    if resp.has_focus() {
        take_key(ui, egui::Key::Enter);
    }
    let state = State::of(&resp, enabled);
    let p = ui.painter();
    let face = button_colors(kind, state, crate::theme::colors());
    let radius = paint_button(p, rc, kind, state, &face, Shape::Push);
    // "&Start now": the letter after '&' is underlined and Alt+letter presses it.
    let (shown, mnemonic) = match label.split_once('&') {
        Some((before, after)) if !after.is_empty() => (format!("{before}{after}"), Some(before)),
        _ => (label.to_owned(), None),
    };
    // A translation wider than the button ends in "..." and shows whole on hover (UI rule).
    let (shown, cut) = fit_label(p, &shown, rc);
    let mnemonic = mnemonic.filter(|_| !cut);
    let w = text_width(p, &shown);
    let x = (rc.center().x - w / 2.0).round();
    // "..." sits on the baseline (no capitals above it): the dots themselves centred.
    let baseline = if shown.trim() == "..." {
        rc.top() + (rc.height() / 2.0).round() + 1.0
    } else {
        rc.top() + button_baseline(rc.height())
    };
    // Centred in a rect on screen: placed by its left, not by an English pen.
    let drawn = text_at(p, tahoma(), x, baseline, &shown, face.text);
    let mut pressed_by_key = false;
    if let Some(before) = mnemonic
        && let Some(letter) = shown[before.len()..].chars().next()
    {
        // Underlined as the menus are (always dark; light once the keyboard is in use), while
        // Alt+letter always works.
        if crate::chrome::cues(ui.ctx()) {
            let (x0, x1) = char_span(p, &tahoma(), &shown, before.chars().count(), drawn);
            hline(p, x0, x1, baseline + 3.0, face.text);
        }
        pressed_by_key = enabled && mnemonic_pressed(ui, letter);
    }
    let resp = if pressed_by_key {
        let mut r = resp;
        r.flags.insert(egui::response::Flags::CLICKED);
        r
    } else {
        resp
    };
    if resp.has_focus() || (default_focus && ui.ctx().memory(|m| m.focused().is_none())) {
        focus_ring_round(ui.painter(), rc.shrink(2.0), (radius - 2.0).max(0.0));
    }
    if cut {
        resp.on_hover_text(label.replace('&', ""))
    } else {
        resp
    }
}

/// A push button's face, edge and bottom line in the theme's button style (`palette::ButtonLook`):
/// flat (Windows 11, KDE), or shaded toward the theme's `gloss` (Luna, Aero, Aqua), as `shape`
/// says it is used. Every button and every part drawn as one comes here. Its corners' radius.
fn paint_button(
    p: &Painter,
    rc: Rect,
    kind: ButtonKind,
    state: State,
    face: &Face,
    shape: Shape,
) -> f32 {
    use crate::palette::ButtonLook;
    let c = crate::theme::colors();
    let m = crate::theme::metrics();
    let look = crate::theme::current().controls.button;
    let part = shape != Shape::Push;
    let radius = match look {
        _ if shape == Shape::DropDown => control_radius(),
        _ if part => control_radius().min((rc.height() / 3.0).floor()),
        ButtonLook::Fluent | ButtonLook::Breeze => control_radius(),
        ButtonLook::Luna | ButtonLook::Aero => m.button_radius,
        // Mac OS X's push buttons are round at the ends.
        ButtonLook::Aqua => (rc.height() / 2.0).floor(),
    };
    let Some(shine) = Shine::of_button(look) else {
        p.rect_filled(rc, radius, face.fill);
        p.rect_stroke(
            rc,
            radius,
            Stroke::new(m.border, face.border),
            StrokeKind::Inside,
        );
        // The bottom edge: a push button's and a drop-down's 3 points in from its ends, a small
        // part's from its corners (none on one too low for it).
        if shape != Shape::Small || rc.height() >= 14.0 {
            let inset = if shape == Shape::Small { radius } else { 3.0 };
            hline(
                p,
                rc.left() + inset,
                rc.right() - inset - 1.0,
                rc.bottom() - 1.0,
                face.bottom,
            );
        }
        return radius;
    };
    fill_shaded(p, rc, radius, &shine.stops(face.fill, c.gloss));
    // Windows XP: a glow inside the edge, orange under the pointer, blue on the default button
    // (its own border's colour; the edge itself stays dark).
    let mut border = face.border;
    if look == ButtonLook::Luna && state != State::Disabled {
        let glow = match (state, kind) {
            (State::Hot, _) => Some(c.button_glow),
            (State::Pressed, _) => None,
            (_, ButtonKind::Default) => {
                border = c.button_border;
                Some(face.border)
            }
            _ => None,
        };
        if let Some(glow) = glow.filter(|g| g.a() > 0) {
            p.rect_stroke(
                rc.shrink(m.border),
                (radius - m.border).max(0.0),
                Stroke::new(2.0, glow),
                StrokeKind::Inside,
            );
        }
    }
    p.rect_stroke(
        rc,
        radius,
        Stroke::new(m.border, border),
        StrokeKind::Inside,
    );
    radius
}

/// What a face drawn by [`paint_button`] is: a push button, a small button-like part (a spin
/// box's halves, an icon button: its corners no rounder than a third of its height) or a
/// drop-down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Push,
    Small,
    DropDown,
}

/// A button's colours.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Face {
    fill: Color32,
    border: Color32,
    /// The bottom edge (a darker line at rest; flat, the border's own, while pressed).
    bottom: Color32,
    text: Color32,
}

/// A button's colours for `kind` in `state`, from `c` (a theme's variant). The states are
/// decided here, once for every button: the pressed one flattens its bottom edge.
fn button_colors(kind: ButtonKind, state: State, c: &crate::palette::Palette) -> Face {
    button_colors_in(kind, state, c, crate::theme::current().controls.button)
}

/// [`button_colors`] in the button style `look`.
fn button_colors_in(
    kind: ButtonKind,
    state: State,
    c: &crate::palette::Palette,
    look: crate::palette::ButtonLook,
) -> Face {
    let pressed = state == State::Pressed;
    // Mac OS X: the default button is the accent's gel, its edge a shade darker.
    if look == crate::palette::ButtonLook::Aqua && kind == ButtonKind::Default {
        let face = button_colors_in(ButtonKind::Accent, state, c, look);
        return Face {
            border: crate::derive::shade(face.fill, -0.12),
            ..face
        };
    }
    if kind == ButtonKind::Accent {
        // The accent filled, a shade lighter under the pointer and darker pressed.
        let (fill, text) = match state {
            State::Pressed => (c.button_accent_pressed, c.button_accent_text_pressed),
            State::Hot => (c.button_accent_hot, c.button_accent_text),
            _ => (c.button_accent, c.button_accent_text),
        };
        return Face {
            fill,
            border: fill,
            bottom: if pressed {
                fill
            } else {
                c.button_accent_bottom
            },
            text,
        };
    }
    // Windows 11's buttons: a face a little lighter than the window in dark, a quiet border,
    // the default button outlined in the accent.
    let (border, bottom) = match kind {
        ButtonKind::Normal => (c.button_border, c.button_bottom),
        ButtonKind::Default | ButtonKind::Accent => {
            (c.button_default_border, c.button_default_bottom)
        }
        ButtonKind::Disabled => (c.button_disabled_border, c.button_disabled_border),
    };
    // KDE and Windows 7: the edge in the accent under the pointer and pressed (its face tinted,
    // by the theme's colours).
    let breeze = matches!(
        look,
        crate::palette::ButtonLook::Breeze | crate::palette::ButtonLook::Aero
    );
    let (border, bottom) = if breeze && matches!(state, State::Hot | State::Pressed) {
        (c.accent, c.accent)
    } else {
        (border, bottom)
    };
    let face = Face {
        fill: match state {
            State::Disabled => c.button_disabled,
            State::Pressed => c.button_pressed,
            State::Hot => c.button_hot,
            _ => c.button,
        },
        border,
        bottom,
        text: match state {
            State::Disabled => c.text_disabled,
            State::Pressed => c.button_text_pressed,
            _ => c.button_text,
        },
    };
    if pressed {
        Face {
            bottom: face.border,
            ..face
        }
    } else {
        face
    }
}

/// Alt+`letter` was pressed (and is taken): Latin letters as egui keys, other scripts (Cyrillic,
/// Greek, Arabic…) as the text typed with Alt held.
pub fn mnemonic_pressed(ui: &Ui, letter: char) -> bool {
    let upper: String = letter.to_uppercase().collect();
    if let Some(key) = egui::Key::from_name(&upper)
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::ALT, key))
    {
        return true;
    }
    let lower: String = letter.to_lowercase().collect();
    ui.input_mut(|i| {
        let alt = i.modifiers.alt;
        let at = i
            .events
            .iter()
            .position(|e| matches!(e, egui::Event::Text(t) if alt && t.to_lowercase() == lower));
        at.map(|k| i.events.remove(k)).is_some()
    })
}

/// Enter, without a modifier, that no control took first: a dialog's default button. A control
/// that answers Enter itself (an open list, a multi-line box) takes it with [`take_key`] before
/// the dialog asks.
pub fn enter_pressed(ui: &Ui) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.is_none())
}

/// Escape that no control took first (an open list or picker closes itself): a dialog's Cancel.
pub fn escape_pressed(ui: &Ui) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Escape))
}

/// Take `key` (without modifiers) for a control that answers it, so the dialog under it doesn't
/// too: Escape closes an open list, not the window; Enter in a multi-line box is a new line.
pub fn take_key(ui: &Ui, key: egui::Key) -> bool {
    ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key))
}

/// A part clicked by the pointer that never takes the keyboard focus (egui's `click()` is also
/// a Tab stop): spinner arrows, a combo's arrow, scroll bars, toolbar and menu-bar items, rows
/// of a list that handles its own keys. Tab then only reaches real controls.
pub fn pointer_only() -> Sense {
    Sense::CLICK
}

/// [`pointer_only`] that can also be dragged (a scroll bar's thumb).
pub fn pointer_drag() -> Sense {
    Sense::CLICK | Sense::DRAG
}

/// The ring of `resp`'s control around `rc` while it has the keyboard focus.
pub fn show_focus(ui: &Ui, resp: &Response, rc: Rect) {
    if resp.has_focus() {
        focus_ring(ui.painter(), rc);
    }
}

/// Is the keyboard in use in the current window? A navigation key (Tab, the arrows, Space,
/// Enter, Home / End, Page Up / Down) turns it on, a mouse press off: focus rings show only
/// for a user moving with the keys, as Windows shows them.
pub fn keyboard_in_use(ctx: &egui::Context) -> bool {
    use egui::Key as K;
    let key = egui::Id::unique(("keyboard-in-use", ctx.viewport_id()));
    let (keys, press) = ctx.input(|i| {
        let nav = i.events.iter().any(|e| {
            matches!(
                e,
                egui::Event::Key {
                    key: K::Tab
                        | K::ArrowUp
                        | K::ArrowDown
                        | K::ArrowLeft
                        | K::ArrowRight
                        | K::Space
                        | K::Enter
                        | K::Home
                        | K::End
                        | K::PageUp
                        | K::PageDown
                        | K::F6,
                    pressed: true,
                    ..
                }
            )
        });
        (nav, i.pointer.any_pressed())
    });
    let mut on = ctx.data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
    if keys {
        on = true;
    } else if press {
        on = false;
    }
    ctx.data_mut(|d| d.insert_temp(key, on));
    on
}

/// The keyboard focus around `rc`: a dotted rectangle, every other pixel, in the theme's ink.
pub fn focus_ring(p: &Painter, rc: Rect) {
    focus_ring_round(p, rc, control_radius());
}

/// [`focus_ring`] whose ring or frame (where the theme draws one) has corners of `radius`: a
/// control rounded otherwise than the others (Mac OS X's push buttons, round at the ends).
pub fn focus_ring_round(p: &Painter, rc: Rect, radius: f32) {
    // Only once the keyboard is in use in this window, as Windows shows focus rectangles.
    if !keyboard_in_use(p.ctx()) {
        return;
    }
    let ink = crate::theme::colors().focus_ring;
    // GNOME: a rounded ring, as wide as two points, the controls' corners.
    match crate::theme::current().controls.focus {
        crate::palette::FocusLook::Ring => {
            p.rect_stroke(rc, radius, Stroke::new(2.0, ink), StrokeKind::Inside);
            return;
        }
        // KDE: a frame one point wide.
        crate::palette::FocusLook::Frame => {
            p.rect_stroke(rc, radius, Stroke::new(1.0, ink), StrokeKind::Inside);
            return;
        }
        crate::palette::FocusLook::Dotted => {}
    }
    let (l, t, r, b) = (rc.left(), rc.top(), rc.right() - 1.0, rc.bottom() - 1.0);
    let mut x = l;
    while x <= r {
        px(p, x, t, ink);
        px(p, x, b, ink);
        x += 2.0;
    }
    let mut y = t;
    while y <= b {
        px(p, l, y, ink);
        px(p, r, y, ink);
        y += 2.0;
    }
}

/// Win11 light checkbox glyph (13 × 13) with its label at glyph.x + 16 (baseline glyph top + 10).
pub fn checkbox_light(
    ui: &mut Ui,
    glyph: Pos2,
    checked: &mut bool,
    caption: &str,
    enabled: bool,
) -> Response {
    checkbox_light_until(ui, glyph, checked, caption, enabled, None)
}

/// [`checkbox_light`] whose caption must end before English x `until` (a box after it on the
/// same row).
pub fn checkbox_light_until(
    ui: &mut Ui,
    glyph: Pos2,
    checked: &mut bool,
    caption: &str,
    enabled: bool,
    until: Option<f32>,
) -> Response {
    let caption = crate::i18n::t(caption);
    let p = ui.painter().clone();
    // A caption longer than its room ends in "..." at the edge, whole on hover (UI rule).
    let (shown, cut) = fit_caption_until(&p, glyph.x + 16.0, caption, until);
    let w = text_width(&p, &shown);
    // `glyph` is English; the glyph and its caption (after it) are placed on screen.
    let english = glyph;
    let hit = rect(Rect::from_min_size(english, vec2(16.0 + w + 2.0, 13.0)));
    let glyph = at(english, 13.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(
        hit,
        widget_id(ui, ("check-light", glyph.x as i32, glyph.y as i32)),
        sense,
    );
    pass_focus_on(ui, &resp, enabled);
    let resp = enter_is_not_a_click(ui, resp);
    focus_on_press(&resp);
    show_focus(ui, &resp, hit.expand(1.0));
    if resp.clicked() {
        *checked = !*checked;
    }
    expose(
        &resp,
        egui::Role::CheckBox,
        caption,
        enabled,
        Some(*checked),
    );
    let rc = Rect::from_min_size(glyph, vec2(13.0, 13.0));
    check_box_glyph(&p, rc, *checked, State::of(&resp, enabled));
    let color = if enabled {
        LIGHT_TEXT()
    } else {
        LIGHT_DISABLED()
    };
    text(&p, english.x + 16.0, glyph.y + 10.0, &shown, color);
    if cut {
        resp.on_hover_text(caption)
    } else {
        resp
    }
}

crate::theme::themed! {
    /// Tab page body.
    pub LIGHT_PAGE => tab_page;
    /// An unselected tab, and the page's shadow.
    pub TAB_UNSELECTED => tab;
    pub TAB_HOT => tab_hot;
    CHEVRON => chevron;
    CHEVRON_DISABLED => chevron_disabled;
}

/// A Windows 11 check box's 13 × 13 box in `rc`, ticked or not, in `state`: the one look of
/// check boxes and of the check lists (Options → Browsers, Sounds).
/// A check box in a list's row ([`row_list`]): its 13 × 13 box at English `at`, ticked by a
/// click (hot and pressed as any check box), not a Tab stop of its own (the list is: Space
/// ticks the selected row). `salt` names the item, so the box stays its own while the list
/// scrolls. True when clicked.
pub fn row_check(
    ui: &Ui,
    p: &Painter,
    at: Pos2,
    on: &mut bool,
    label: &str,
    salt: impl std::hash::Hash + std::fmt::Debug,
) -> bool {
    let rc = rect(Rect::from_min_size(at, vec2(13.0, 13.0)));
    let resp = ui.interact(
        rc.expand(2.0),
        widget_id(ui, ("row-check", salt)),
        pointer_only(),
    );
    if resp.clicked() {
        *on = !*on;
    }
    // Named by its row's item, ticked or not (#95).
    expose(&resp, egui::Role::CheckBox, label, true, Some(*on));
    check_box_glyph(p, rc, *on, State::of(&resp, true));
    resp.clicked()
}

/// An era theme's check box (or, `round`, radio button) in `rc` (13 × 13), `on` when checked:
/// Windows XP's, Windows 7's or Mac OS X's (`palette::CheckLook`). False, drawing nothing, for
/// the other looks.
fn era_check(p: &Painter, rc: Rect, on: bool, state: State, round: bool) -> bool {
    use crate::derive::mix;
    use crate::palette::CheckLook;
    let look = crate::theme::current().controls.check;
    if !matches!(look, CheckLook::Luna | CheckLook::Aero | CheckLook::Aqua) {
        return false;
    }
    let c = crate::theme::colors();
    let m = crate::theme::metrics();
    let radius = if round { 6.5 } else { m.check_radius };
    let disabled = state == State::Disabled;
    let (fill_c, edge) = fluent_box(state);
    let fill_c = fill_c.unwrap_or(c.field_disabled);
    let stroke = |rc: Rect, r: f32, ink: Color32| {
        p.rect_stroke(rc, r, Stroke::new(m.border, ink), StrokeKind::Inside);
    };
    // The mark: a tick, or a radio's dot (shaded as the theme's gloss shades).
    let mark = |ink: Color32, width: f32| {
        if round {
            let dot = Rect::from_center_size(rc.center(), vec2(5.0, 5.0));
            fill_shaded(p, dot, 2.5, &[(0.0, mix(ink, c.gloss, 0.45)), (1.0, ink)]);
        } else {
            p.line(
                vec![
                    pos2(rc.left() + 3.0, rc.top() + 6.0),
                    pos2(rc.left() + 5.5, rc.top() + 8.5),
                    pos2(rc.left() + 10.0, rc.top() + 3.5),
                ],
                Stroke::new(width, ink),
            );
        }
    };
    match look {
        // Windows XP: a dark edge, the inside shaded from its top, an orange glow inside under
        // the pointer; a thick tick (a dot) in the checked colour.
        CheckLook::Luna => {
            let inner = rc.shrink(m.border);
            fill_shaded(
                p,
                inner,
                (radius - m.border).max(0.0),
                &[(0.0, mix(fill_c, edge, 0.18)), (1.0, fill_c)],
            );
            if state == State::Hot && c.button_glow.a() > 0 {
                stroke(inner, (radius - 1.0).max(0.0), c.button_glow);
                stroke(inner.shrink(1.0), (radius - 2.0).max(0.0), c.button_glow);
            }
            stroke(rc, radius, edge);
            if on {
                let ink = if disabled {
                    c.check_on_disabled
                } else {
                    fluent_accent(state)
                };
                mark(ink, 2.5);
            }
        }
        // Windows 7: a quiet edge (the accent's under the pointer), a white margin and a
        // shaded square inside; the tick in the mark's colour.
        CheckLook::Aero => {
            p.rect_filled(rc, radius, c.field);
            stroke(rc, radius, edge);
            if !round || !on {
                let inner = rc.shrink(3.0);
                fill_shaded(
                    p,
                    inner,
                    if round { 3.5 } else { 0.0 },
                    &[(0.0, mix(fill_c, edge, 0.35)), (1.0, fill_c)],
                );
            }
            if on {
                let ink = if disabled {
                    c.check_on_disabled
                } else if round {
                    fluent_accent(state)
                } else {
                    c.check_mark
                };
                mark(ink, 2.0);
            }
        }
        // Mac OS X: a glossy box, round corners; checked, the accent's gel and a dark mark.
        CheckLook::Aqua => {
            let face = if on && !disabled {
                fluent_accent(state)
            } else {
                fill_c
            };
            fill_shaded(p, rc, radius, &Shine::Aqua.stops(face, c.gloss));
            let edge = if on && !disabled {
                crate::derive::shade(face, -0.15)
            } else {
                edge
            };
            stroke(rc, radius, edge);
            if on {
                let ink = if disabled {
                    c.check_on_disabled
                } else {
                    c.check_mark
                };
                if round {
                    p.circle_filled(rc.center(), 2.5, ink);
                } else {
                    mark(ink, 2.0);
                }
            }
        }
        _ => {}
    }
    true
}

pub fn check_box_glyph(p: &Painter, rc: Rect, checked: bool, state: State) {
    if era_check(p, rc, checked, state, false) {
        return;
    }
    // KDE: the box's edge in the accent under the pointer; checked, the accent's edge around
    // its inside tinted with it, a tick in the text's colour.
    if crate::theme::current().controls.check == crate::palette::CheckLook::Breeze {
        let m = crate::theme::metrics();
        let (fill_c, border) = breeze_box(state, checked);
        if let Some(f) = fill_c {
            p.rect_filled(rc, m.check_radius, f);
        }
        p.rect_stroke(
            rc,
            m.check_radius,
            Stroke::new(m.border, border),
            StrokeKind::Inside,
        );
        if checked {
            p.line(
                vec![
                    pos2(rc.left() + 3.0, rc.top() + 6.5),
                    pos2(rc.left() + 5.5, rc.top() + 9.0),
                    pos2(rc.left() + 10.0, rc.top() + 4.0),
                ],
                Stroke::new(2.0, breeze_mark(state)),
            );
        }
        return;
    }
    // GNOME: a 2-point border at rest (its inside tinted under the pointer), the accent filled
    // with a light tick, 2 points wide, when checked.
    if crate::theme::current().controls.check == crate::palette::CheckLook::Adwaita {
        let m = crate::theme::metrics();
        if checked {
            p.rect_filled(rc, m.check_radius, fluent_accent(state));
            p.line(
                vec![
                    pos2(rc.left() + 3.0, rc.top() + 6.5),
                    pos2(rc.left() + 5.5, rc.top() + 9.0),
                    pos2(rc.left() + 10.0, rc.top() + 4.0),
                ],
                Stroke::new(2.0, fluent_mark()),
            );
        } else {
            let (fill_c, border) = fluent_box(state);
            if let Some(f) = fill_c {
                p.rect_filled(rc, m.check_radius, f);
            }
            p.rect_stroke(
                rc,
                m.check_radius,
                Stroke::new(2.0, border),
                StrokeKind::Inside,
            );
        }
        return;
    }
    // Windows 8: the box keeps its inside and border (the border in the accent under the
    // pointer), a tick in the accent.
    if crate::theme::current().controls.check == crate::palette::CheckLook::Square {
        let (fill_c, border) = fluent_box(state);
        let m = crate::theme::metrics();
        if let Some(f) = fill_c {
            p.rect_filled(rc, m.check_radius, f);
        }
        let border = if matches!(state, State::Hot | State::Pressed) {
            accent()
        } else {
            border
        };
        p.rect_stroke(
            rc,
            m.check_radius,
            Stroke::new(m.border, border),
            StrokeKind::Inside,
        );
        if checked {
            let ink = if state == State::Disabled {
                crate::theme::colors().check_on_disabled
            } else {
                fluent_accent(state)
            };
            p.line(
                vec![
                    pos2(rc.left() + 2.5, rc.top() + 6.5),
                    pos2(rc.left() + 5.5, rc.top() + 9.5),
                    pos2(rc.left() + 10.5, rc.top() + 3.0),
                ],
                Stroke::new(2.0, ink),
            );
        }
        return;
    }
    if checked {
        p.rect_filled(
            rc,
            crate::theme::metrics().check_radius,
            fluent_accent(state),
        );
        p.line(
            vec![
                pos2(rc.left() + 3.0, rc.top() + 6.5),
                pos2(rc.left() + 5.5, rc.top() + 9.0),
                pos2(rc.left() + 10.0, rc.top() + 3.5),
            ],
            Stroke::new(1.4, fluent_mark()),
        );
    } else {
        let (fill_c, border) = fluent_box(state);
        let m = crate::theme::metrics();
        if let Some(f) = fill_c {
            p.rect_filled(rc, m.check_radius, f);
        }
        p.rect_stroke(
            rc,
            m.check_radius,
            Stroke::new(m.border, border),
            StrokeKind::Inside,
        );
    }
}

/// A Windows 11 check box that is partly ticked (a folder with some of its files chosen): the
/// accent box with a dash, as Windows draws it (a square sat on the unticked box before).
pub fn check_box_mixed(p: &Painter, rc: Rect, state: State) {
    p.rect_filled(
        rc,
        crate::theme::metrics().check_radius,
        fluent_accent(state),
    );
    let y = rc.center().y.floor() + 0.5;
    p.line_segment(
        [pos2(rc.left() + 3.0, y), pos2(rc.right() - 3.0, y)],
        Stroke::new(1.4, fluent_mark()),
    );
}

/// A row's check box ([`row_check`] at English `glyph`) shown partly ticked: a folder or a
/// package with only some of its files chosen.
pub fn row_check_mixed(p: &Painter, glyph: Pos2) {
    check_box_mixed(
        p,
        rect(Rect::from_min_size(glyph, vec2(13.0, 13.0))),
        State::Normal,
    );
}

/// A tree row's expander at English point `at`: ▸ when closed (pointing into the row: ◂ in
/// RTL), ▾ when open (Add torrent's folders, Link grabber's packages).
pub fn tree_expander(p: &Painter, at: Pos2, open: bool) {
    let c = MUTED();
    let pts = if open {
        vec![at, pos2(at.x + 8.0, at.y), pos2(at.x + 4.0, at.y + 5.0)]
    } else {
        vec![
            pos2(at.x + 2.0, at.y - 1.0),
            pos2(at.x + 7.0, at.y + 3.5),
            pos2(at.x + 2.0, at.y + 8.0),
        ]
    };
    let pts = pts.into_iter().map(|q| pos2(mx(q.x), q.y)).collect();
    p.add(egui::Shape::convex_polygon(pts, c, Stroke::NONE));
}

/// A checked Windows 11 check box's or radio's accent in `state`: lighter under the pointer,
/// lighter still while pressed, grey when disabled.
fn fluent_accent(state: State) -> Color32 {
    let c = crate::theme::colors();
    match state {
        State::Disabled => c.check_on_disabled,
        State::Hot => c.check_on_hot,
        State::Pressed => c.check_on_pressed,
        _ => c.check_on,
    }
}

/// The mark on a checked box or radio (white on the light accent's blue, dark on the dark's).
fn fluent_mark() -> Color32 {
    crate::theme::colors().check_mark
}

/// A KDE check box's or radio's inside (none when disabled) and edge: the accent's edge under
/// the pointer or pressed, and when checked, around an inside tinted with the accent.
fn breeze_box(state: State, checked: bool) -> (Option<Color32>, Color32) {
    let c = crate::theme::colors();
    let (fill_c, border) = fluent_box(state);
    let edge = match state {
        State::Disabled if checked => c.check_on_disabled,
        State::Disabled => border,
        _ if checked => fluent_accent(state),
        State::Hot | State::Pressed => c.check_on_hot,
        _ => border,
    };
    let inside = match (checked, fill_c) {
        (true, Some(f)) => Some(crate::derive::mix(f, fluent_accent(state), 0.3)),
        (_, f) => f,
    };
    (inside, edge)
}

/// The tick or dot on a checked KDE box or radio: the text's colour, greyed when disabled.
fn breeze_mark(state: State) -> Color32 {
    let c = crate::theme::colors();
    if state == State::Disabled {
        c.check_on_disabled
    } else {
        c.check_mark
    }
}

/// An unchecked Windows 11 check box's or radio's fill (none when disabled) and border in
/// `state`: darker fill under the pointer, darker still and a fainter border while pressed.
fn fluent_box(state: State) -> (Option<Color32>, Color32) {
    fluent_box_in(state, crate::theme::colors())
}

fn fluent_box_in(state: State, c: &crate::palette::Palette) -> (Option<Color32>, Color32) {
    match state {
        State::Disabled => (None, c.check_border_disabled),
        State::Hot => (Some(c.check_hot), c.check_border_hot),
        State::Pressed => (Some(c.check_pressed), c.check_border_pressed),
        _ => (Some(c.check), c.check_border),
    }
}

/// Win11 light radio glyph (13 × 13) with its label at glyph.x + 16. The caller sets the
/// choice when the response is clicked.
pub fn radio_light(
    ui: &mut Ui,
    glyph: Pos2,
    selected: bool,
    caption: &str,
    enabled: bool,
) -> Response {
    radio_light_until(ui, glyph, selected, caption, enabled, None)
}

/// [`radio_light`] whose caption also stops before English x `until` (a control after it).
pub fn radio_light_until(
    ui: &mut Ui,
    glyph: Pos2,
    selected: bool,
    caption: &str,
    enabled: bool,
    until: Option<f32>,
) -> Response {
    let caption = crate::i18n::t(caption);
    let p = ui.painter().clone();
    // A caption longer than its room ends in "..." at the edge, whole on hover (UI rule).
    let (shown, cut) = fit_caption_until(&p, glyph.x + 16.0, caption, until);
    let w = text_width(&p, &shown);
    // `glyph` is English; the glyph and its caption (after it) are placed on screen.
    let english = glyph;
    let hit = rect(Rect::from_min_size(english, vec2(16.0 + w + 2.0, 13.0)));
    let glyph = at(english, 13.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(
        hit,
        widget_id(ui, ("radio-light", glyph.x as i32, glyph.y as i32)),
        sense,
    );
    pass_focus_on(ui, &resp, enabled);
    let resp = enter_is_not_a_click(ui, resp);
    focus_on_press(&resp);
    show_focus(ui, &resp, hit.expand(1.0));
    expose(
        &resp,
        egui::Role::RadioButton,
        caption,
        enabled,
        Some(selected),
    );
    let c = glyph + vec2(6.5, 6.5);
    let state = State::of(&resp, enabled);
    // The era themes: their own round radios.
    if era_check(
        &p,
        Rect::from_center_size(c, vec2(13.0, 13.0)),
        selected,
        state,
        true,
    ) {
        // Drawn.
    } else if crate::theme::current().controls.check == crate::palette::CheckLook::Breeze {
        // KDE: as its check box, round; the dot in the text's colour.
        let (fill_c, edge) = breeze_box(state, selected);
        if let Some(f) = fill_c {
            p.circle_filled(c, 6.0, f);
        }
        p.circle_stroke(c, 6.0, Stroke::new(1.0, edge));
        if selected {
            p.circle_filled(c, 2.5, breeze_mark(state));
        }
    } else if selected {
        p.circle_filled(c, 6.5, fluent_accent(state));
        // Windows 11's dot grows under the pointer and shrinks while pressed.
        let dot = match state {
            State::Hot => 3.1,
            State::Pressed => 2.1,
            _ => 2.6,
        };
        p.circle_filled(c, dot, fluent_mark());
    } else {
        let (fill_c, ring) = fluent_box(state);
        if let Some(f) = fill_c {
            p.circle_filled(c, 6.0, f);
        }
        // GNOME's ring is two points wide, as its check boxes' edge.
        let adwaita = crate::theme::current().controls.check == crate::palette::CheckLook::Adwaita;
        if adwaita {
            p.circle_stroke(c, 5.5, Stroke::new(2.0, ring));
        } else {
            p.circle_stroke(c, 6.0, Stroke::new(1.0, ring));
        }
        // Pressed, Windows 11 shows the dot to come.
        if state == State::Pressed && !adwaita {
            p.circle_filled(c, 2.1, ring);
        }
    }
    let color = if enabled {
        LIGHT_TEXT()
    } else {
        LIGHT_DISABLED()
    };
    text(&p, english.x + 16.0, glyph.y + 10.0, &shown, color);
    if cut {
        resp.on_hover_text(caption)
    } else {
        resp
    }
}

/// A held button that repeats, as Windows' spinners and scroll-bar arrows do: true when it is
/// pressed, then after 0.4 s every 50 ms while it stays held (redraws are asked for to keep
/// time).
pub fn auto_repeat(ui: &Ui, resp: &Response) -> bool {
    let key = resp.id.with("repeat-at");
    if !resp.is_pointer_button_down_on() {
        ui.data_mut(|d| d.remove::<f64>(key));
        return false;
    }
    // Paused while the pointer is off it (Windows' arrows wait for it to come back).
    if !resp.contains_pointer() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
        return false;
    }
    let now = ui.input(|i| i.time);
    let due = ui.data(|d| d.get_temp::<f64>(key));
    let (fire, next) = match due {
        None => (true, now + 0.4),
        Some(t) if now >= t => (true, t + 0.05),
        Some(t) => (false, t),
    };
    ui.data_mut(|d| d.insert_temp(key, next));
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs_f64((next - now).max(0.01)));
    fire
}

/// What a control is for screen readers (Narrator, NVDA through AccessKit): its role, its name
/// (the full translated label, "&" taken out), whether it can be used, and for a check box,
/// radio button, tab or list row whether it is on. Our controls are painted by hand, so egui
/// can't tell otherwise. Call right after the control's `interact`, before any tooltip.
/// A line of text a window paints (a status, an error) made readable too: a screen reader and UI
/// Automation find it as static text at `rc`, by `salt`, with `text` as it reads (no mnemonic).
pub fn readable(ui: &Ui, rc: Rect, salt: impl std::hash::Hash + std::fmt::Debug, text: &str) {
    let resp = ui.interact(rc, widget_id(ui, ("readable", salt)), Sense::hover());
    let text = text.to_owned();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
}

pub fn expose(resp: &Response, role: egui::Role, label: &str, enabled: bool, on: Option<bool>) {
    let name = label
        .replace("&&", "\u{1}")
        .replace('&', "")
        .replace('\u{1}', "&");
    resp.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(role, enabled, &name);
        info.selected = on;
        info
    });
}

/// A drop-down list for screen readers and UI Automation, whose list (the shared popup) they
/// can't reach: Set value picks the item of that text (as shown, or as written), Expand and
/// Collapse open and close the list. Returns the item picked this frame.
pub fn assistive_choice(resp: &Response, items: &[&str], open: &mut bool) -> Option<usize> {
    use egui::accesskit::{Action, ActionData};
    let (value, expand, collapse) = resp.ctx.input(|i| {
        let value = i
            .accesskit_action_requests(resp.id, Action::SetValue)
            .find_map(|r| match &r.data {
                Some(ActionData::Value(v)) => Some(v.to_string()),
                _ => None,
            });
        (
            value,
            i.has_accesskit_action_request(resp.id, Action::Expand),
            i.has_accesskit_action_request(resp.id, Action::Collapse),
        )
    });
    if expand {
        *open = true;
    }
    if collapse {
        *open = false;
    }
    let expanded = *open;
    resp.ctx.accesskit_node_builder(resp.id, |n| {
        n.add_action(Action::SetValue);
        n.add_action(if expanded {
            Action::Collapse
        } else {
            Action::Expand
        });
        n.set_expanded(expanded);
    });
    let value = value?;
    let value = value.trim();
    items
        .iter()
        .position(|s| crate::i18n::t(s) == value || *s == value)
}

/// Whether a screen reader or UI Automation set this control's value this frame (AccessKit's
/// Set value), focused or not: such a value is meant at once, where a typed one waits for Enter
/// or the focus to leave.
pub fn value_set_by_assistive(resp: &Response) -> bool {
    resp.ctx
        .input(|i| i.has_accesskit_action_request(resp.id, egui::accesskit::Action::SetValue))
}

/// [`expose`] for a control showing a value (a drop-down): named by its caption, the value read
/// after it.
pub fn expose_value(resp: &Response, role: egui::Role, value: &str, enabled: bool) {
    let name = caption_for(&resp.ctx, resp.rect).unwrap_or_default();
    let value = value.to_owned();
    resp.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(role, enabled, &name);
        info.current_text_value = Some(value.clone());
        info
    });
}

/// The click of a control that opens a popup, except the one ending the press that just closed
/// that popup (it went down on the control): a click on an open list's arrow closes it, not
/// closes and opens it again.
pub fn opener_click(ui: &Ui, mut resp: Response) -> Response {
    let key = crate::chrome::dismissed_at_key(ui.ctx());
    let Some(at) = ui.data(|d| d.get_temp::<Pos2>(key)) else {
        return resp;
    };
    if resp.clicked() && resp.rect.contains(at) {
        resp.flags.remove(egui::response::Flags::CLICKED);
        ui.data_mut(|d| d.remove::<Pos2>(key));
    } else if !ui.input(|i| i.pointer.any_down() || i.pointer.any_released()) {
        // Between presses: that press is over.
        ui.data_mut(|d| d.remove::<Pos2>(key));
    }
    resp
}

/// The wheel's travel this frame in rows of `row_h` points, down positive (the caller checks
/// the pointer is over it): a notch moves the system's "lines per notch" rows, a page setting
/// `page` rows, a trackpad its own travel. A Ctrl wheel (zoom) or a Shift wheel (sideways)
/// isn't counted.
pub fn wheel_rows(ui: &Ui, row_h: f32, page: f32) -> f32 {
    #[cfg(windows)]
    let lines = crate::win32::wheel_lines();
    #[cfg(unix)]
    let lines = crate::unix::wheel_lines();
    let notch = lines.unwrap_or(page);
    let rows = ui.input(|i| {
        i.events
            .iter()
            .map(|e| match e {
                egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } if !(modifiers.ctrl || modifiers.command || modifiers.shift) => match unit {
                    egui::MouseWheelUnit::Line => -delta.y * notch,
                    egui::MouseWheelUnit::Page => -delta.y * page,
                    egui::MouseWheelUnit::Point => -delta.y / row_h,
                },
                _ => 0.0,
            })
            .sum()
    });
    // Taken: a window scrolled as a whole (scale::pane) leaves this turn to the list under the
    // pointer.
    if rows != 0.0 {
        let pass = ui.ctx().cumulative_pass_nr();
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::unique("wheel-taken"), pass));
    }
    rows
}

/// Whether a list or a box under the pointer took this frame's wheel ([`wheel_rows`]).
pub fn wheel_taken(ui: &Ui) -> bool {
    let pass = ui.ctx().cumulative_pass_nr();
    ui.ctx()
        .data(|d| d.get_temp::<u64>(egui::Id::unique("wheel-taken")))
        == Some(pass)
}

/// [`wheel_rows`] in whole rows, the rest kept under `id` for the next turn of the wheel (a
/// trackpad's small moves add up).
pub fn wheel_steps(ui: &Ui, id: egui::Id, row_h: f32, page: f32) -> isize {
    let key = id.with("wheel-rest");
    let rest: f32 = ui.ctx().data(|d| d.get_temp(key)).unwrap_or(0.0);
    let total = rest + wheel_rows(ui, row_h, page);
    let whole = total.trunc();
    ui.ctx().data_mut(|d| d.insert_temp(key, total - whole));
    whole as isize
}

/// A control greyed while it has the keys hands them to the next one, as in Windows: egui let
/// them go to nowhere, and Tab then started over from the window's first control (a button
/// that greys itself when pressed, a box greyed by a choice made from the keyboard). Called
/// where the control is laid out, so "next" is the one after it.
pub fn pass_focus_on(ui: &Ui, resp: &Response, enabled: bool) {
    let id = resp.id;
    if !enabled
        && ui
            .ctx()
            .memory(|m| m.focused().is_none() && m.had_focus_last_frame(id))
    {
        ui.ctx()
            .memory_mut(|m| m.move_focus(egui::FocusDirection::Next));
    }
}

/// A system file or folder dialog titled `title`, for [`start_pick`]: the system draws the title,
/// so it goes without isolates (#919).
pub fn file_dialog(title: &str) -> rfd::FileDialog {
    rfd::FileDialog::new().set_title(crate::i18n::os_text(title))
}

/// A system file or folder dialog's answer, from its thread.
type PickSlot = std::sync::Arc<std::sync::Mutex<Option<Option<std::path::PathBuf>>>>;

/// Show a system Open / Save / folder dialog (`ask` builds and runs it) on a thread of its
/// own: it blocked the app's windows while open (progress windows stood still). One per
/// `key` at a time; [`picked`] gives its answer on a later frame.
pub fn start_pick(
    ctx: &egui::Context,
    key: &str,
    ask: impl FnOnce() -> Option<std::path::PathBuf> + Send + 'static,
) {
    let id = egui::Id::unique(("pick", key));
    if ctx.data(|d| d.get_temp::<PickSlot>(id)).is_some() {
        return;
    }
    let slot: PickSlot = Default::default();
    ctx.data_mut(|d| d.insert_temp(id, slot.clone()));
    let wake = ctx.clone();
    std::thread::spawn(move || {
        let answer = ask();
        *slot.lock().unwrap() = Some(answer);
        wake.request_repaint();
    });
}

/// The path chosen in [`start_pick`]'s dialog `key`, once (None while it is open, or when it
/// was cancelled).
pub fn picked(ctx: &egui::Context, key: &str) -> Option<std::path::PathBuf> {
    let id = egui::Id::unique(("pick", key));
    let slot = ctx.data(|d| d.get_temp::<PickSlot>(id))?;
    let answer = slot.lock().unwrap().take()?;
    ctx.data_mut(|d| d.remove::<PickSlot>(id));
    answer
}

/// A press gives a control the keys, as in Windows (egui focuses only text boxes on a click):
/// the arrows work in a radio group clicked, Ctrl+C in a field clicked. Its ring shows only
/// once the keyboard is used ([`keyboard_in_use`]).
pub fn focus_on_press(resp: &Response) {
    if resp.is_pointer_button_down_on() && !resp.has_focus() && resp.sense.senses_click() {
        resp.request_focus();
    }
}

/// egui clicks a focused control on Enter as on Space. A check box, radio button or tab answers
/// Space only, as in Windows: Enter is the dialog's default button (it ticked the box and
/// pressed OK). Links do open on Enter.
pub fn enter_is_not_a_click(ui: &Ui, mut resp: Response) -> Response {
    if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        resp.flags.remove(egui::response::Flags::CLICKED);
    }
    resp
}

/// A focused control that answers arrow keys (or Escape) itself keeps them: egui otherwise also
/// moves the focus to the next control on an arrow and drops it on Escape. Call each frame
/// with the control's response.
pub fn keep_keys(ui: &Ui, resp: &Response, horizontal: bool, vertical: bool, escape: bool) {
    if resp.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                resp.id,
                egui::EventFilter {
                    horizontal_arrows: horizontal,
                    vertical_arrows: vertical,
                    escape,
                    ..Default::default()
                },
            )
        });
    }
}

/// Type-ahead, as Windows lists and drop-downs have it: letters typed while it has the keys
/// pick the next of its `n` items whose `label` starts with them (typed within a second of
/// each other; one letter again and again goes through the items starting with it). `key`
/// keeps what was typed. Returns the item to choose.
pub fn typed_match(
    ui: &Ui,
    key: egui::Id,
    n: usize,
    current: Option<usize>,
    label: impl Fn(usize) -> String,
) -> Option<usize> {
    let typed: String = ui.input(|i| {
        if i.modifiers.ctrl || i.modifiers.alt || i.modifiers.command {
            return String::new();
        }
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<String>()
    });
    let typed: String = typed.chars().filter(|c| !c.is_control()).collect();
    // Space alone is a key of its own (it ticks a row), not a name's first letter.
    if typed.trim().is_empty() || n == 0 {
        return None;
    }
    let now = ui.input(|i| i.time);
    let (mut buf, at) = ui
        .data(|d| d.get_temp::<(String, f64)>(key))
        .unwrap_or_default();
    if now - at > 1.0 {
        buf.clear();
    }
    buf.push_str(&typed.to_lowercase());
    ui.data_mut(|d| d.insert_temp(key, (buf.clone(), now)));
    let first = buf.chars().next().unwrap_or_default();
    let repeated = buf.chars().all(|c| c == first);
    // The same letter again: the next item with it; else the first from here with the prefix.
    let (needle, start) = if repeated {
        (first.to_string(), current.map_or(0, |c| c + 1))
    } else {
        (buf, current.unwrap_or(0))
    };
    (0..n)
        .map(|k| (start + k) % n)
        .find(|&i| label(i).to_lowercase().starts_with(&needle))
}

/// A tab control's keys, as Windows has them: Ctrl+Tab / Ctrl+Page Down the next of its `n`
/// tabs, Ctrl+Shift+Tab / Ctrl+Page Up the previous one, wrapping. For a window's main tabs
/// (one tab control a window answers them).
pub fn tab_keys(ui: &Ui, n: usize, current: usize) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let (next, prev) = ui.input_mut(|i| {
        let ctrl = egui::Modifiers::COMMAND;
        let ctrl_shift = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
        (
            i.consume_key(ctrl, egui::Key::Tab) || i.consume_key(ctrl, egui::Key::PageDown),
            i.consume_key(ctrl_shift, egui::Key::Tab) || i.consume_key(ctrl, egui::Key::PageUp),
        )
    });
    match (next, prev) {
        (true, _) => Some((current + 1) % n),
        (_, true) => Some((current + n - 1) % n),
        _ => None,
    }
}

/// A radio group's keys, as Windows has them: while one of the group's buttons (`group`, in
/// order) has the focus, Up / Left and Down / Right move the focus to the previous / next
/// enabled one, wrapping, and choose it. Returns the one chosen.
pub fn radio_keys(ui: &Ui, group: &[Response]) -> Option<usize> {
    let at = group.iter().position(Response::has_focus)?;
    keep_keys(ui, &group[at], true, true, false);
    let step: isize = if take_key(ui, egui::Key::ArrowDown) || take_key(ui, egui::Key::ArrowRight) {
        1
    } else if take_key(ui, egui::Key::ArrowUp) || take_key(ui, egui::Key::ArrowLeft) {
        -1
    } else {
        return None;
    };
    let n = group.len() as isize;
    let next = (1..n)
        .map(|k| (at as isize + step * k).rem_euclid(n) as usize)
        .find(|&j| group[j].sense.senses_click())?;
    group[next].request_focus();
    Some(next)
}

/// Etched horizontal separator in English rect `x, y, w × 2`: `#a0a0a0` over `#ffffff`, the
/// white line 1 px longer so it covers the far end.
pub fn etched_h(p: &Painter, x: f32, y: f32, w: f32) {
    let x = rect(Rect::from_min_size(pos2(x, y), vec2(w, 2.0))).left();
    hline(p, x, x + w - 2.0, y, ETCH_SHADOW());
    px(p, x + w - 1.0, y, ETCH_LIGHT());
    hline(p, x, x + w - 1.0, y + 1.0, ETCH_LIGHT());
}

/// Etched vertical separator in English rect `x, y, 2 × h`.
pub fn etched_v(p: &Painter, x: f32, y: f32, h: f32) {
    let x = rect(Rect::from_min_size(pos2(x, y), vec2(2.0, h))).left();
    vline(p, x, y, y + h - 2.0, ETCH_SHADOW());
    px(p, x, y + h - 1.0, ETCH_LIGHT());
    vline(p, x + 1.0, y, y + h - 1.0, ETCH_LIGHT());
}

/// [`fill`] in English pixels (inclusive): on screen in a mirrored surface.
pub fn fill_l(p: &Painter, x0: f32, y0: f32, x1: f32, y1: f32, c: Color32) {
    let (a, b) = (mcol(x0), mcol(x1));
    fill(p, a.min(b), y0, a.max(b), y1, c);
}

/// [`px`] at an English pixel.
pub fn px_l(p: &Painter, x: f32, y: f32, c: Color32) {
    px(p, mcol(x), y, c);
}

/// One tab of a light tab control: its x range (inclusive, from the client origin) when
/// selected and when not (Windows draws the selected tab wider).
pub struct LightTab {
    pub label: &'static str,
    pub sel: (f32, f32),
    pub unsel: (f32, f32),
}

/// A translated row of tabs, sized to their labels as Windows sizes them: each `pad` px either
/// side of its (translated) label, from English x `start`, `gap` px apart. A row longer than
/// `right` caps the widest tabs to share the room and cuts their labels with "...".
/// (x0, x1, label shown) per tab, English x.
pub fn tab_row(
    p: &Painter,
    labels: &[&str],
    start: f32,
    right: f32,
    gap: f32,
    pad: f32,
) -> Vec<(f32, f32, String)> {
    let widths: Vec<f32> = labels
        .iter()
        .map(|s| text_width(p, s).ceil() + 2.0 * pad)
        .collect();
    let room = right - start - gap * labels.len().saturating_sub(1) as f32;
    let cap = width_cap(&widths, room);
    let mut x0 = start;
    labels
        .iter()
        .zip(widths)
        .map(|(s, w)| {
            let w = w.min(cap);
            let shown = ellipsize(p, &tahoma(), s, w - 2.0 * pad);
            let tab = (x0, x0 + w, shown);
            x0 += w + gap;
            tab
        })
        .collect()
}

/// The largest whole width `c` with Σ min(wᵢ, c) ≤ `room` (infinite when everything fits): the
/// narrow ones keep their width, the wide ones share what is left.
pub fn width_cap(widths: &[f32], room: f32) -> f32 {
    let mut sorted = widths.to_vec();
    sorted.sort_by(f32::total_cmp);
    let mut used = 0.0;
    for (k, w) in sorted.iter().enumerate() {
        let rest = (sorted.len() - k) as f32;
        if used + w * rest > room {
            return ((room - used) / rest).floor();
        }
        used += w;
    }
    f32::INFINITY
}

#[cfg(test)]
mod tab_row_tests {
    #[test]
    fn the_widest_share_what_is_left() {
        assert_eq!(super::width_cap(&[50.0, 60.0], 200.0), f32::INFINITY);
        // 40 keeps its width; 100 and 120 share the remaining 120.
        assert_eq!(super::width_cap(&[100.0, 40.0, 120.0], 160.0), 60.0);
    }
}

/// Light tab control in `rc` (client rect of the whole control): the pane and the tabs.
/// Returns the tab clicked this frame.
pub fn tabs_light(
    ui: &mut Ui,
    o: Pos2,
    rc: Rect,
    tabs: &[LightTab],
    selected: usize,
) -> Option<usize> {
    let p = ui.painter().clone();
    let border = HEADER_LINE();
    let shadow = TAB_UNSELECTED();
    // Pane: x rc.left ..= right − 3, y top + 20 ..= bottom − 2, then a 2 px / 1 px shadow.
    let (l, t, rr, b) = (
        rc.left(),
        rc.top() + 20.0,
        rc.right() - 3.0,
        rc.bottom() - 2.0,
    );
    fill(&p, l, t, rr, b, LIGHT_PAGE());
    hline(&p, l, rr, t, border);
    hline(&p, l, rr, b, border);
    vline(&p, l, t, b, border);
    vline(&p, rr, t, b, border);
    fill(&p, rr + 1.0, t, rr + 2.0, b + 1.0, shadow);
    hline(&p, l, rr + 2.0, b + 1.0, shadow);
    // (selected range, unselected range, label x, label shown), from the client origin.
    type Placed = ((f32, f32), (f32, f32), f32, String);
    let placed: Vec<Placed> = if tabs.iter().all(|t| crate::i18n::t(t.label) == t.label) {
        tabs.iter()
            .map(|t| (t.sel, t.unsel, t.sel.0 + 8.0, t.label.to_owned()))
            .collect()
    } else {
        // A translation: tabs sized to their labels from the first tab's place, within the pane
        // (the selected one 2 px wider each side).
        let labels: Vec<&str> = tabs.iter().map(|t| crate::i18n::t(t.label)).collect();
        let (start, left) = tabs.first().map_or((0.0, 0.0), |t| (t.unsel.0, t.sel.0));
        tab_row(&p, &labels, start, left + rc.width() - 3.0 - 2.0, 2.0, 8.0)
            .into_iter()
            .map(|(x0, x1, shown)| {
                let lx = x0 + ((x1 - x0 - text_width(&p, &shown)) / 2.0).floor();
                ((x0 - 2.0, x1 + 2.0), (x0, x1), lx, shown)
            })
            .collect()
    };
    let mut clicked = None;
    let mut draw = |i: usize, tab: &LightTab, sel: bool| {
        let (sel_x, unsel_x, label_x, shown) = &placed[i];
        let (x0, x1) = if sel { *sel_x } else { *unsel_x };
        // English pixel columns; the tabs run from the leading edge.
        let (a, b) = (mcol(o.x + x0), mcol(o.x + x1));
        let (x0, x1) = (a.min(b), a.max(b));
        let y0 = if sel { rc.top() } else { rc.top() + 2.0 };
        let hit = Rect::from_min_max(pos2(x0, y0), pos2(x1 + 1.0, t));
        // Keyed by the control's place and the tab's index: its x changes when it is chosen,
        // and the focus must stay on it.
        // One Tab stop, the chosen tab (Windows' tab control); the arrows choose.
        let sense = if sel { Sense::click() } else { pointer_only() };
        let id_of =
            |ui: &Ui, k: usize| widget_id(ui, ("tab-light", rc.min.x as i32, rc.min.y as i32, k));
        let resp = ui.interact(hit, id_of(ui, i), sense);
        focus_on_press(&resp);
        expose(
            &resp,
            egui::Role::Tab,
            crate::i18n::t(tab.label),
            true,
            Some(sel),
        );
        let resp = enter_is_not_a_click(ui, resp);
        if resp.clicked() && !sel {
            clicked = Some(i);
        }
        if sel {
            keep_keys(ui, &resp, true, false, false);
            if resp.has_focus() {
                let last = tabs.len().saturating_sub(1) as isize;
                let fwd = if rtl() { -1 } else { 1 };
                let to = if take_key(ui, egui::Key::ArrowRight) {
                    Some(i as isize + fwd)
                } else if take_key(ui, egui::Key::ArrowLeft) {
                    Some(i as isize - fwd)
                } else if take_key(ui, egui::Key::Home) {
                    Some(0)
                } else if take_key(ui, egui::Key::End) {
                    Some(last)
                } else {
                    None
                };
                if let Some(to) = to.map(|t| t.clamp(0, last) as usize).filter(|t| *t != i) {
                    clicked = Some(to);
                    let next = id_of(ui, to);
                    ui.memory_mut(|m| m.request_focus(next));
                }
            }
        }
        let face = if sel {
            LIGHT_PAGE()
        } else if resp.hovered() {
            TAB_HOT()
        } else {
            shadow
        };
        fill(&p, x0, y0, x1, t, face);
        hline(&p, x0 + 1.0, x1 - 1.0, y0, border);
        vline(&p, x0, y0 + 1.0, t, border);
        vline(&p, x1, y0 + 1.0, t, border);
        if sel {
            // Open at the bottom: the pane's top line stops at the tab.
            hline(&p, x0 + 1.0, x1 - 1.0, t, LIGHT_PAGE());
        } else {
            hline(&p, x0, x1, t, border);
        }
        let base = if sel {
            rc.top() + 13.0
        } else {
            rc.top() + 15.0
        };
        let full = crate::i18n::t(tab.label);
        let drawn = text(&p, o.x + label_x, base, shown, LIGHT_TEXT());
        // Over the tab's face (drawn before it, the face hid it).
        show_focus(ui, &resp, hit.shrink(2.0));
        if shown != full {
            full_on_hover(ui, drawn, ("tab-light-label", i), full);
        }
    };
    for (i, tab) in tabs.iter().enumerate().filter(|(i, _)| *i != selected) {
        draw(i, tab, false);
    }
    if let Some(tab) = tabs.get(selected) {
        draw(selected, tab, true);
    }
    clicked
}

/// The 8 × 5 "v" of drop-downs, its top-left at `at`.
pub fn chevron_light(p: &Painter, at: Pos2, enabled: bool) {
    let c = if enabled {
        CHEVRON()
    } else {
        CHEVRON_DISABLED()
    };
    p.line(
        vec![
            at + vec2(0.5, 0.5),
            at + vec2(4.0, 4.0),
            at + vec2(7.5, 0.5),
        ],
        Stroke::new(1.2, c),
    );
}

/// The frame of a light drop-down (combo or date picker): rounded `#c8c8c8` border, `#fefefe`
/// fill; enabled ones darken on hover and take the accent border while open.
/// A small button's face (a spin box's halves, the icon buttons): a push button's colours in
/// every state ([`button_colors`]): they had their own, and a disabled one looked hovered.
pub fn button_like(p: &Painter, rc: Rect, state: State) {
    let kind = if state == State::Disabled {
        ButtonKind::Disabled
    } else {
        ButtonKind::Normal
    };
    let face = button_colors(kind, state, crate::theme::colors());
    paint_button(p, rc, kind, state, &face, Shape::Small);
}

pub fn dropdown_frame_light(p: &Painter, rc: Rect, state: State) {
    // Drawn as a button (Windows 11 draws its drop-downs so): the same faces and states, in
    // the current theme; open, the accent border and no bottom edge (its list is out).
    let open = state == State::Open;
    let kind = if state == State::Disabled {
        ButtonKind::Disabled
    } else {
        ButtonKind::Normal
    };
    let face = button_colors(
        kind,
        if open { State::Hot } else { state },
        crate::theme::colors(),
    );
    // Open: the accent all round (its bottom too: the list is out).
    let face = if open {
        Face {
            border: accent(),
            bottom: accent(),
            ..face
        }
    } else {
        face
    };
    paint_button(p, rc, kind, state, &face, Shape::DropDown);
}

/// The accent (Windows' blue): borders of open and focused controls.
pub fn accent() -> Color32 {
    crate::theme::colors().accent
}

/// Light drop-down list (CBS_DROPDOWNLIST): the chosen item and a chevron; a click opens the
/// list under it. Returns the item chosen this frame.
pub fn combo_light(
    ui: &mut Ui,
    rc: Rect,
    items: &[&str],
    selected: usize,
    enabled: bool,
    id: &str,
) -> Option<usize> {
    let key = widget_id(ui, ("combo-light", id));
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(rc, key.with("button"), sense);
    pass_focus_on(ui, &resp, enabled);
    let resp = opener_click(ui, resp);
    focus_on_press(&resp);
    keep_keys(ui, &resp, false, true, false);
    let mut open = ui.data(|d| d.get_temp::<bool>(key).unwrap_or(false)) && enabled;
    let popup_id = format!("combo-light-{}", key.short_debug_format());
    let mut chosen = None;
    let last = items.len().saturating_sub(1);
    // egui clicks a focused control on Enter too: on a closed one Enter is the dialog's default
    // button, not an opener.
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    if resp.clicked() && !enter {
        open = !open;
    }
    // The keys, as a Windows drop-down list has them (an open list's are the popup's).
    if resp.has_focus() && enabled && !items.is_empty() && !open {
        let opener = ui.input_mut(|i| {
            i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowDown)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::F4)
        });
        if opener {
            open = true;
            crate::chrome::start_at(ui.ctx(), &popup_id, selected.min(last));
        }
        let to = if take_key(ui, egui::Key::ArrowDown) {
            Some((selected + 1).min(last))
        } else if take_key(ui, egui::Key::ArrowUp) {
            Some(selected.saturating_sub(1))
        } else if take_key(ui, egui::Key::Home) {
            Some(0)
        } else if take_key(ui, egui::Key::End) {
            Some(last)
        } else {
            None
        };
        // Typing picks the item starting with it (the arrows change the choice too).
        let typed = typed_match(ui, key.with("typed"), items.len(), Some(selected), |i| {
            crate::i18n::t(items[i]).to_owned()
        });
        // Closed: the arrows change the choice itself.
        if let Some(i) = to.or(typed).filter(|i| *i != selected) {
            chosen = Some(i);
        }
    }
    let p = ui.painter().clone();
    dropdown_frame_light(&p, rc, State::of(&resp, enabled).open_if(open));
    let color = if enabled {
        LIGHT_TEXT()
    } else {
        LIGHT_DISABLED()
    };
    // Items are shown in the interface language (data such as numbers passes through as is).
    let full = crate::i18n::t(items.get(selected).copied().unwrap_or(""));
    expose_value(&resp, egui::Role::ComboBox, full, enabled);
    if enabled && let Some(i) = assistive_choice(&resp, items, &mut open) {
        chosen = Some(i).filter(|i| *i != selected);
    }
    // Cut with "..." before the chevron when too long (a translation), whole on hover.
    let label = ellipsize(&p, &tahoma(), full, rc.width() - 5.0 - 19.0);
    let drawn = text_start(
        &p,
        tahoma(),
        lead(rc) + 5.0 * fwd(),
        rc.top() + 13.0,
        &label,
        color,
    );
    if label != full && !open {
        full_on_hover(ui, drawn, ("combo-light-label", id), full);
    }
    // The chevron at the trailing edge.
    let chevron_x = if rtl() {
        rc.left() + 5.0
    } else {
        rc.right() - 13.0
    };
    chevron_light(&p, pos2(chevron_x, rc.top() + 8.0), enabled);
    // Over the face (drawn before it, the face hid it).
    show_focus(ui, &resp, rc.shrink(3.0));
    if open {
        // The list: the shared popup (a window of its own, never clipped by the dialog, kept on
        // the screen; the keys, the pointer), the choice marked.
        use crate::chrome::{MenuEntry, PopupResult, label as menu_label};
        let entries: Vec<MenuEntry<usize>> = items
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let shown = crate::i18n::t(s).replace('&', "&&");
                MenuEntry::Radio(menu_label(&shown), i, true, i == selected)
            })
            .collect();
        let at = if rtl() {
            rc.right_bottom()
        } else {
            rc.left_bottom()
        };
        match crate::chrome::popup(
            ui.ctx(),
            crate::chrome::MenuKind::List,
            &popup_id,
            at,
            &entries,
            &|_| None,
        ) {
            PopupResult::Chosen(i) => {
                open = false;
                chosen = Some(i).filter(|i| *i != selected);
                resp.request_focus();
            }
            PopupResult::Dismissed => {
                open = false;
                resp.request_focus();
            }
            PopupResult::Open | PopupResult::Neighbor(_) => {}
        }
        // The keys went to the list: the dialog's default button doesn't take Enter too.
        take_key(ui, egui::Key::Enter);
    }
    ui.data_mut(|d| d.insert_temp(key, open));
    chosen
}

/// A button that opens a menu (Download complete's Snooze, #741): its label and a chevron on a
/// drop-down's face, in the current theme. A click, Space, Alt+Down or F4 opens `items` (English,
/// translated here) under it, in the shared popup; Enter stays the window's default button's.
/// Returns the item picked this frame.
pub fn menu_button(ui: &mut Ui, rc: Rect, label: &str, items: &[&str], id: &str) -> Option<usize> {
    let key = widget_id(ui, ("menu-button", id));
    let resp = ui.interact(rc, key.with("button"), Sense::click());
    pass_focus_on(ui, &resp, true);
    let resp = opener_click(ui, resp);
    focus_on_press(&resp);
    let mut open = ui.data(|d| d.get_temp::<bool>(key).unwrap_or(false));
    let popup_id = format!("menu-button-{}", key.short_debug_format());
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    if resp.clicked() && !enter {
        open = !open;
    }
    if resp.has_focus() && !open {
        let opener = ui.input_mut(|i| {
            i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowDown)
                || i.consume_key(egui::Modifiers::NONE, egui::Key::F4)
        });
        if opener {
            open = true;
            crate::chrome::start_at(ui.ctx(), &popup_id, 0);
        }
    }
    let p = ui.painter().clone();
    dropdown_frame_light(&p, rc, State::of(&resp, true).open_if(open));
    let shown = crate::i18n::t(label);
    expose(&resp, egui::Role::Button, shown, true, None);
    let text = ellipsize(&p, &tahoma(), shown, rc.width() - 8.0 - 19.0);
    text_start(
        &p,
        tahoma(),
        lead(rc) + 8.0 * fwd(),
        rc.top() + 14.0,
        &text,
        TEXT(),
    );
    let chevron_x = if rtl() {
        rc.left() + 6.0
    } else {
        rc.right() - 14.0
    };
    chevron_light(&p, pos2(chevron_x, rc.top() + 8.0), true);
    show_focus(ui, &resp, rc.shrink(3.0));
    let mut chosen = None;
    if open {
        use crate::chrome::{MenuEntry, PopupResult, label as menu_label};
        let entries: Vec<MenuEntry<usize>> = items
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let shown = crate::i18n::t(s).replace('&', "&&");
                MenuEntry::Item(menu_label(&shown), i, true, false)
            })
            .collect();
        // Under the button from its leading edge (a menu's top-right in right to left).
        let at = if rtl() {
            rc.right_bottom()
        } else {
            rc.left_bottom()
        };
        match crate::chrome::popup(
            ui.ctx(),
            crate::chrome::MenuKind::Context,
            &popup_id,
            at,
            &entries,
            &|_| None,
        ) {
            PopupResult::Chosen(i) => {
                open = false;
                chosen = Some(i);
            }
            PopupResult::Dismissed => {
                open = false;
                resp.request_focus();
            }
            PopupResult::Open | PopupResult::Neighbor(_) => {}
        }
        // The keys went to the menu: the window's default button doesn't take Enter too.
        take_key(ui, egui::Key::Enter);
    }
    ui.data_mut(|d| d.insert_temp(key, open));
    chosen
}

/// A [`menu_button`]'s width for `label` in this language: its text, its chevron and their
/// margins, between `min` and `max`.
pub fn menu_button_width(p: &Painter, label: &str, min: f32, max: f32) -> f32 {
    (text_width(p, crate::i18n::t(label)) + 8.0 + 22.0)
        .round()
        .clamp(min, max)
}

/// A spin box's arrows in `zone` ([`spin_zone`]): up and down halves inside the box, without
/// borders of their own, tinted under the pointer and pressed as the box's other parts are.
/// Returns +1 / −1 for a click (held: repeats), 0 otherwise.
pub fn spin_arrows(ui: &mut Ui, rc: Rect, enabled: bool, id: &str) -> i32 {
    let p = ui.painter().clone();
    let half = (rc.height() / 2.0).floor();
    let up = Rect::from_min_size(rc.min, vec2(rc.width(), half));
    let down = Rect::from_min_max(pos2(rc.left(), rc.top() + half), rc.max);
    let mut step = 0;
    for (i, part, dir) in [(0, up, 1), (1, down, -1)] {
        let sense = if enabled {
            pointer_only()
        } else {
            Sense::hover()
        };
        let r = ui.interact(part, widget_id(ui, ("updown-light", id, i)), sense);
        pass_focus_on(ui, &r, enabled);
        let name = if dir > 0 {
            // i18n: a spin box's arrow, for screen readers: one more
            crate::i18n::t("Increase")
        } else {
            // i18n: a spin box's arrow, for screen readers: one less
            crate::i18n::t("Decrease")
        };
        expose(&r, egui::Role::Button, name, enabled, None);
        let state = if !enabled {
            State::Disabled
        } else if r.is_pointer_button_down_on() {
            State::Pressed
        } else if r.hovered() {
            State::Hot
        } else {
            State::Normal
        };
        if let Some(tint) = subtle_fill(state, crate::theme::colors()) {
            p.rect_filled(part.shrink(1.0), crate::theme::metrics().part_radius, tint);
        }
        let c = part.center();
        let arrow = if enabled {
            LIGHT_TEXT()
        } else {
            CHEVRON_DISABLED()
        };
        let pts = if dir > 0 {
            vec![c + vec2(-3.0, 1.5), c + vec2(3.0, 1.5), c + vec2(0.0, -1.5)]
        } else {
            vec![
                c + vec2(-3.0, -1.5),
                c + vec2(3.0, -1.5),
                c + vec2(0.0, 1.5),
            ]
        };
        p.add(egui::Shape::convex_polygon(pts, arrow, Stroke::NONE));
        // Held, it keeps counting.
        if auto_repeat(ui, &r) {
            step = dir;
        }
    }
    step
}

/// A flat 19 × 21 button with a 16 × 16 icon (the Scheduler's move up / down / remove), its
/// `tip` (English, translated here) on hover.
pub fn icon_button_light(ui: &mut Ui, rc: Rect, id: &str, enabled: bool, tip: &str) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(rc, widget_id(ui, ("icon-button-light", id)), sense);
    pass_focus_on(ui, &resp, enabled);
    expose(
        &resp,
        egui::Role::Button,
        crate::i18n::t(tip),
        enabled,
        None,
    );
    let p = ui.painter();
    button_like(p, rc, State::of(&resp, enabled));
    show_focus(ui, &resp, rc.shrink(2.0));
    // No label of its own: its tooltip says what it does (translated).
    resp.on_hover_text(crate::i18n::t(tip))
}

/// The flat icon button with our "to the notification area" glyph, drawn in the text ink: hides
/// a window to the tray (#63) where the system draws the title bar (Linux, macOS). `tip` is
/// English, translated here.
#[cfg_attr(windows, allow(dead_code))]
pub fn to_tray_button(ui: &mut Ui, rc: Rect, id: &str, tip: &str) -> Response {
    let resp = icon_button_light(ui, rc, id, true, tip);
    to_tray_glyph(
        ui.painter(),
        rc.center().round(),
        Stroke::new(1.0, LIGHT_TEXT()),
        rtl(),
    );
    resp
}

/// Our "minimize it away to the notification area" glyph (#63; our own drawing): Minimize's dash
/// at its height, shortened to 6 of its 10 px on the start side, and from its end a 45° arrow
/// (shaft about 5.7 px, open head) down towards the screen's corner where the tray is. No
/// vertical arrow and no tray box, so it reads neither as "download" nor as "restore". Right to
/// left it is mirrored, pointing down to the left: Windows in a right-to-left language mirrors its
/// taskbar, the notification area at the bottom left, and our caption mirrors with it. Centred on
/// `c` in `ink`. The title bar's hide button, and the tab-row button where the system draws the
/// title bar.
fn to_tray_glyph(p: &Painter, c: Pos2, ink: Stroke, rtl: bool) {
    to_tray_glyph_sized(p, c, ink, rtl, 1.0);
}

/// [`to_tray_glyph`] `size` times as large.
fn to_tray_glyph_sized(p: &Painter, c: Pos2, ink: Stroke, rtl: bool, size: f32) {
    let s = if rtl { -size } else { size };
    let at = |x: f32, y: f32| c + vec2(s * x, size * y);
    // The dash, where Minimize's is (y + 0.5 keeps a 1 px line on the pixel grid).
    p.line_segment([at(-5.0, 0.5), at(1.0, 0.5)], ink);
    // The arrow from the dash's end, down to the end side at 45°, and its open head; the head's
    // strokes on half pixels, so they stay one pixel wide at 100 %.
    p.line_segment([at(1.0, 0.5), at(4.5, 4.0)], ink);
    p.add(egui::Shape::line(
        vec![at(4.5, 1.5), at(4.5, 4.5), at(1.5, 4.5)],
        ink,
    ));
}

/// A title bar we draw ourselves, with Windows 11's caption buttons (#63). Used on Windows,
/// where the progress window's frame is ours (win32::progress_frame); Linux and macOS keep the
/// system's decorations, whose title bars take no buttons of ours (and Wayland places them
/// itself), so the hide button sits in the tab row there.
pub mod caption {
    #![cfg_attr(not(windows), allow(dead_code))]
    use super::*;

    crate::theme::themed! {
        /// A title bar we draw ourselves (Windows 11's caption, both themes).
        pub CAPTION_BG => caption;
        pub CAPTION_INK => caption_text;
        /// A caption button's hover and pressed tints over the bar: WinUI's caption tokens, as
        /// Windows Terminal ships them (#0FFFFFFF / #09000000, #0AFFFFFF / #06000000).
        pub CAPTION_HOVER => caption_hot;
        pub CAPTION_PRESSED => caption_pressed;
        /// A button at rest, where the style shows one (GNOME's round buttons).
        CAPTION_BUTTON => caption_button;
        /// Close's hover red, and its glyph on it.
        CLOSE_HOVER => caption_close;
        CLOSE_INK => caption_close_text;
    }
    /// The title and glyphs while the window isn't the active one, and a greyed button's: the
    /// one disabled ink of every control.
    fn caption_ink_inactive() -> Color32 {
        crate::theme::colors().caption_text_inactive
    }
    /// Close's hover red with a white glyph; pressed, the red at 0.9 and the glyph at 0.7
    /// (WinUI's, both themes).
    fn close_pressed() -> Color32 {
        CLOSE_HOVER().gamma_multiply(0.9)
    }

    /// A drawn title bar's height and one caption button's width, in points (Windows 11's).
    pub const CAPTION_H: f32 = 32.0;
    pub const CAPTION_BUTTON_W: f32 = 46.0;
    /// Where the window's icon ends: the system menu's place in the title bar.
    pub const CAPTION_ICON_W: f32 = 32.0;

    /// How the theme's title bar is laid out (`palette::CaptionLook`).
    #[derive(Clone, Copy)]
    struct Style {
        /// One button's width (Close's its own: Windows 7's is wider), and the room after the
        /// last one at the window's end.
        button_w: f32,
        close_w: f32,
        end: f32,
        /// GNOME's round buttons (a circle this wide, tinted at rest), else Windows' tiles the
        /// bar's height.
        circle: Option<f32>,
        /// The window's icon at the start (the system menu's place).
        icon: bool,
        /// The title centred in the bar (else after the icon), and in bold.
        centred: bool,
        bold: bool,
        /// A button under the pointer or pressed draws its glyph in the bar's colour (KDE).
        invert: bool,
        /// The era themes (#45): the bar and its buttons shaded in the era's way, each button
        /// a tile of this size (a zero width: its place's) with a face of its own at rest, hung
        /// from the bar's top (`hang`, Windows 7) or centred in it.
        era: Option<Shine>,
        tile: egui::Vec2,
        hang: bool,
    }

    fn style() -> Style {
        use crate::palette::CaptionLook;
        let windows = Style {
            button_w: CAPTION_BUTTON_W,
            close_w: CAPTION_BUTTON_W,
            end: 0.0,
            circle: None,
            icon: true,
            centred: false,
            bold: false,
            invert: false,
            era: None,
            tile: egui::Vec2::ZERO,
            hang: false,
        };
        match crate::theme::current().controls.caption {
            CaptionLook::System => windows,
            CaptionLook::Metro => Style {
                centred: true,
                ..windows
            },
            // libadwaita's header bar: 24-point round buttons 6 apart, no icon, a bold title.
            CaptionLook::Adwaita => Style {
                button_w: 30.0,
                close_w: 30.0,
                end: 4.0,
                circle: Some(24.0),
                icon: false,
                centred: true,
                bold: true,
                ..windows
            },
            // KDE's Breeze: the icon, the title centred, 18-point round buttons shown only
            // under the pointer, in the title's colour with the glyph in the bar's.
            CaptionLook::Breeze => Style {
                button_w: 26.0,
                close_w: 26.0,
                end: 4.0,
                circle: Some(18.0),
                centred: true,
                invert: true,
                ..windows
            },
            // Windows XP: the icon and a bold title, 21-point square buttons with a light edge.
            CaptionLook::Luna => Style {
                button_w: 23.0,
                close_w: 23.0,
                end: 5.0,
                bold: true,
                era: Some(Shine::Luna),
                tile: vec2(21.0, 21.0),
                ..windows
            },
            // Windows 7: the icon and the title, buttons 20 points high hung from the bar's
            // top, side by side, Close wider.
            CaptionLook::Aero => Style {
                button_w: 27.0,
                close_w: 46.0,
                end: 6.0,
                era: Some(Shine::Aero),
                tile: vec2(0.0, 20.0),
                hang: true,
                ..windows
            },
            // Mac OS X: no icon, the title centred, round 15-point buttons.
            CaptionLook::Aqua => Style {
                button_w: 23.0,
                close_w: 23.0,
                end: 8.0,
                icon: false,
                centred: true,
                era: Some(Shine::Aqua),
                tile: vec2(15.0, 15.0),
                ..windows
            },
        }
    }

    /// The bar's colour: the active one, or the theme's inactive one while the window isn't
    /// the active one (where it has one).
    fn bar_colour(active: bool) -> Color32 {
        let inactive = crate::theme::colors().caption_inactive;
        if !active && inactive.a() > 0 {
            inactive
        } else {
            CAPTION_BG()
        }
    }

    /// An era's bar, shaded from its top: XP's light edge on a deep colour, Windows 7's glassy
    /// halves, Mac OS X's light grey falling to the bar's colour.
    fn bar_stops(era: Shine, bar: Color32, g: Color32) -> Vec<(f32, Color32)> {
        use crate::derive::{mix, shade};
        match era {
            Shine::Luna => vec![
                (0.0, mix(bar, g, 0.45)),
                (0.12, mix(bar, g, 0.12)),
                (0.5, bar),
                (1.0, shade(bar, -0.06)),
            ],
            Shine::Aero => vec![
                (0.0, mix(bar, g, 0.45)),
                (0.5, mix(bar, g, 0.2)),
                (0.5, bar),
                (1.0, mix(bar, g, 0.1)),
            ],
            Shine::Aqua => vec![(0.0, mix(bar, g, 0.6)), (1.0, bar)],
        }
    }

    /// A caption button of a title bar we draw ourselves, from the window's end inwards: Close,
    /// Maximize, Minimize, then ours.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum CaptionButton {
        Close,
        Maximize,
        Minimize,
        /// Hide the window in the notification area (#63).
        ToTray,
    }

    impl CaptionButton {
        /// Its accessible name, English (translated where shown).
        fn name(self) -> &'static str {
            match self {
                // i18n: a window's title-bar button: closes the window
                CaptionButton::Close => crate::i18n::mark("Close"),
                // i18n: a window's title-bar button: makes the window fill the screen
                CaptionButton::Maximize => crate::i18n::mark("Maximize"),
                // i18n: a window's title-bar button: minimizes the window
                CaptionButton::Minimize => crate::i18n::mark("Minimize"),
                // i18n: tooltip of a button in a download's progress window: hides the window to the notification area (system tray); the download goes on
                CaptionButton::ToTray => crate::i18n::mark("Hide to the notification area"),
            }
        }

        /// The theme icon set's name for its glyph (`crate::themeicons`).
        fn need(self, maximized: bool) -> &'static str {
            match self {
                CaptionButton::Close => "ui-close",
                CaptionButton::Maximize if maximized => "ui-restore",
                CaptionButton::Maximize => "ui-maximize",
                CaptionButton::Minimize => "ui-minimize",
                CaptionButton::ToTray => "ui-to-tray",
            }
        }

        /// Its glyph: the theme's icon set's where it has one, else Segoe Fluent's shapes drawn
        /// as our own vectors, 10 px across and 1 px wide at `size` 1 and `width` 1 (Restore's
        /// two squares while maximized).
        #[allow(clippy::too_many_arguments)]
        fn glyph(
            self,
            ui: &Ui,
            p: &Painter,
            c: Pos2,
            ink: Color32,
            maximized: bool,
            size: f32,
            width: f32,
        ) {
            let px = 16.0 * size * ui.ctx().pixels_per_point();
            if let Some((src, tint)) =
                crate::themeicons::source(self.need(maximized), px, ink, true)
            {
                let rc = Rect::from_center_size(c, vec2(16.0, 16.0) * size);
                let img = egui::Image::new(src);
                if let Ok(egui::load::TexturePoll::Ready { texture }) =
                    img.load_for_size(ui.ctx(), rc.size())
                {
                    p.image(
                        texture.id,
                        rc,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        tint,
                    );
                    return;
                }
            }
            let s = Stroke::new(width, ink);
            let v = |x: f32, y: f32| c + vec2(x, y) * size;
            if self == CaptionButton::Maximize && maximized {
                p.rect_stroke(
                    Rect::from_center_size(v(-1.0, 1.0), vec2(8.0, 8.0) * size),
                    1.0,
                    s,
                    egui::StrokeKind::Inside,
                );
                p.line(
                    vec![
                        v(-1.5, -3.5),
                        v(-1.5, -4.5),
                        v(4.5, -4.5),
                        v(4.5, 1.5),
                        v(3.5, 1.5),
                    ],
                    s,
                );
                return;
            }
            match self {
                CaptionButton::Minimize => {
                    p.line_segment([v(-5.0, 0.5), v(5.0, 0.5)], s);
                }
                CaptionButton::Maximize => {
                    p.rect_stroke(
                        Rect::from_center_size(c, vec2(9.0, 9.0) * size),
                        1.0,
                        s,
                        egui::StrokeKind::Inside,
                    );
                }
                CaptionButton::Close => {
                    p.line_segment([v(-5.0, -5.0), v(5.0, 5.0)], s);
                    p.line_segment([v(-5.0, 5.0), v(5.0, -5.0)], s);
                }
                CaptionButton::ToTray => to_tray_glyph_sized(p, c, s, rtl(), size),
            }
        }
    }

    /// Windows keeps the window's own title bar in UI Automation (its "TitleBar", built from the
    /// window's styles, whatever is drawn): its Minimize, Maximize and Close are spoken, and work,
    /// through the system. Ours would say them a second time, so they stay out of the
    /// accessibility tree there; only our own button (Hide) is exposed. Brave and VS Code have no
    /// such TitleBar only because Chromium's own provider replaces the window's; AccessKit's
    /// doesn't, and the system menu, Alt+Space and taskbar minimize need the styles kept.
    const SYSTEM_SPEAKS: bool = cfg!(windows);

    thread_local! {
        /// The next title bar's Maximize as the window's frame sees it (hot, pressed): where the
        /// frame answers for it (Snap Layouts, win32::drawn_frame), egui never sees the pointer
        /// there.
        static FRAME_MAX: std::cell::Cell<(bool, bool)> = const { std::cell::Cell::new((false, false)) };
    }

    /// Tell the next [`title_bar`] how its window's frame sees Maximize: under the pointer,
    /// pressed.
    pub fn frame_maximize(hot: bool, pressed: bool) {
        FRAME_MAX.with(|c| c.set((hot, pressed)));
    }

    /// Keep `resp`'s node out of the accessibility tree (screen readers hear the system's).
    fn unspoken(ui: &Ui, resp: &Response) {
        ui.ctx().accesskit_node_builder(resp.id, |n| n.set_hidden());
    }

    /// One caption button: Windows 11's 46 × 32, a hover and a pressed tint (Close turns red, its
    /// glyph white), greyed when `enabled` is false, its glyph grey while the window is inactive.
    /// Exposed as a button with its name; a tooltip only for ours (the system's have none).
    fn caption_button(
        ui: &mut Ui,
        rc: Rect,
        b: CaptionButton,
        enabled: bool,
        active: bool,
    ) -> bool {
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let resp = ui.interact(rc, widget_id(ui, ("caption-button", b.name())), sense);
        let name = crate::i18n::t(b.name());
        if b == CaptionButton::ToTray || !SYSTEM_SPEAKS {
            expose(&resp, egui::Role::Button, name, enabled, None);
        } else {
            unspoken(ui, &resp);
        }
        let p = ui.painter();
        let (frame_hot, frame_pressed) = if b == CaptionButton::Maximize {
            FRAME_MAX.with(|c| c.get())
        } else {
            (false, false)
        };
        let hover = enabled && (resp.hovered() || frame_hot);
        let pressed = enabled && (resp.is_pointer_button_down_on() || frame_pressed);
        let close = b == CaptionButton::Close;
        let st = style();
        let maximized = ui.ctx().input(|i| i.viewport().maximized) == Some(true);
        if let Some(era) = st.era {
            era_button(
                ui,
                &resp,
                rc,
                b,
                era,
                (hover, pressed, enabled, active),
                maximized,
            );
            let clicked = resp.clicked();
            if b == CaptionButton::ToTray {
                resp.on_hover_text(name);
            }
            return clicked;
        }
        let circle = st.circle;
        let fill = match (pressed, hover, close) {
            (true, _, true) => Some(close_pressed()),
            (false, true, true) => Some(CLOSE_HOVER()),
            (true, _, false) => Some(CAPTION_PRESSED()),
            (false, true, false) => Some(CAPTION_HOVER()),
            // GNOME's round buttons show at rest too.
            _ if circle.is_some() && enabled => Some(CAPTION_BUTTON()),
            _ => None,
        };
        if let Some(f) = fill {
            match circle {
                Some(d) => {
                    p.circle_filled(rc.center(), d / 2.0, f);
                }
                None => {
                    p.rect_filled(rc, 0.0, f);
                }
            }
        }
        let ink = if st.invert && (hover || pressed) {
            CAPTION_BG()
        } else if close && pressed {
            CLOSE_INK().gamma_multiply(0.7)
        } else if close && hover {
            CLOSE_INK()
        } else if !enabled || !active {
            caption_ink_inactive()
        } else {
            CAPTION_INK()
        };
        b.glyph(ui, p, rc.center().round(), ink, maximized, 1.0, 1.0);
        let ring = match circle {
            Some(d) => Rect::from_center_size(rc.center(), vec2(d, d)),
            None => rc.shrink(3.0),
        };
        show_focus(ui, &resp, ring);
        let clicked = resp.clicked();
        if b == CaptionButton::ToTray {
            resp.on_hover_text(name);
        }
        clicked
    }

    /// An era theme's caption button (#45) in its place `rc`: a shaded tile with an edge, its
    /// own face at rest (Close red; Mac OS X's graphite until the pointer is on it), lighter
    /// under the pointer and darker pressed, faded while greyed or the window inactive; the
    /// glyph light on it (Windows 7's with a shadow), or Mac OS X's dark one shown only under
    /// the pointer. There, our Hide button is a plain picture in the bar, as a toolbar's.
    fn era_button(
        ui: &Ui,
        resp: &Response,
        rc: Rect,
        b: CaptionButton,
        era: Shine,
        (hover, pressed, enabled, active): (bool, bool, bool, bool),
        maximized: bool,
    ) {
        use crate::derive::{mix, shade};
        let p = ui.painter();
        let c = crate::theme::colors();
        let st = style();
        let close = b == CaptionButton::Close;
        let bar = bar_colour(active);
        let at = rc.center().round();
        if era == Shine::Aqua && b == CaptionButton::ToTray {
            if hover || pressed {
                let tint = if pressed {
                    CAPTION_PRESSED()
                } else {
                    CAPTION_HOVER()
                };
                p.rect_filled(Rect::from_center_size(at, vec2(22.0, 22.0)), 4.0, tint);
            }
            let ink = if enabled && active {
                CAPTION_INK()
            } else {
                caption_ink_inactive()
            };
            b.glyph(ui, p, at, ink, maximized, 1.0, 1.0);
            show_focus(ui, resp, Rect::from_center_size(at, vec2(22.0, 22.0)));
            return;
        }
        let size = vec2(
            if st.tile.x > 0.0 {
                st.tile.x
            } else {
                rc.width()
            },
            st.tile.y,
        );
        let tile = if st.hang {
            Rect::from_min_size(pos2(at.x - size.x / 2.0, rc.top()), size)
        } else {
            Rect::from_center_size(at, size)
        };
        let base = if close && (era != Shine::Aqua || hover || pressed) {
            CLOSE_HOVER()
        } else {
            CAPTION_BUTTON()
        };
        let base = if pressed {
            shade(base, -0.1)
        } else if hover {
            mix(base, c.gloss, 0.3)
        } else {
            base
        };
        let base = if enabled && active {
            base
        } else {
            mix(base, bar, 0.55)
        };
        let (top, bottom) = match era {
            Shine::Luna => (3.0, 3.0),
            Shine::Aero => (0.0, 4.0),
            Shine::Aqua => (size.y / 2.0, size.y / 2.0),
        };
        fill_shaded_corners(p, tile, top, bottom, &era.stops(base, c.gloss));
        let edge = match era {
            Shine::Luna => c.gloss.gamma_multiply(0.9),
            Shine::Aero => shade(bar, -0.35),
            Shine::Aqua => shade(base, -0.2),
        };
        let corners = egui::CornerRadius {
            nw: top as u8,
            ne: top as u8,
            sw: bottom as u8,
            se: bottom as u8,
        };
        p.rect_stroke(tile, corners, Stroke::new(1.0, edge), StrokeKind::Inside);
        let (size, width) = match era {
            Shine::Luna => (0.8, 2.0),
            Shine::Aero => (0.9, 1.5),
            Shine::Aqua => (0.55, 1.4),
        };
        let at = tile.center().round();
        if era == Shine::Aqua {
            // Mac OS X shows its buttons' glyphs only under the pointer.
            if hover || pressed {
                b.glyph(ui, p, at, CAPTION_INK(), maximized, size, width);
            }
        } else {
            let ink = if enabled && active {
                CLOSE_INK()
            } else {
                CLOSE_INK().gamma_multiply(0.7)
            };
            if era == Shine::Aero {
                let shadow = Color32::from_black_alpha(110);
                b.glyph(ui, p, at + vec2(1.0, 1.0), shadow, maximized, size, width);
            }
            b.glyph(ui, p, at, ink, maximized, size, width);
        }
        show_focus(ui, resp, tile);
    }

    /// What a drawn title bar did: the button pressed, and where its buttons and its icon are (in
    /// points), for the window's hit test (win32::set_caption_hits).
    pub struct TitleBar {
        pub pressed: Option<CaptionButton>,
        pub buttons: Vec<Rect>,
        pub icon: Rect,
        /// The colour along the bar's top edge (the sizing frame above the main window's bar
        /// takes it: win32::caption_colour). Only Windows draws title bars (elsewhere unread).
        #[cfg_attr(not(windows), allow(dead_code))]
        pub top: Color32,
    }

    /// A title bar we draw ourselves (where the window's frame is ours: win32::thin_frame): the
    /// window's icon and `title`, then `buttons` (each with its enabled state) from the window's end
    /// inwards, mirrored right to left. The title is exposed as the bar's label. The rest of the bar
    /// is the caption the system drags, snaps and opens the system menu from (win32's hit test):
    /// egui only gets the buttons.
    pub fn title_bar(
        ui: &mut Ui,
        rect: Rect,
        title: &str,
        buttons: &[(CaptionButton, bool)],
    ) -> TitleBar {
        // MDM_TEST_ACTIVE: every bar drawn as the active window's (pictures of windows off the
        // screens, which never have the focus).
        static ALWAYS_ACTIVE: std::sync::LazyLock<bool> =
            std::sync::LazyLock::new(|| std::env::var_os("MDM_TEST_ACTIVE").is_some());
        let active = *ALWAYS_ACTIVE || ui.ctx().input(|i| i.viewport().focused) != Some(false);
        let rtl = rtl();
        let p = ui.painter().clone();
        let st = style();
        let bar = bar_colour(active);
        let top = match st.era {
            Some(era) => {
                let stops = bar_stops(era, bar, crate::theme::colors().gloss);
                fill_shaded(&p, rect, 0.0, &stops);
                stops.first().map_or(bar, |s| s.1)
            }
            None => {
                p.rect_filled(rect, 0.0, bar);
                bar
            }
        };
        let mut pressed = None;
        let mut rects = Vec::with_capacity(buttons.len());
        // From the window's end inwards, each as wide as its kind is.
        let mut from_end = st.end;
        for (b, enabled) in buttons {
            let bw = if *b == CaptionButton::Close {
                st.close_w
            } else {
                st.button_w
            };
            let x0 = if rtl {
                rect.left() + from_end
            } else {
                rect.right() - from_end - bw
            };
            from_end += bw;
            let rc = Rect::from_min_size(pos2(x0, rect.top()), vec2(bw, rect.height()));
            if caption_button(ui, rc, *b, *enabled, active) {
                pressed = Some(*b);
            }
            rects.push(rc);
        }
        // The frame's word on Maximize holds for this bar only.
        FRAME_MAX.with(|c| c.set((false, false)));
        let buttons_w = from_end;
        // Without an icon, the title keeps a little room from the bar's start.
        let icon_w = if st.icon { CAPTION_ICON_W } else { 12.0 };
        if st.icon {
            let icon_c = if rtl {
                pos2(rect.right() - 16.0, rect.center().y)
            } else {
                pos2(rect.left() + 16.0, rect.center().y)
            };
            egui::Image::new(egui::include_image!("../assets/icons/app/mdm-32.png"))
                .paint_at(ui, Rect::from_center_size(icon_c.round(), vec2(16.0, 16.0)));
        }
        let font = if st.bold {
            FontId::new(segoe().size, egui::FontFamily::Name("tahoma-bold".into()))
        } else {
            segoe()
        };
        let room = rect.width() - icon_w - buttons_w - 12.0;
        let shown = ellipsize(&p, &font, title, room.max(0.0));
        let ink = if active {
            CAPTION_INK()
        } else {
            caption_ink_inactive()
        };
        let (anchor, x) = if rtl {
            (egui::Align2::RIGHT_CENTER, rect.right() - icon_w)
        } else {
            (egui::Align2::LEFT_CENTER, rect.left() + icon_w)
        };
        // Windows 8 and GNOME centre the title in the bar (kept clear of the icon and the
        // buttons).
        let (anchor, x) = if st.centred {
            let half = text_width_font(&p, font.clone(), &shown) / 2.0;
            let (lo, hi) = if rtl {
                (rect.left() + buttons_w + half, rect.right() - icon_w - half)
            } else {
                (rect.left() + icon_w + half, rect.right() - buttons_w - half)
            };
            (
                egui::Align2::CENTER_CENTER,
                rect.center().x.clamp(lo.min(hi), hi.max(lo)),
            )
        } else {
            (anchor, x)
        };
        let galley = p.layout_no_wrap(shown.clone(), font.clone(), ink);
        let drawn = anchor.anchor_size(pos2(x, rect.center().y), galley.size());
        let c = crate::theme::colors();
        match st.era {
            // Windows XP: a dark shadow under the title.
            Some(Shine::Luna) => {
                let shadow = crate::derive::shade(bar, -0.25);
                let g = p.layout_no_wrap(shown.clone(), font.clone(), shadow);
                p.galley(drawn.min + vec2(1.0, 1.0), g, shadow);
            }
            // Windows 7: the title on a soft glow (the light variant's dark title).
            Some(Shine::Aero) if !c.dark => {
                let glow = egui::epaint::Shadow {
                    offset: [0, 0],
                    blur: 14,
                    spread: 1,
                    color: c.gloss.gamma_multiply(0.75),
                };
                p.add(glow.as_shape(drawn.expand2(vec2(6.0, 0.0)), 8));
            }
            // Mac OS X: a light line under the title, as if pressed into the bar.
            Some(Shine::Aqua) if !c.dark => {
                let light = c.gloss.gamma_multiply(0.7);
                let g = p.layout_no_wrap(shown.clone(), font.clone(), light);
                p.galley(drawn.min + vec2(0.0, 1.0), g, light);
            }
            _ => {}
        }
        p.galley(drawn.min, galley, ink);
        let label = ui.interact(drawn, widget_id(ui, "caption-title"), Sense::hover());
        // The window's own name says the title to screen readers (where the system speaks for
        // the title bar); elsewhere the label does.
        if SYSTEM_SPEAKS {
            unspoken(ui, &label);
        } else {
            expose(&label, egui::Role::Label, title, true, None);
        }
        if shown != title {
            label.on_hover_text(title);
        }
        // Without an icon, no system-menu place (Alt+Space and a right-click on the bar still
        // open the system menu).
        let icon = if !st.icon {
            Rect::from_min_size(rect.min, egui::Vec2::ZERO)
        } else if rtl {
            Rect::from_min_max(pos2(rect.right() - CAPTION_ICON_W, rect.top()), rect.max)
        } else {
            Rect::from_min_size(rect.min, vec2(CAPTION_ICON_W, rect.height()))
        };
        TitleBar {
            pressed,
            buttons: rects,
            icon,
            top,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `r` already gives the screen rect (mirrored in RTL); `rect` around it mirrors it back,
    /// so the thing stays at its English place in Arabic (#170: the browser offer window's logo
    /// over its subline). No window does that.
    #[test]
    fn no_rect_is_mirrored_twice() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let twice = ["rect(r(", "rect(winui::r("];
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (n, line) in src.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                if twice.iter().any(|t| code.contains(t)) && !code.contains("\"rect(") {
                    found.push(format!("{}:{}", path.display(), n + 1));
                }
            }
        }
        assert!(found.is_empty(), "rect(r(...)) mirrors twice: {found:?}");
    }

    /// `f` with a painter whose Tahoma is the interface font (as on macOS and Linux).
    fn with_painter(f: impl FnOnce(&Painter)) {
        let ctx = egui::Context::default();
        let mut defs = egui::FontDefinitions::default();
        install_fonts(&mut defs, None);
        ctx.set_fonts(defs);
        let mut f = Some(f);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(f) = f.take() {
                f(ui.painter());
            }
        });
        out.textures_delta.clear();
    }

    #[test]
    fn a_single_line_label_never_grows_another_row() {
        with_painter(|p| {
            // A title that fits is drawn as it is, with nothing to hover.
            let (rc, cut) = text_fit(p, 11.0, 70.0, "A short title", TEXT(), 489.0);
            assert!(!cut);
            assert!(rc.right() <= 489.0);
            // An empty one draws nothing and is not a cut.
            let (rc, cut) = text_fit(p, 11.0, 70.0, "", TEXT(), 489.0);
            assert!(!cut);
            assert_eq!(rc.width(), 0.0);
            // A column with room for the "..." alone stays in its column.
            let right = 11.0 + text_width(p, "...") + 1.0;
            let (rc, cut) = text_fit(p, 11.0, 70.0, "A title", TEXT(), right);
            assert!(cut);
            assert!(rc.right() <= right);
            // A long title stays one row, whatever it has in it: unbroken, wide Unicode, and
            // line breaks (a site's title). A cut says the whole original is shown on hover.
            let cases = [
                "W".repeat(2000),
                "日本語".repeat(500),
                "مرحبا بالعالم ".repeat(100),
                "🎬".repeat(500),
                "Short\ntitle".to_owned(),
                "Short\rtitle".to_owned(),
                "Long title\r\nwith more than one line\nand a third".to_owned(),
                "\n".to_owned(),
                "a\n".repeat(100),
            ];
            for s in cases {
                let (rc, cut) = text_fit(p, 11.0, 70.0, &s, TEXT(), 489.0);
                assert!(cut, "not cut, so no whole text on hover: {s:?}");
                assert!(rc.right() <= 489.0, "past its right edge: {s:?}");
                assert!(rc.height() <= 20.0, "more than one row: {s:?}");
            }
        });
    }

    #[test]
    fn a_row_of_buttons_widens_for_long_labels_only() {
        with_painter(|p| {
            // Short labels keep their English places (Download video's bottom row).
            let row = [("Later", 73.0), ("Cancel", 73.0)];
            assert_eq!(
                buttons_to_right(p, 489.0, 8.0, &row, 160.0),
                vec![(335.0, 73.0), (416.0, 73.0)]
            );
            assert_eq!(
                buttons_from_left(p, 11.0, 8.0, &row, 160.0),
                vec![(11.0, 73.0), (92.0, 73.0)]
            );
            // A long one is as wide as its text and the room inside the border, up to the most
            // allowed; the row keeps its end and its gaps.
            let long = "A label much longer than its English button";
            let want = (text_width(p, long) + 16.0).ceil();
            let row = [(long, 90.0), ("Cancel", 73.0)];
            let placed = buttons_to_right(p, 489.0, 8.0, &row, 400.0);
            assert_eq!(placed[0], (489.0 - 73.0 - 8.0 - want, want));
            assert_eq!(placed[1], (416.0, 73.0));
            let placed = buttons_from_left(p, 11.0, 8.0, &row, 400.0);
            assert_eq!(placed[1].0, 11.0 + want + 8.0);
            assert_eq!(buttons_to_right(p, 489.0, 8.0, &row, 120.0)[0].1, 120.0);
            // Its label then fits whole (no "...").
            let rc = Rect::from_min_size(pos2(0.0, 0.0), vec2(want, 21.0));
            assert!(!fit_label(p, long, rc).1);
        });
    }

    #[test]
    fn a_row_keeps_its_english_places_and_uneven_gaps() {
        // Download File Info's row: gaps of 19 and 18 px, ending at 461.
        let row = [
            (97.0, 109.0, 90.0),
            (225.0, 109.0, 95.0),
            (352.0, 109.0, 50.0),
        ];
        assert_eq!(
            place_row(&row, 19.0, 11.0, 200.0),
            vec![(97.0, 109.0), (225.0, 109.0), (352.0, 109.0)]
        );
        // The middle one grows: it widens to the left and pushes the first, the gaps kept.
        let row = [
            (97.0, 109.0, 90.0),
            (225.0, 109.0, 150.0),
            (352.0, 109.0, 50.0),
        ];
        assert_eq!(
            place_row(&row, 19.0, 11.0, 200.0),
            vec![(56.0, 109.0), (184.0, 150.0), (352.0, 109.0)]
        );
        // A button with room before it grows into it without moving the others.
        let row = [
            (30.0, 138.0, 100.0),
            (311.0, 82.0, 120.0),
            (422.0, 73.0, 60.0),
        ];
        assert_eq!(
            place_row(&row, 8.0, 30.0, 200.0),
            vec![(30.0, 138.0), (273.0, 120.0), (422.0, 73.0)]
        );
        // The last one grows into the room before it, then pushes, `gap` apart.
        let row = [(311.0, 82.0, 60.0), (422.0, 73.0, 120.0)];
        assert_eq!(
            place_row(&row, 8.0, 0.0, 200.0),
            vec![(285.0, 82.0), (375.0, 120.0)]
        );
        // At most `max`.
        let row = [(311.0, 82.0, 60.0), (422.0, 73.0, 300.0)];
        assert_eq!(place_row(&row, 8.0, 0.0, 150.0)[1], (345.0, 150.0));
        // The Scheduler's row: the first, at the row's start, grows; those it reaches move right
        // as little as they must, the rest keep their places.
        let english = [21.0, 102.0, 216.0, 297.0, 390.0, 484.0, 565.0];
        let row: Vec<(f32, f32, f32)> = english
            .iter()
            .enumerate()
            .map(|(i, &x)| (x, 73.0, if i == 0 { 116.0 } else { 60.0 }))
            .collect();
        let xs: Vec<f32> = place_row(&row, 8.0, 21.0, 200.0)
            .iter()
            .map(|p| p.0)
            .collect();
        assert_eq!(xs, [21.0, 145.0, 226.0, 307.0, 390.0, 484.0, 565.0]);
    }

    #[test]
    fn a_full_row_takes_room_from_buttons_wider_than_their_label() {
        // The crash report window's row: 588 px from 16 to 604, gaps of 6, no room to spare.
        let english = [
            (16.0, 170.0),
            (192.0, 110.0),
            (308.0, 110.0),
            (424.0, 112.0),
            (542.0, 62.0),
        ];
        let with = |fits: [f32; 5]| -> Vec<(f32, f32, f32)> {
            english
                .iter()
                .zip(fits)
                .map(|(&(x, w), f)| (x, w, f))
                .collect()
        };
        // English: as it is.
        let row = with([100.0, 80.0, 75.0, 110.0, 50.0]);
        assert_eq!(place_row(&row, 6.0, 16.0, 260.0), english.to_vec());
        // The fourth needs 166: the room is taken from those with the most to spare (the first,
        // then the third); the row keeps both its ends and its gaps.
        let row = with([110.0, 100.0, 95.0, 166.0, 50.0]);
        let placed = place_row(&row, 6.0, 16.0, 260.0);
        assert_eq!(placed[0], (16.0, 121.0));
        assert_eq!(placed[1].1, 110.0);
        assert_eq!(placed[2].1, 105.0);
        assert_eq!(placed[3].1, 166.0);
        assert_eq!(placed[4], (542.0, 62.0));
        for w in placed.windows(2) {
            assert_eq!(w[1].0 - (w[0].0 + w[0].1), 6.0);
        }
        // Not even then: no button goes under 73 px or its label, and the widest widened ones
        // are narrowed (back to their English width at most).
        let row = with([200.0, 150.0, 150.0, 250.0, 50.0]);
        let placed = place_row(&row, 6.0, 16.0, 260.0);
        let total: f32 = placed.iter().map(|p| p.1).sum::<f32>() + 4.0 * 6.0;
        assert_eq!(placed[0].0, 16.0);
        assert_eq!(total, 588.0);
        assert!(placed.iter().zip(&english).all(|(p, e)| p.1 >= e.1));
        assert!(
            placed
                .iter()
                .zip(&row)
                .all(|(p, r)| p.1 < r.2.max(r.1 + 1.0))
        );
    }

    #[test]
    fn a_label_that_fits_keeps_its_button_as_it_is() {
        with_painter(|p| {
            // Whole with less than the usual room around it (Download a gallery's English
            // Install button): not widened, alone or in a row.
            let label = "Install gallery-dl";
            let text = text_width(p, label);
            let tight = (text + 6.0).ceil();
            assert_eq!(button_fit_width(p, label, tight, 300.0), tight);
            assert_eq!(
                button_row(p, &[(label, 100.0, tight)], 8.0, 0.0, 300.0),
                vec![(100.0, tight)]
            );
            // Cut: as wide as the label and the usual room.
            let cut = (text + 2.0).floor();
            assert_eq!(button_fit_width(p, label, cut, 300.0), (text + 16.0).ceil());
        });
    }

    #[test]
    fn a_title_takes_a_second_line_only_where_one_is_too_long() {
        with_painter(|p| {
            let font = tahoma_bold();
            let s = "Proxy socks configuration page";
            let whole = text_width_font(p, font.clone(), s);
            // Room for it: one line, as it is.
            assert_eq!(fit_lines(p, font.clone(), s, whole), vec![s]);
            // Short of room: two lines, broken where the wider is narrowest.
            let two = fit_lines(p, font.clone(), s, whole - 1.0);
            assert_eq!(two, vec!["Proxy socks", "configuration page"]);
            // After a slash or a hyphen, the mark kept on the first line.
            let two = fit_lines(p, font.clone(), "Proxyserver/SOCKS-konfiguration", 10.0);
            assert_eq!(two.concat(), "Proxyserver/SOCKS-konfiguration");
            assert!(two[0].ends_with('/') || two[0].ends_with('-'), "{two:?}");
            // A word with no place to break stays whole.
            assert_eq!(
                fit_lines(p, font.clone(), "Konfiguration", 10.0),
                vec!["Konfiguration"]
            );
            // Chinese breaks between its characters.
            assert_eq!(fit_lines(p, font, "代理服务器配置", 10.0).len(), 2);
        });
    }

    #[test]
    fn a_drop_down_is_as_wide_as_its_widest_choice() {
        with_painter(|p| {
            // Short choices: its English width.
            assert_eq!(
                combo_fit_width(p, &["Sleep", "Hibernate"], 119.0, 300.0),
                119.0
            );
            // A long one: it and the chevron's room, whichever is chosen.
            let long = "A choice much longer than its English box";
            let want = (text_width(p, long) + 24.0).ceil();
            assert_eq!(combo_fit_width(p, &["Sleep", long], 119.0, 300.0), want);
            // No room for it: as wide as there is (it ends in "..."), never below English.
            assert_eq!(combo_fit_width(p, &[long], 119.0, 150.0), 150.0);
            assert_eq!(combo_fit_width(p, &[long], 119.0, 80.0), 119.0);
        });
    }

    #[test]
    fn a_row_from_the_left_widens_to_the_right() {
        with_painter(|p| {
            let long = "A label much longer than its English button";
            let want = (text_width(p, long) + 16.0).ceil();
            // Short labels: their English places.
            let row = [("OK", 11.0, 90.0), ("Cancel", 107.0, 90.0)];
            assert_eq!(
                button_row_from_left(p, &row, 6.0, 400.0, 300.0),
                vec![(11.0, 90.0), (107.0, 90.0)]
            );
            assert_eq!(
                button_row(p, &row, 6.0, 0.0, 300.0),
                vec![(11.0, 90.0), (107.0, 90.0)]
            );
            // A long first one pushes the second right, the gap kept.
            let row = [(long, 11.0, 90.0), ("Cancel", 107.0, 90.0)];
            let placed = button_row_from_left(p, &row, 6.0, 600.0, 400.0);
            assert_eq!(placed, vec![(11.0, want), (11.0 + want + 6.0, 90.0)]);
            // The same as a row ending at the right: the first grows to the left.
            let placed = button_row(p, &row, 6.0, -400.0, 400.0);
            assert_eq!(placed, vec![(101.0 - want, want), (107.0, 90.0)]);
            // Toggling labels: laid out for the widest translation.
            assert_eq!(widest_label(p, &["OK", long]), long);
            // A lone button widens around its centre, inside its room. (Labels from variables:
            // the catalog's extractor takes literal ones as texts to translate.)
            let short = "OK";
            assert_eq!(
                button_around(p, short, 480.0, 73.0, 460.0, 562.0),
                (480.0, 73.0)
            );
            assert_eq!(
                button_around(p, long, 480.0, 73.0, 460.0, 562.0),
                (460.0, 102.0)
            );
            let label = "Preview the part downloaded";
            let w = (text_width(p, label) + 16.0).ceil();
            let (x, got) = button_around(p, label, 480.0, 73.0, 400.0, 600.0);
            assert_eq!(got, w);
            assert!((x + w / 2.0 - 516.5).abs() <= 1.0);
        });
    }

    #[test]
    fn data_keeps_its_own_lines_and_reads_left_to_right() {
        with_painter(|p| {
            // A report's lines (Windows' line ends too) stay lines; only a long one wraps.
            let report = "Kind: freeze\r\nTime: 2026-11-06 10:00:01\n\n  at mdm::app::update\nend";
            assert_eq!(
                wrap_lines(p, report, 1000.0),
                vec![
                    "Kind: freeze",
                    "Time: 2026-11-06 10:00:01",
                    "",
                    "  at mdm::app::update",
                    "end"
                ]
            );
            let long = "one two three four five six seven eight nine ten";
            let lines = wrap_lines(p, long, text_width(p, "one two three") + 1.0);
            assert_eq!(lines[0], "one two three");
            assert!(
                lines
                    .iter()
                    .all(|l| text_width(p, l) <= text_width(p, "one two three") + 1.0)
            );
            // Drawn: as many lines (a translation's paragraph is flowed again, data never).
            assert_eq!(
                data_paragraph(p, 24.0, 100.0, report, TEXT(), 400.0, 14.0),
                5
            );
        });
    }

    #[test]
    fn data_starts_at_the_left_of_its_room_in_rtl_too() {
        assert_eq!(data_left(24.0, 400.0), 24.0);
        // English 24..400 in a 600 px window, mirrored: on screen 200..576, read from 200.
        let _s = Surface::enter_if(true, 0.0, 600.0);
        assert_eq!(data_left(24.0, 400.0), 200.0);
    }

    #[test]
    fn a_widened_button_grows_into_the_room_before_it_in_rtl_too() {
        with_painter(|p| {
            let long = "A label much longer than its English button";
            let english = Rect::from_min_size(pos2(401.0, 135.0), vec2(96.0, 21.0));
            let ltr = button_rect_for(p, english, long, 300.0);
            assert_eq!(ltr.right(), 497.0);
            assert!(ltr.left() < 401.0);
            // Mirrored: the button is at the window's left, and grows to the right (inwards).
            let _s = Surface::enter_if(true, 0.0, 600.0);
            let shown = button_rect_for(p, rect(english), long, 300.0);
            assert_eq!(shown.left(), 600.0 - 497.0);
            assert_eq!(shown.width(), ltr.width());
            assert_eq!(rect(shown), ltr);
        });
    }

    #[test]
    fn a_path_is_shortened_in_the_middle_and_keeps_its_file_name() {
        let ctx = egui::Context::default();
        let font = FontId::proportional(11.0);
        let path = r"C:\Users\Someone\Downloads\Video\Ştirile PRO TV - 4 Noiembrie 2026 _ Cutremur politic după alegerile din Statele Unite.mp4";
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let p = ui.painter();
            let width = |s: &str| text_width_font(p, font.clone(), s);
            // Fits: as it is.
            assert_eq!(ellipsize_middle(p, &font, "C:\\a.mp4", 400.0), "C:\\a.mp4");
            for max in [120.0, 200.0, 300.0] {
                let s = ellipsize_middle(p, &font, path, max);
                assert!(width(&s) <= max, "{s} is wider than {max}");
                assert!(s.starts_with("C:\\") && s.ends_with(".mp4"), "{s}");
                assert!(s.contains("..."), "{s}");
            }
            // Nothing fits but the dots: no panic, the dots alone.
            assert_eq!(ellipsize_middle(p, &font, path, 1.0), "...");
        });
        out.textures_delta.clear();
    }

    /// A row list drawn headless with AccessKit on: its rows' nodes (role, name, place in the
    /// list) and the list's state after the frame.
    struct Drawn {
        rows: Vec<(egui::accesskit::NodeId, String, usize, usize)>,
        list_children: usize,
        st: RowList,
        events: RowEvents,
        took: std::time::Duration,
    }

    fn draw_list(ctx: &egui::Context, n: usize, st: RowList, events: Vec<egui::Event>) -> Drawn {
        let mut st = st;
        let mut ev = RowEvents::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 300.0))),
            events,
            ..Default::default()
        };
        let started = std::time::Instant::now();
        let mut out = ctx.run_ui(input, |ui| {
            let rc = Rect::from_min_size(pos2(10.0, 10.0), egui::vec2(200.0, 102.0));
            ev = row_list(ui, "test-rows", rc, n, 20.0, &mut st, |_, _, i, _, look| {
                look.name = format!("Row {i}");
            });
        });
        let took = started.elapsed();
        out.textures_delta.clear();
        let update = out
            .platform_output
            .accesskit_update
            .expect("AccessKit is on");
        let mut rows = Vec::new();
        let by_id: std::collections::HashMap<_, _> =
            update.nodes.iter().map(|(id, n)| (*id, n)).collect();
        // The list's rows as a screen reader sees them: its children, through the containers
        // that AccessKit leaves out.
        fn options(
            by_id: &std::collections::HashMap<egui::accesskit::NodeId, &egui::accesskit::Node>,
            node: &egui::accesskit::Node,
        ) -> usize {
            node.children()
                .iter()
                .filter_map(|c| by_id.get(c))
                .map(|c| match c.role() {
                    egui::accesskit::Role::ListBoxOption => 1,
                    egui::accesskit::Role::GenericContainer => options(by_id, c),
                    _ => 0,
                })
                .sum()
        }
        let mut list_children = 0;
        for (id, node) in &update.nodes {
            match node.role() {
                egui::accesskit::Role::ListBoxOption => rows.push((
                    *id,
                    node.label().unwrap_or_default().to_owned(),
                    node.position_in_set().unwrap_or(0),
                    node.size_of_set().unwrap_or(0),
                )),
                egui::accesskit::Role::ListBox => list_children = options(&by_id, node),
                _ => {}
            }
        }
        rows.sort_by_key(|r| r.2);
        Drawn {
            rows,
            list_children,
            st,
            events: ev,
            took,
        }
    }

    fn ask(target: egui::accesskit::NodeId, action: egui::accesskit::Action) -> egui::Event {
        egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
            action,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: target,
            data: None,
        })
    }

    /// #111: every row of a short list is in the accessibility tree, in view or not, inside the
    /// list, named, with its place; a screen reader can scroll one into view or select it.
    #[test]
    fn every_row_of_a_short_list_is_reachable() {
        use egui::accesskit::Action;
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let st = RowList::default();
        let d = draw_list(&ctx, 40, st, vec![]);
        let d = draw_list(&ctx, 40, d.st, vec![]);
        assert_eq!(d.rows.len(), 40, "every row has a node");
        assert_eq!(d.list_children, 40, "inside the list");
        for (i, (_, name, pos, size)) in d.rows.iter().enumerate() {
            assert_eq!(
                (name.as_str(), *pos, *size),
                (format!("Row {i}").as_str(), i, 40)
            );
        }
        assert_eq!(d.st.first, 0);
        // Scroll into view: row 35 shows (the view is 5 rows).
        let row35 = d.rows[35].0;
        let d = draw_list(&ctx, 40, d.st, vec![ask(row35, Action::ScrollIntoView)]);
        assert!((d.st.first..d.st.first + 5).contains(&35), "{:?}", d.st);
        // Select (AccessKit's click) a row out of view: selected and brought into view.
        let row2 = d.rows[2].0;
        let d = draw_list(&ctx, 40, d.st, vec![ask(row2, Action::Click)]);
        assert_eq!((d.st.selected, d.events.picked), (Some(2), Some(2)));
        let d = draw_list(&ctx, 40, d.st, vec![]);
        assert!((d.st.first..d.st.first + 5).contains(&2), "{:?}", d.st);
    }

    /// #111: a list of thousands (a playlist, #69) exposes a bounded window of rows around the
    /// view, each with its place among all of them, so a frame costs what a short list's does.
    #[test]
    fn a_long_list_exposes_a_window_of_rows_around_the_view() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let st = RowList {
            first: 5_000,
            ..RowList::default()
        };
        let d = draw_list(&ctx, 10_000, st, vec![]);
        let d = draw_list(&ctx, 10_000, d.st, vec![]);
        assert_eq!(d.rows.len(), A11Y_ROWS);
        let (lo, hi) = (d.rows[0].2, d.rows[A11Y_ROWS - 1].2);
        assert!(lo < 5_000 && hi >= 5_004, "{lo}..={hi}");
        assert!(d.rows.iter().all(|r| r.3 == 10_000));
        // Timing, for the record (cargo test --release -- --nocapture): AccessKit on vs off.
        let mut on = std::time::Duration::ZERO;
        let mut st = d.st;
        for _ in 0..20 {
            let d = draw_list(&ctx, 10_000, st, vec![]);
            on += d.took;
            st = d.st;
        }
        let off_ctx = egui::Context::default();
        let mut off = std::time::Duration::ZERO;
        for _ in 0..20 {
            let started = std::time::Instant::now();
            let mut out = off_ctx.run_ui(egui::RawInput::default(), |ui| {
                let rc = Rect::from_min_size(pos2(10.0, 10.0), egui::vec2(200.0, 102.0));
                row_list(
                    ui,
                    "test-rows",
                    rc,
                    10_000,
                    20.0,
                    &mut st,
                    |_, _, i, _, look| {
                        look.name = format!("Row {i}");
                    },
                );
            });
            off += started.elapsed();
            out.textures_delta.clear();
        }
        println!(
            "10,000 rows, a frame: {:?} with a screen reader on, {:?} off",
            on / 20,
            off / 20
        );
    }

    #[test]
    fn a_control_greyed_with_the_keys_hands_them_on() {
        let ctx = egui::Context::default();
        let a = Rect::from_min_size(pos2(10.0, 10.0), egui::vec2(75.0, 23.0));
        let b = a.translate(egui::vec2(0.0, 30.0));
        let mut first_id = None;
        // Labels from variables: the catalog's extractor takes literal ones as texts to translate.
        let (one, two) = ("OK", "Cancel");
        let frame = |enabled: bool, first_id: &mut Option<egui::Id>| {
            let mut second = None;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let kind = if enabled {
                    ButtonKind::Normal
                } else {
                    ButtonKind::Disabled
                };
                let r = button(ui, a, one, kind, false, None);
                if first_id.is_none() {
                    r.request_focus();
                }
                *first_id = Some(r.id);
                second = Some(button(ui, b, two, ButtonKind::Normal, false, None).id);
            });
            out.textures_delta.clear();
            second.unwrap()
        };
        frame(true, &mut first_id);
        frame(true, &mut first_id);
        assert!(ctx.memory(|m| m.has_focus(first_id.unwrap())));
        // Greyed: the next button has the keys (egui had kept them on the greyed one).
        let second = frame(false, &mut first_id);
        frame(false, &mut first_id);
        assert!(
            ctx.memory(|m| m.has_focus(second)),
            "{:?}",
            ctx.memory(|m| m.focused())
        );
    }

    #[test]
    /// egui's context locks itself for `data`, `memory`, `input` and their `_mut`s: asking it
    /// again inside one (`ctx.viewport_id()` to build a key) deadlocks the app at once. The
    /// key is worked out first; this keeps it so in every file of the app.
    fn no_context_call_inside_a_context_lock() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let opens = [
            ".data(|",
            ".data_mut(|",
            ".memory(|",
            ".memory_mut(|",
            ".input(|",
            ".input_mut(|",
            ".options(|",
            ".options_mut(|",
            ".fonts(|",
            ".fonts_mut(|",
        ];
        // What asks the context again: its own methods, and a Response's that read its memory
        // (`d.insert_temp(key, resp.has_focus())` froze every editable drop-down as it took the
        // focus: 0.2.2's Add URL).
        let inner = [
            "ctx()",
            ".viewport_id()",
            ".cumulative_pass_nr()",
            "_key(ctx",
            "_key(ui",
            "_key(&",
            ".has_focus()",
            ".gained_focus()",
            ".lost_focus()",
            ".request_focus()",
            ".surrender_focus()",
            "ui.data(",
            "ui.data_mut(",
            "ui.memory(",
            "ui.memory_mut(",
            "ui.input(",
            "ui.input_mut(",
            "keyboard_in_use(",
            "widget_id(",
        ];
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let mut src = std::fs::read_to_string(&path).unwrap();
            // Not this test's own patterns.
            if let Some(me) = src.find("fn no_context_call_inside_a_context_lock") {
                src.truncate(me);
            }
            for open in opens {
                let mut from = 0;
                while let Some(at) = src[from..].find(open) {
                    let start = from + at + open.len() - 2; // the "(" of the call
                    let mut depth = 0;
                    let mut end = start;
                    for (k, c) in src[start..].char_indices() {
                        match c {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            _ => {}
                        }
                        if depth == 0 {
                            end = start + k;
                            break;
                        }
                    }
                    let body = &src[start..end];
                    if inner.iter().any(|i| body.contains(i)) {
                        let line = src[..start].lines().count();
                        found.push(format!("{}:{line}", path.display()));
                    }
                    from = end.max(start + 1);
                }
            }
        }
        assert!(
            found.is_empty(),
            "context asked inside its own lock: {found:?}"
        );
    }

    #[test]
    fn no_let_in_an_if_binds_a_condition() {
        // `if a && let x = b && c` binds `b` alone and makes `c` a condition of the `if`: 0.2.3's
        // toolbar bound `on_arrow = dropdown && <pointer past the button>`, and only its arrows
        // ever clicked. Such a value is computed before the `if`.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            for (n, line) in src.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") || code.contains("fn no_let_in_an_if") {
                    continue;
                }
                // `let <name> = <expr> &&…` / `||…` after an `if` or `&&` on the same line.
                for lead in ["if let ", "&& let "] {
                    if let Some(at) = code.find(lead) {
                        let rest = &code[at + lead.len()..];
                        if let Some(eq) = rest.find(" = ")
                            && rest[..eq].chars().all(|c| c.is_alphanumeric() || c == '_')
                            && (rest[eq..].contains(" && ") || rest[eq..].contains(" || "))
                        {
                            found.push(format!("{}:{}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "a let in an if binds only up to its &&: {found:?}"
        );
    }

    #[test]
    fn button_labels_stay_centred_in_taller_buttons() {
        // The measured heights keep their measured baselines.
        assert_eq!(button_baseline(21.0), 13.0);
        assert_eq!(button_baseline(19.0), 13.0);
        assert_eq!(button_baseline(23.0), 14.0);
        // Taller: the label moves down by half the extra height.
        assert_eq!(button_baseline(25.0), 15.0);
        assert_eq!(button_baseline(27.0), 16.0);
        // Text boxes and read-only fields centre the same way.
        assert_eq!(baseline_in(100.0, 21.0), 113.0);
        assert_eq!(baseline_in(100.0, 23.0), 114.0);
        assert_eq!(baseline_in(100.0, 26.0), 116.0);
    }

    #[test]
    fn lines_land_on_whole_pixels_at_any_scaling() {
        let r = Rect::from_min_max(pos2(10.0, 3.0), pos2(11.0, 4.0));
        assert_eq!(snap(1.0, r), r);
        // 125 %: a 1 pt line at x 10 → pixels 12.5..13.75: one whole pixel, on the grid.
        let s = snap(1.25, r);
        assert_eq!(
            ((s.min.x * 1.25).fract(), (s.max.x * 1.25).fract()),
            (0.0, 0.0)
        );
        assert!(s.width() * 1.25 >= 1.0);
    }

    #[test]
    fn a_paste_into_one_line_is_one_clean_line() {
        assert_eq!(single_line("https://x/a.zip\r\n"), "https://x/a.zip");
        assert_eq!(single_line("a\r\nb\tc\u{7}"), "a b c");
    }

    #[test]
    fn a_pressed_button_looks_pressed_in_both_themes() {
        // Issue #7: pressed was #f5f5f5 against hot #f6f6f6, the same to the eye.
        for theme in crate::palette::builtins() {
            for c in [&theme.light, &theme.dark] {
                button_states_differ(c, theme.controls.button);
            }
        }
    }

    fn button_states_differ(c: &crate::palette::Palette, look: crate::palette::ButtonLook) {
        let button_colors = |kind, state, c| button_colors_in(kind, state, c, look);
        {
            for kind in [ButtonKind::Normal, ButtonKind::Default] {
                let rest = button_colors(kind, State::Normal, c);
                let hot = button_colors(kind, State::Hot, c);
                let pressed = button_colors(kind, State::Pressed, c);
                // KDE: the edge takes the accent under the pointer, the face pressed; the label
                // stays.
                if look == crate::palette::ButtonLook::Breeze {
                    assert_ne!(hot.border, rest.border, "hot takes the accent's edge");
                    assert_ne!(pressed.fill, rest.fill, "pressed tints the face");
                    assert_eq!(pressed.bottom, pressed.border, "one edge all round");
                    continue;
                }
                // The era themes: pressed deepens the face; under the pointer the face lightens
                // (Windows 7, Mac OS X) or Windows XP's glow shows inside the edge.
                use crate::palette::ButtonLook as B;
                if matches!(look, B::Luna | B::Aero | B::Aqua) {
                    assert_ne!(pressed.fill, rest.fill, "pressed deepens the face");
                    if look == B::Luna {
                        assert!(c.button_glow.a() > 0, "XP glows under the pointer");
                    } else {
                        assert_ne!(hot.fill, rest.fill, "hot differs from rest");
                    }
                    continue;
                }
                assert_ne!(pressed.text, rest.text, "pressed dims the label");
                assert_eq!(
                    pressed.bottom, pressed.border,
                    "pressed flattens the bottom edge"
                );
                // A flat style (Windows 8) has no darker bottom edge; pressed is then told by
                // its face.
                if c.button_bottom != c.button_border {
                    assert_ne!(
                        rest.bottom, rest.border,
                        "at rest the bottom edge is darker"
                    );
                } else {
                    assert_ne!(
                        pressed.fill, rest.fill,
                        "a flat button's pressed face differs"
                    );
                }
                assert_ne!(hot.fill, rest.fill, "hot differs from rest");
            }
            let disabled = button_colors(ButtonKind::Disabled, State::Disabled, c);
            let enabled = button_colors(ButtonKind::Normal, State::Normal, c);
            let gap = |a: Color32, b: Color32| (a.r() as i32 - b.r() as i32).abs();
            assert!(
                gap(disabled.text, enabled.text) >= 0x50,
                "disabled text is clearly dimmer"
            );
        }
    }

    #[test]
    fn control_states_open_and_tint() {
        assert_eq!(State::Hot.open_if(true), State::Open);
        assert_eq!(State::Normal.open_if(true), State::Open);
        assert_eq!(State::Pressed.open_if(true), State::Pressed);
        assert_eq!(State::Disabled.open_if(true), State::Disabled);
        assert_eq!(State::Hot.open_if(false), State::Hot);
        let (light, dark) = (
            &crate::palette::CLASSIC.light,
            &crate::palette::CLASSIC.dark,
        );
        for c in [light, dark] {
            assert_eq!(subtle_fill(State::Normal, c), None);
            assert_eq!(subtle_fill(State::Disabled, c), None);
            let grey = |s| subtle_fill(s, c).unwrap().r();
            // Pressed (and open) is deeper than hot, on either surface.
            assert!(grey(State::Pressed) < grey(State::Hot));
            assert_eq!(subtle_fill(State::Open, c), subtle_fill(State::Pressed, c));
        }
        assert!(
            highlight_fill(State::Pressed, light).unwrap().r()
                < highlight_fill(State::Hot, light).unwrap().r()
        );
        // Windows 11 boxes: deeper under the pointer, deeper still pressed.
        // Light: darker under the pointer, darker still pressed. Dark: lighter under the
        // pointer, lighter still pressed (Windows 11). The three differ; disabled is empty.
        let fill = |s, c| fluent_box_in(s, c).0.unwrap().r();
        assert!(
            fill(State::Pressed, light) < fill(State::Hot, light)
                && fill(State::Hot, light) < fill(State::Normal, light)
        );
        assert!(
            fill(State::Normal, dark) < fill(State::Hot, dark)
                && fill(State::Hot, dark) < fill(State::Pressed, dark)
        );
        for c in [light, dark] {
            assert_eq!(fluent_box_in(State::Disabled, c).0, None);
        }
    }

    /// 6 px per character: a stand-in for the font.
    fn w(s: &str) -> f32 {
        6.0 * s.chars().count() as f32
    }

    fn limits_lines() -> [SentenceLine; 2] {
        // Options → Connection's Download limits, as IDM places them.
        let line = |lead_baseline: f32, y: f32, gap: f32, baseline: f32| SentenceLine {
            lead: Lead::Right(170.0),
            lead_baseline,
            boxes: pos2(178.0, y),
            gap,
            baseline,
        };
        [
            line(345.0, 330.0, 8.0, 346.0),
            line(366.0, 351.0, 11.0, 364.0),
        ]
    }

    const BOXES: [(&str, egui::Vec2); 2] = [
        ("mb", egui::Vec2::new(36.0, 20.0)),
        ("hours", egui::Vec2::new(36.0, 20.0)),
    ];

    #[test]
    fn pieces_split_at_the_boxes_only() {
        let rows = sentence_pieces("Get {n} of {mb} MB\n{hours} h", &["mb", "hours"]);
        assert_eq!(
            rows,
            vec![
                vec![
                    Piece::Text("Get {n} of"),
                    Piece::Slot("mb"),
                    Piece::Text("MB")
                ],
                vec![Piece::Slot("hours"), Piece::Text("h")],
            ]
        );
    }

    #[test]
    fn a_long_right_aligned_lead_stays_in_the_window() {
        let w = |s: &str| 6.0 * s.chars().count() as f32;
        // 240 px ending at 170 would start at -70: it starts at the window's edge instead and the
        // box moves right to make room.
        let template = format!("{} {{mb}} MB\nevery {{hours}} hours", "x".repeat(40));
        let (texts, rects) = sentence_layout(&template, &limits_lines(), &BOXES, w, 6.0);
        assert_eq!(texts[0].0, 6.0);
        assert_eq!(rects[0].min.x, 6.0 + 240.0 + 3.0);
        // The second line, short, keeps its English place.
        assert_eq!(rects[1].min.x, 178.0);
    }

    #[test]
    fn english_lands_where_the_pieces_were() {
        let (texts, rects) = sentence_layout(
            "Download no more than {mb} MBytes\nevery {hours} hours",
            &limits_lines(),
            &BOXES,
            w,
            f32::MIN,
        );
        let lead = w("Download no more than");
        assert_eq!(
            texts,
            vec![
                (170.0 - lead, 345.0, "Download no more than".to_owned()),
                (222.0, 346.0, "MBytes".to_owned()),
                (170.0 - w("every"), 366.0, "every".to_owned()),
                (225.0, 364.0, "hours".to_owned()),
            ]
        );
        assert_eq!(
            rects[0],
            Rect::from_min_size(pos2(178.0, 330.0), vec2(36.0, 20.0))
        );
        assert_eq!(
            rects[1],
            Rect::from_min_size(pos2(178.0, 351.0), vec2(36.0, 20.0))
        );
    }

    #[test]
    fn a_translation_moves_the_boxes() {
        // Boxes first on each line, the hours on the first line.
        let (texts, rects) =
            sentence_layout("{hours} h: {mb} MB", &limits_lines(), &BOXES, w, f32::MIN);
        assert_eq!(rects[1].min, pos2(178.0, 330.0));
        assert_eq!(texts[0], (222.0, 346.0, "h:".to_owned()));
        assert_eq!(rects[0].min, pos2(222.0 + w("h:") + 8.0, 330.0));
        assert_eq!(texts[1].0, rects[0].right() + 8.0);
    }

    #[test]
    fn a_long_lead_pushes_a_left_aligned_box() {
        let line = [SentenceLine {
            lead: Lead::Left(242.0),
            lead_baseline: 66.0,
            boxes: pos2(297.0, 53.0),
            gap: 5.0,
            baseline: 66.0,
        }];
        let files = [("files", vec2(39.0, 23.0))];
        let (_, rects) = sentence_layout("Download {files} files", &line, &files, w, f32::MIN);
        assert_eq!(rects[0].min.x, 297.0);
        let (_, rects) =
            sentence_layout("Herunterladen {files} Dateien", &line, &files, w, f32::MIN);
        assert_eq!(rects[0].min.x, 242.0 + w("Herunterladen") + 3.0);
    }

    /// A 400 px client area at x 100..500.
    fn mirrored() -> Surface {
        Surface::enter_if(true, 100.0, 500.0)
    }

    #[test]
    fn a_left_to_right_surface_changes_nothing() {
        let _s = Surface::enter_if(false, 100.0, 500.0);
        assert!(!rtl());
        let o = pos2(100.0, 50.0);
        assert_eq!(
            r(o, 10.0, 5.0, 30.0, 8.0),
            Rect::from_min_size(pos2(110.0, 55.0), vec2(30.0, 8.0))
        );
        assert_eq!((mx(110.0), mcol(110.0), fwd()), (110.0, 110.0, 1.0));
        assert_eq!(at(pos2(110.0, 7.0), 13.0), pos2(110.0, 7.0));
    }

    #[test]
    fn rects_are_made_at_their_mirrored_place() {
        let _s = mirrored();
        assert!(rtl());
        let o = pos2(100.0, 50.0);
        // x → L + R − x − w: 10 px from the left edge, 30 wide → ends 10 px from the right.
        let rc = r(o, 10.0, 5.0, 30.0, 8.0);
        assert_eq!(rc, Rect::from_min_size(pos2(460.0, 55.0), vec2(30.0, 8.0)));
        // Made twice is where it was: `rect` turns a screen rect back into its English one.
        assert_eq!(
            rect(rc),
            Rect::from_min_size(pos2(110.0, 55.0), vec2(30.0, 8.0))
        );
        // An edge, a pixel column, a point that starts something 13 wide.
        assert_eq!(mx(110.0), 490.0);
        assert_eq!(mcol(110.0), 489.0);
        assert_eq!(at(pos2(110.0, 7.0), 13.0), pos2(477.0, 7.0));
        // Leading edge and reading direction.
        assert_eq!(lead(rc), rc.right());
        assert_eq!(fwd(), -1.0);
        assert_eq!(start_align(), egui::Align::RIGHT);
        assert_eq!(end_align(), egui::Align::LEFT);
    }

    #[test]
    fn surfaces_nest_and_end() {
        {
            let _outer = mirrored();
            {
                // A popup's own surface, then a time picker's left-to-right inside.
                let _popup = Surface::enter_if(true, 0.0, 200.0);
                assert_eq!(mx(20.0), 180.0);
                let _ltr = Surface::enter_if(false, 0.0, 0.0);
                assert!(!rtl());
                assert_eq!(mx(20.0), 20.0);
            }
            assert_eq!(mx(110.0), 490.0);
        }
        assert!(!rtl());
        assert_eq!(mx(110.0), 110.0);
    }

    #[test]
    fn a_sentence_runs_from_the_right() {
        // Download limits as English places them, then mirrored: the lead text starts at the
        // right (its pen is mirrored by `text`), the boxes follow to its left.
        let (texts, rects) = sentence_layout(
            "Download no more than {mb} MBytes\nevery {hours} hours",
            &limits_lines(),
            &BOXES,
            w,
            f32::MIN,
        );
        let _s = Surface::enter_if(true, 0.0, 400.0);
        let shown: Vec<Rect> = rects.into_iter().map(rect).collect();
        assert_eq!(
            shown[0],
            Rect::from_min_size(pos2(400.0 - 178.0 - 36.0, 330.0), vec2(36.0, 20.0))
        );
        // The lead's English box ends at 170: on screen it starts there from the right, right of
        // the box.
        let lead = &texts[0];
        let lead_box = rect(Rect::from_min_size(
            pos2(lead.0, 0.0),
            vec2(w(&lead.2), 1.0),
        ));
        assert_eq!(lead_box.left(), 400.0 - 170.0);
        assert!(lead_box.left() > shown[0].right());
    }

    #[test]
    fn extra_lines_join_the_last_and_a_missing_box_still_shows() {
        let (texts, rects) = sentence_layout("a {mb}\nb\nc", &limits_lines(), &BOXES, w, f32::MIN);
        assert_eq!(texts.last().unwrap().2, "c");
        assert_eq!(texts.last().unwrap().1, 364.0);
        assert!(rects[1].min.x > 170.0);
    }
}
