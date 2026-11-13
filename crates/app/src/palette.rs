//! A theme: every colour, size and icon style the painters draw with (#45). The painters read
//! the active theme through `crate::theme::colors()` and `crate::theme::metrics()`; nothing in
//! the painting code holds a colour or a control's size of its own.
//!
//! - [`Palette`]: one variant's colours (a theme has a light and a dark one; the mode picks
//!   which), by role. Each role has a stable key (`"button.hot"`), which theme files use.
//! - [`Metrics`]: the sizes a control painter owns (corner radii, border and focus widths, font
//!   sizes, row heights). Where a window puts its controls stays the window's layout.
//! - [`IconStyle`]: how the toolbar's and Options' outline icons are inked.
//!
//! [`CLASSIC`] is MDM's own look (IDM's classic layout, Windows 11 controls, accent #0078D4),
//! written out with the exact values the painters had before themes: the default, unchanged.

use std::borrow::Cow;

use egui::Color32;

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}
const fn grey(v: u8) -> Color32 {
    rgb(v, v, v)
}
const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, a)
}

/// The progress window's segment bar: 13 rows top to bottom, each row's colour for the parts
/// not downloaded yet, downloaded, and a connection's start marker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segments {
    pub missing: [Color32; 13],
    pub done: [Color32; 13],
    pub start: [Color32; 13],
}

/// The classic bar, measured from IDM's (spec "Segment bar"), as (missing, done, start) rows.
const SEG_ROWS: [(u32, u32, u32); 13] = [
    (0xfafafc, 0xbfc6f3, 0xf67566),
    (0xf7f8fa, 0xa4aae7, 0xf46a5e),
    (0xf5f6fa, 0x898edd, 0xf25e56),
    (0xf2f4f9, 0x7176d3, 0xef514c),
    (0xecf0f6, 0x686bcf, 0xea4240),
    (0xdbe0ee, 0x1a3cb7, 0xbb0205),
    (0xdce1f0, 0x0f43b8, 0xbe0205),
    (0xdee3f1, 0x1155bf, 0xc60b09),
    (0xdfe4f1, 0x1267c7, 0xd21913),
    (0xe1e5f1, 0x117acd, 0xde281b),
    (0xe2e7f3, 0x128ed6, 0xeb3724),
    (0xe4e8f4, 0x1aafe3, 0xf1462c),
    (0xe6e9f5, 0x2dd3f1, 0xf64f30),
];

const fn hex(v: u32) -> Color32 {
    rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// The classic segment bar; in dark mode its "not here yet" rows are the same gradient dimmed
/// (they were a bright white strip in the dark window).
const fn classic_segments(dark: bool) -> Segments {
    const fn dim(x: u8) -> u8 {
        (x as f32 * 0.22 + 22.0) as u8
    }
    let mut s = Segments {
        missing: [Color32::BLACK; 13],
        done: [Color32::BLACK; 13],
        start: [Color32::BLACK; 13],
    };
    let mut i = 0;
    while i < 13 {
        let (m, d, st) = SEG_ROWS[i];
        let m = hex(m);
        s.missing[i] = if dark {
            rgb(dim(m.r()), dim(m.g()), dim(m.b()))
        } else {
            m
        };
        s.done[i] = hex(d);
        s.start[i] = hex(st);
        i += 1;
    }
    s
}

/// Declares [`Palette`]: one `Color32` field per role, its theme-file key and what it colours,
/// with the classic dark and light values.
macro_rules! palette {
    ($($field:ident: $key:literal, $doc:literal = $dark:expr, $light:expr;)*) => {
        /// One variant's colours (light or dark), by role, plus the few switches that differ
        /// between IDM's light and dark looks.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Palette {
            /// A dark surface: light-on-dark glyph rendering and the dark window caption.
            pub dark: bool,
            /// A focused text box's accent line: straight (IDM's dark look) or curving into the
            /// box's rounded corners (Windows 11's).
            pub focus_line_straight: bool,
            /// Access letters are always underlined (IDM's dark look), not only once the keyboard
            /// is in use (Windows' rule).
            pub access_keys_always: bool,
            /// The category tree's expanders: chevrons (dark look) or classic +/- boxes.
            pub tree_chevrons: bool,
            /// The progress window's segment bar.
            pub segments: Segments,
            $(#[doc = $doc] pub $field: Color32,)*
        }

        /// Every role: its key in a theme file and what it colours, in declaration order.
        // The theme picker and the themes help page (#45, next) list them.
        #[allow(dead_code)]
        pub const ROLES: &[(&str, &str)] = &[$(($key, $doc),)*];

        impl Palette {
            /// The colour of the role with this key.
            #[allow(dead_code)]
            pub fn role(&self, key: &str) -> Option<Color32> {
                match key {
                    $($key => Some(self.$field),)*
                    _ => None,
                }
            }

            /// Sets the role with this key; false when there is no such role.
            pub fn set_role(&mut self, key: &str, c: Color32) -> bool {
                match key {
                    $($key => { self.$field = c; true })*
                    _ => false,
                }
            }

            /// The classic look's dark or light variant.
            const fn classic(dark: bool) -> Self {
                Self {
                    dark,
                    focus_line_straight: dark,
                    access_keys_always: dark,
                    tree_chevrons: dark,
                    segments: classic_segments(dark),
                    $($field: if dark { $dark } else { $light },)*
                }
            }
        }
    };
}

palette! {
    // ---- window and text ----
    window: "window", "Windows' and dialogs' background" = grey(0x39), grey(0xf0);
    text: "text", "Text and glyphs" = grey(0xff), grey(0x00);
    text_disabled: "text.disabled", "Disabled text, the one grey of every greyed control and label" = grey(0x8a), grey(0x8a);
    // Dark: #ababab, 4.5:1 on the dark tab page (#a0a0a0 was 4.0:1 there, 4.4:1 on the window).
    muted: "text.muted", "Secondary text: notes, sizes, versions" = grey(0xab), grey(0x6d);
    link: "link", "Text that opens something" = rgb(0x99, 0xc8, 0xff), rgb(0x00, 0x66, 0xcc);
    // Dark: #ff9999, 5:1 on the dark tab page (#ff8080 was 4.3:1 there).
    error: "error", "Error text" = rgb(0xff, 0x99, 0x99), rgb(0xc4, 0x2b, 0x1c);
    // Dark: the error colour (light mode's #c42b1c was 2:1 on the dark dialog).
    error_strong: "error.strong", "A failed check's message in Options (connection, browser and helper tests)" = rgb(0xff, 0x99, 0x99), rgb(0xc4, 0x2b, 0x1c);
    warning: "warning", "Warning text" = rgb(0x9d, 0x5d, 0x00), rgb(0x9d, 0x5d, 0x00);
    success: "success", "Success text (a matching checksum)" = rgb(0x6c, 0xcb, 0x5f), rgb(0x0f, 0x7b, 0x0f);
    reach_near: "reach.near", "A download's server near by its round trip (under 60 ms), in Properties" = rgb(0x6c, 0xcb, 0x5f), rgb(0x10, 0x7c, 0x10);
    reach_medium: "reach.medium", "A server at a medium round trip (60 to 150 ms)" = rgb(0xf7, 0xb0, 0x40), rgb(0x9d, 0x5d, 0x00);
    reach_far: "reach.far", "A server far by its round trip (over 150 ms)" = rgb(0xff, 0x99, 0xa4), rgb(0xc4, 0x2b, 0x1c);
    flag_edge: "flag.edge", "The thin edge around a country's flag" = rgba(0x00, 0x00, 0x00, 40), rgba(0x00, 0x00, 0x00, 40);
    accent: "accent", "The accent: focus lines, open and default controls, filters, links' focus" = rgb(0x60, 0xcd, 0xff), rgb(0x00, 0x78, 0xd4);
    focus_ring: "focus.ring", "The dotted keyboard focus rectangle" = grey(0xcc), grey(0x0d);
    // ---- selection ----
    selection: "selection", "A selection's fill: rows, a chosen day, selected text" = rgb(0x00, 0x78, 0xd4), rgb(0x00, 0x78, 0xd4);
    selection_text: "selection.text", "Text on the selection" = grey(0xff), grey(0xff);
    selection_muted: "selection.muted", "Secondary text on the selection" = grey(0xe0), grey(0xe0);
    selection_inactive: "selection.inactive", "A selected row in a list without the focus" = grey(0x4a), grey(0xd9);
    selection_focus: "selection.focus", "The focused row's dotted outline on a selected row" = rgb(0xff, 0x99, 0x33), rgb(0xff, 0x99, 0x33);
    // ---- group boxes, separators, tabs ----
    group_line: "group.line", "A group box's frame" = grey(0x50), grey(0xdc);
    etch_shadow: "etch.shadow", "An etched separator's shadow line" = grey(0x22), grey(0xa0);
    etch_light: "etch.light", "An etched separator's highlight line" = grey(0x50), grey(0xff);
    tab_page: "tab.page", "A tab page's body" = grey(0x40), grey(0xf9);
    tab: "tab", "An unselected tab, and the page's shadow" = grey(0x33), grey(0xf3);
    tab_hot: "tab.hot", "A tab under the pointer" = grey(0x46), grey(0xfd);
    chevron: "chevron", "A drop-down's or spin box's arrow" = grey(0xc8), grey(0x6f);
    chevron_disabled: "chevron.disabled", "A greyed arrow" = grey(0x6a), grey(0xbe);
    // ---- parts without a face of their own ----
    subtle_hot: "subtle.hot", "A part with no face of its own (an arrow, a column header) under the pointer" = grey(0x45), grey(0xf3);
    subtle_pressed: "subtle.pressed", "Such a part pressed or open" = grey(0x26), grey(0xe8);
    highlight_hot: "highlight.hot", "A calendar's day or arrow under the pointer, a drop target" = rgb(0x2d, 0x4a, 0x60), rgb(0xe5, 0xf1, 0xfb);
    highlight_pressed: "highlight.pressed", "A calendar's day or arrow pressed" = rgb(0x23, 0x3d, 0x52), rgb(0xcc, 0xe4, 0xf7);
    // ---- text boxes ----
    field: "field", "A text box's or a list's inside" = grey(0x2b), grey(0xff);
    field_border: "field.border", "A text box's border" = grey(0x4b), grey(0xe5);
    field_bottom: "field.bottom", "A text box's bottom line while it can be typed in" = grey(0x9a), grey(0x86);
    field_disabled: "field.disabled", "A greyed text box's inside" = grey(0x33), grey(0xf9);
    field_disabled_border: "field.disabled.border", "A greyed text box's border" = grey(0x44), grey(0xe9);
    field_readonly: "field.readonly", "A read-only text box's inside" = grey(0x30), grey(0xf3);
    field_readonly_border: "field.readonly.border", "A read-only text box's border" = grey(0x4b), grey(0xe2);
    combo_chevron: "combo.chevron", "A drop-down's chevron" = grey(0x6f), grey(0x6f);
    // Dark: the dark parts' tints (light mode's greys showed over the dark box).
    combo_arrow_hot: "combo.arrow.hot", "An editable drop-down's arrow zone under the pointer" = grey(0x45), grey(0xf3);
    combo_arrow_pressed: "combo.arrow.pressed", "An editable drop-down's arrow zone pressed or open" = grey(0x26), grey(0xe8);
    // ---- buttons ----
    button: "button", "A button's face" = grey(0x44), grey(0xfd);
    button_hot: "button.hot", "A button's face under the pointer" = grey(0x4b), grey(0xf6);
    button_pressed: "button.pressed", "A button's face pressed" = grey(0x3c), grey(0xf5);
    button_border: "button.border", "A button's border" = grey(0x55), grey(0xd0);
    button_bottom: "button.bottom", "A button's bottom edge" = grey(0x4b), grey(0xba);
    button_text: "button.text", "A button's text" = grey(0xff), grey(0x00);
    button_text_pressed: "button.text.pressed", "A button's text pressed" = grey(0xc8), grey(0x5d);
    button_default_border: "button.default.border", "The default button's border" = rgb(0x60, 0xcd, 0xff), rgb(0x00, 0x78, 0xd4);
    button_default_bottom: "button.default.bottom", "The default button's bottom edge" = rgb(0x4f, 0xb5, 0xe4), rgb(0x00, 0x6b, 0xbe);
    button_disabled: "button.disabled", "A greyed button's face" = grey(0x3d), grey(0xf9);
    button_disabled_border: "button.disabled.border", "A greyed button's border" = grey(0x46), grey(0xe9);
    button_accent: "button.accent", "An accent button's face" = rgb(0x60, 0xcd, 0xff), rgb(0x00, 0x78, 0xd4);
    button_accent_hot: "button.accent.hot", "An accent button's face under the pointer" = rgb(0x5a, 0xb9, 0xe5), rgb(0x1a, 0x86, 0xd9);
    button_accent_pressed: "button.accent.pressed", "An accent button's face pressed" = rgb(0x52, 0xa6, 0xd0), rgb(0x00, 0x6c, 0xbe);
    button_accent_bottom: "button.accent.bottom", "An accent button's bottom edge" = rgb(0x4f, 0xb5, 0xe4), rgb(0x00, 0x5a, 0x9e);
    button_accent_text: "button.accent.text", "An accent button's text" = grey(0x00), grey(0xff);
    button_accent_text_pressed: "button.accent.text.pressed", "An accent button's text pressed" = grey(0x1a), rgb(0xe6, 0xf0, 0xfa);
    button_glow: "button.glow", "The glow inside a button under the pointer, where the style draws one (Luna; transparent: none)" = Color32::TRANSPARENT, Color32::TRANSPARENT;
    gloss: "gloss", "The shine glossy parts blend toward at their top: buttons, boxes, bars and title bars (Luna, Aero, Aqua)" = grey(0xff), grey(0xff);
    // ---- check boxes and radios ----
    check: "check", "An unchecked box's or radio's inside" = grey(0x2b), grey(0xf3);
    check_hot: "check.hot", "Its inside under the pointer" = grey(0x3a), grey(0xea);
    check_pressed: "check.pressed", "Its inside pressed" = grey(0x47), grey(0xdd);
    check_border: "check.border", "An unchecked box's or radio's border" = grey(0x9b), grey(0x62);
    check_border_hot: "check.border.hot", "Its border under the pointer" = grey(0xa0), grey(0x62);
    check_border_pressed: "check.border.pressed", "Its border pressed" = grey(0x70), grey(0x9a);
    check_border_disabled: "check.border.disabled", "A greyed unchecked box's border" = grey(0x5a), grey(0xc3);
    check_on: "check.on", "A checked box's or radio's accent" = rgb(0x60, 0xcd, 0xff), rgb(0x00, 0x78, 0xd4);
    check_on_hot: "check.on.hot", "The checked accent under the pointer" = rgb(0x5a, 0xbc, 0xe8), rgb(0x1a, 0x86, 0xd9);
    check_on_pressed: "check.on.pressed", "The checked accent pressed" = rgb(0x52, 0xa6, 0xcc), rgb(0x33, 0x92, 0xdc);
    check_on_disabled: "check.on.disabled", "A greyed checked box's fill" = grey(0x5a), grey(0xc3);
    check_mark: "check.mark", "The tick or dot on the checked accent" = grey(0x00), grey(0xff);
    // ---- progress and meters ----
    progress_track: "progress.track", "A progress bar's track" = grey(0x9a), grey(0x86);
    progress_bar: "progress.bar", "A progress bar's fill" = rgb(0x60, 0xcd, 0xff), rgb(0x00, 0x78, 0xd4);
    meter_track: "meter.track", "A meter's track (a download's part done)" = grey(0x2b), grey(0xe6);
    meter_edge: "meter.edge", "A meter's edge" = grey(0x4b), grey(0xbc);
    meter_fill: "meter.fill", "A meter's fill" = rgb(0x0f, 0x7b, 0x0f), rgb(0x0f, 0x7b, 0x0f);
    // ---- lists and tables ----
    list: "list", "The download list's background" = grey(0x39), grey(0xff);
    list_frame: "list.frame", "A list's, a table's or a box's outline" = grey(0x5a), rgb(0x82, 0x87, 0x90);
    grid: "grid", "Grid lines between a list's rows in dialogs" = grey(0x40), grey(0xf0);
    list_grid: "list.grid", "The download list's grid lines" = grey(0x55), grey(0xf0);
    header: "header", "Every list's and table's column headers" = grey(0x19), grey(0xff);
    header_line: "header.line", "A header's bottom line in dialogs' lists" = grey(0x4e), grey(0xe5);
    header_sep: "header.sep", "The dividers between column headers" = grey(0x63), grey(0xe5);
    table_line: "table.line", "The dividers in the link grabber's and Add torrent's tables" = grey(0x4a), grey(0xe5);
    sorted: "header.sorted", "The sorted column's header" = grey(0x26), grey(0xf0);
    sort_arrow: "header.sort-arrow", "The sort chevron" = grey(0xc8), grey(0x60);
    row_pressed: "row.pressed", "A list row pressed" = grey(0x4f), grey(0xe8);
    chip_web: "chip.web", "The download list's Web chip" = rgb(0x2e, 0x9d, 0x52), rgb(0x2e, 0x9d, 0x52);
    chip_torrent: "chip.torrent", "The download list's Torrent chip" = rgb(0x7e, 0x4f, 0xd0), rgb(0x7e, 0x4f, 0xd0);
    chip_text: "chip.text", "Text on a chip" = grey(0xff), grey(0xff);
    drag_ghost: "list.drag-ghost", "A column header being dragged" = rgba(0x60, 0x60, 0x60, 160), rgba(0x60, 0x60, 0x60, 160);
    marquee: "list.marquee", "The rubber band that selects rows" = rgba(0x33, 0x99, 0xff, 0x40), rgba(0x33, 0x99, 0xff, 0x40);
    marquee_border: "list.marquee.border", "The rubber band's edge" = rgb(0x33, 0x99, 0xff), rgb(0x33, 0x99, 0xff);
    // ---- scroll bars ----
    scroll_track: "scroll.track", "A scroll bar's track" = grey(0x17), grey(0xf0);
    scroll_corner: "scroll.corner", "Where two scroll bars meet" = grey(0x38), grey(0xf0);
    scroll_thumb: "scroll.thumb", "A scroll bar's thumb and arrows" = grey(0x95), grey(0x85);
    scroll_thumb_hot: "scroll.thumb.hot", "The thumb or an arrow under the pointer" = grey(0xb0), grey(0x70);
    scroll_thumb_pressed: "scroll.thumb.pressed", "The thumb or an arrow pressed or dragged" = grey(0xc8), grey(0x5c);
    // ---- menu bar and menus ----
    menubar: "menubar", "The menu bar" = grey(0x20), grey(0xff);
    menubar_bottom: "menubar.bottom", "The menu bar's last row" = grey(0x20), grey(0xf2);
    menubar_line: "menubar.line", "The line under the menu bar" = grey(0x20), grey(0xf0);
    menubar_hot: "menubar.hot", "A menu title under the pointer" = grey(0x50), grey(0xf5);
    menubar_open: "menubar.open", "The open menu's title" = grey(0x50), grey(0xf9);
    menu: "menu", "A menu's background" = grey(0x2b), grey(0xf9);
    menu_item: "menu.item", "A menu item" = grey(0x2c), grey(0xf9);
    menu_hot: "menu.hot", "A menu item under the pointer" = grey(0x35), grey(0xf0);
    menu_border: "menu.border", "A menu's and a popup's frame" = grey(0x3e), grey(0xe5);
    menu_sep: "menu.sep", "A menu's separators" = grey(0x3e), grey(0xd7);
    menu_text: "menu.text", "A menu's text" = grey(0xff), grey(0x00);
    menu_glyph: "menu.glyph", "Bullets, ticks and submenu arrows" = grey(0xff), grey(0x1a);
    menu_text_disabled: "menu.text.disabled", "A greyed menu item" = grey(0x6d), grey(0x9f);
    // ---- toolbar ----
    toolbar_line: "toolbar.line", "The toolbar's top line" = grey(0x4a), grey(0xa0);
    toolbar_line_light: "toolbar.line.light", "The highlight under it" = grey(0x39), grey(0xff);
    toolbar_hot: "toolbar.hot", "A toolbar button under the pointer" = rgb(0x33, 0x41, 0x4d), rgb(0xe5, 0xf3, 0xff);
    toolbar_hot_faint: "toolbar.hot.faint", "A split button's other part while one is under the pointer" = rgb(0x37, 0x3b, 0x3f), rgb(0xf3, 0xf9, 0xff);
    toolbar_pressed: "toolbar.pressed", "A toolbar button pressed" = rgb(0x2a, 0x36, 0x40), rgb(0xcc, 0xe8, 0xff);
    toolbar_hot_edge: "toolbar.hot.edge", "A hot toolbar button's edge" = rgb(0x4d, 0x5f, 0x6e), rgb(0xcc, 0xe8, 0xff);
    toolbar_hot_border: "toolbar.hot.border", "A hot toolbar button's border (transparent: none)" = Color32::TRANSPARENT, rgb(0xcc, 0xe8, 0xff);
    toolbar_split_shadow: "toolbar.split.shadow", "A split button's divider at rest: its shadow" = grey(0x26), grey(0xc6);
    toolbar_split_light: "toolbar.split.light", "The divider's highlight" = grey(0x4d), grey(0xff);
    toolbar_text_disabled: "toolbar.text.disabled", "A greyed toolbar label" = grey(0xa0), grey(0xa0);
    toolbar_arrow: "toolbar.arrow", "A drop-down button's arrow" = grey(0xd0), grey(0x28);
    icon_disabled: "icon.disabled", "A greyed icon's ink (its start)" = grey(0x9f), grey(0x9f);
    icon_disabled_end: "icon.disabled.end", "A greyed icon's ink (its end)" = grey(0x9a), grey(0x9a);
    // ---- categories tree ----
    tree: "tree", "The categories tree's background" = grey(0x20), grey(0xff);
    tree_lines: "tree.lines", "The tree's dotted connectors" = grey(0x6d), grey(0x6d);
    tree_expander: "tree.expander", "The tree's chevrons, where the variant draws them" = grey(0xff), grey(0x1a);
    tree_frame: "tree.frame", "The categories pane's frame" = grey(0xdc), grey(0xdc);
    // ---- progress window ----
    status_value: "progress.status", "The progress window's Status value" = grey(0xff), rgb(0x00, 0x00, 0xff);
    progress_label_disabled: "progress.label.disabled", "A greyed label in the progress window" = grey(0x80), grey(0x80);
    segment_border: "segments.border", "The segment bar's frame" = rgb(0x65, 0x6b, 0x7b), rgb(0x65, 0x6b, 0x7b);
    segment_corner: "segments.corner", "The segment bar's corner pixels" = rgb(0xa2, 0xa5, 0xaf), rgb(0xa2, 0xa5, 0xaf);
    piece: "pieces.have", "A torrent piece that is here" = rgb(0x2e, 0x9d, 0x52), rgb(0x2e, 0x9d, 0x52);
    piece_partial: "pieces.partial", "A square standing for pieces partly here" = rgb(0x3a, 0x6e, 0x4b), rgb(0x9c, 0xd2, 0xae);
    piece_missing: "pieces.missing", "A piece not here yet" = grey(0x45), grey(0xd6);
    // ---- title bars we draw ----
    caption: "caption", "A title bar MDM draws itself" = grey(0x20), grey(0xff);
    caption_text: "caption.text", "Its title and glyphs" = grey(0xff), grey(0x00);
    caption_text_inactive: "caption.text.inactive", "Its title and glyphs while the window isn't the active one, and a greyed button's" = grey(0x8a), grey(0x8a);
    caption_hot: "caption.hot", "A caption button under the pointer (a tint over the bar)" = rgba(0xff, 0xff, 0xff, 0x0f), rgba(0x00, 0x00, 0x00, 0x09);
    caption_pressed: "caption.pressed", "A caption button pressed (a tint over the bar)" = rgba(0xff, 0xff, 0xff, 0x0a), rgba(0x00, 0x00, 0x00, 0x06);
    caption_button: "caption.button", "A caption button at rest, where the style shows one (a tint over the bar; transparent: none)" = Color32::TRANSPARENT, Color32::TRANSPARENT;
    caption_close: "caption.close", "Close under the pointer" = rgb(0xc4, 0x2b, 0x1c), rgb(0xc4, 0x2b, 0x1c);
    caption_close_text: "caption.close.text", "Close's glyph under the pointer" = grey(0xff), grey(0xff);
    caption_inactive: "caption.inactive", "A title bar MDM draws, while its window isn't the active one (transparent: as when active)" = Color32::TRANSPARENT, Color32::TRANSPARENT;
    // ---- other ----
    drop_edge: "drop-target.edge", "The drop target's edge at rest" = grey(0x8d), grey(0x8d);
    // ---- egui's own widgets (tooltips, multi-line boxes' scroll bars) ----
    egui_panel: "egui.panel", "egui's panel background" = grey(0x2b), grey(0xf0);
    egui_window: "egui.window", "Tooltips' background" = grey(0x2b), grey(0xf9);
    egui_window_border: "egui.window.border", "Tooltips' border" = grey(0x1a), grey(0xe5);
    egui_text: "egui.text", "Tooltips' text" = grey(0xf0), grey(0x00);
    egui_selection_text: "egui.selection.text", "Selected text's ink in egui's boxes" = grey(0xf0), grey(0xff);
    egui_extreme: "egui.extreme", "egui's darkest (or lightest) background" = grey(0x1a), grey(0xff);
    // Light: egui's own light values (the classic look set only the dark ones).
    egui_faint: "egui.faint", "egui's faint background" = grey(0x3e), Color32::from_rgba_premultiplied(5, 5, 5, 0);
    egui_widget: "egui.widget", "egui's widgets at rest" = grey(0x3a), grey(0xe6);
    egui_widget_border: "egui.widget.border", "Their border (transparent: none)" = grey(0x6a), Color32::TRANSPARENT;
}

/// Declares [`Metrics`]: one `f32` field per size, its theme-file key and what it sizes, with
/// the classic value.
macro_rules! metrics {
    ($($field:ident: $key:literal, $doc:literal = $value:expr;)*) => {
        /// The sizes the control painters own, in points.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Metrics {
            $(#[doc = $doc] pub $field: f32,)*
        }

        /// Every size: its key in a theme file and what it sizes.
        #[allow(dead_code)]
        pub const SIZES: &[(&str, &str)] = &[$(($key, $doc),)*];

        impl Metrics {
            pub const CLASSIC: Self = Self { $($field: $value,)* };

            #[allow(dead_code)]
            pub fn get(&self, key: &str) -> Option<f32> {
                match key {
                    $($key => Some(self.$field),)*
                    _ => None,
                }
            }

            /// Sets the size with this key; false when there is no such size.
            pub fn set(&mut self, key: &str, v: f32) -> bool {
                match key {
                    $($key => { self.$field = v; true })*
                    _ => false,
                }
            }
        }
    };
}

metrics! {
    control_radius: "radius", "Corner radius of buttons, text boxes and drop-downs" = 3.0;
    button_radius: "radius.button", "Corner radius of a glossy push button (Luna, Aero; Aqua's are round at the ends)" = 3.0;
    part_radius: "radius.part", "Corner radius of a tinted part (an arrow zone, a spin button)" = 2.0;
    check_radius: "radius.check", "Corner radius of a check box" = 2.0;
    flyout_radius: "radius.flyout", "Corner radius of a popup in a window (a calendar, a filter box)" = 6.0;
    menu_item_radius: "radius.menu-item", "Corner radius of a menu item under the pointer" = 4.0;
    widget_radius: "radius.egui", "Corner radius of egui's own widgets" = 2.0;
    popup_radius: "radius.popup", "Corner radius of tooltips" = 4.0;
    border: "border", "Width of a control's border" = 1.0;
    focus_line: "focus-line", "Height of a focused text box's accent line" = 2.0;
    chevron_width: "chevron.width", "Stroke width of a drop-down's chevron" = 1.2;
    font_dialog: "font.dialog", "Dialog text (Tahoma in IDM's dialogs)" = 11.0;
    font_dialog_bold: "font.dialog.bold", "Options pages' titles" = 13.0;
    font_cjk: "font.cjk", "The smallest text in a Chinese, Japanese or Korean interface (their glyphs are denser than Latin's)" = 12.0;
    font_title: "font.title", "A window's headline (Tahoma Bold): the browser extension's, the helpers'" = 16.0;
    font_code: "font.code", "A sign-in code to type at a service's page" = 22.0;
    font_ui: "font.ui", "Menus and the main window's text (Segoe UI)" = 12.0;
    font_ui_bold: "font.ui.bold", "Bold headings in the main window" = 13.0;
    font_small: "font.small", "egui's small text" = 11.0;
    font_body: "font.body", "egui's body text: tooltips, multi-line boxes" = 12.0;
    font_heading: "font.heading", "egui's headings" = 16.0;
    list_row: "list.row", "Height of a row in the download list" = 17.0;
    list_header: "list.header", "Height of the download list's column headers" = 24.0;
    tooltip_width: "tooltip.width", "Where a tooltip's text wraps" = 320.0;
    tooltip_margin_x: "tooltip.margin-x", "A tooltip's left and right padding" = 9.0;
    tooltip_margin_top: "tooltip.margin-top", "A tooltip's top padding" = 6.0;
    tooltip_margin_bottom: "tooltip.margin-bottom", "A tooltip's bottom padding" = 7.0;
}

/// How the outline icons (toolbar, Options pages, helpers) are inked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum IconStyle {
    /// Each icon's own two-colour gradient (the classic toolbar).
    #[default]
    Color,
    /// The text colour.
    Mono,
    /// The accent.
    Accent,
}

impl IconStyle {
    pub fn from_key(s: &str) -> Option<Self> {
        match s {
            "color" | "colour" => Some(IconStyle::Color),
            "mono" => Some(IconStyle::Mono),
            "accent" => Some(IconStyle::Accent),
            _ => None,
        }
    }
}

/// How the shared painters draw each kind of control (#45): the classic look's are Windows 11's;
/// the style themes pick their era's. Colours and sizes still come from the palette and metrics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controls {
    /// A focused text box: an accent line along its bottom (Windows 11), or its whole border in
    /// the accent (Windows 8, XP).
    pub field_focus: FieldFocus,
    /// A check box: Windows 11's (the accent filled, a light tick), or a box that keeps its
    /// inside and takes a tick in the accent (Windows 8).
    pub check: CheckLook,
    /// A progress bar: Windows 11's thin line, or a bordered bar filled (Windows 8).
    pub progress: ProgressLook,
    /// A scroll bar: Windows 11's (thin until hovered, arrows then), or arrows always shown and a
    /// square thumb the bar's width (Windows 8, the classic bars).
    pub scroll: ScrollLook,
    /// The title bar: the system's, or drawn by MDM in the theme's style (Windows only).
    pub caption: CaptionLook,
    /// The keyboard focus around a control: Windows' dotted rectangle, GNOME's rounded ring or
    /// KDE's frame.
    pub focus: FocusLook,
    /// A push button: Windows 11's, or KDE's (the edge in the accent under the pointer, the face
    /// tinted with it pressed).
    pub button: ButtonLook,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldFocus {
    Line,
    Border,
    /// GNOME: a 2-point ring inside the box, in the accent half seen through.
    Ring,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckLook {
    Fluent,
    Square,
    /// GNOME: a 2-point border at rest, the accent filled with a light tick when checked.
    Adwaita,
    /// KDE: the edge in the accent under the pointer; checked, the accent's edge, its inside
    /// tinted with it, a tick in the text's colour.
    Breeze,
    /// Windows XP: a box shaded from its top, a glow inside under the pointer, a thick tick in
    /// the checked colour; round radios with a shaded dot.
    Luna,
    /// Windows 7: a quiet box with a shaded square inside, its edge in the accent under the
    /// pointer, the tick in the check mark's colour.
    Aero,
    /// Mac OS X: a glossy box with round corners, filled with the accent's gel when checked.
    Aqua,
}

/// How a push button shows the pointer and a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonLook {
    /// Windows 11: its face lighter under the pointer, its bottom edge flattened pressed.
    Fluent,
    /// KDE: its edge in the accent under the pointer and pressed, its face tinted pressed.
    Breeze,
    /// Windows XP: a face shaded from white at its top, a dark edge, a glow inside under the
    /// pointer (`button.glow`) and on the default button (its border's colour).
    Luna,
    /// Windows 7: a glossy face, its top half lighter than the bottom, the edge in the accent
    /// under the pointer and pressed.
    Aero,
    /// Mac OS X: a gel face, round at the ends, the default button in the accent's gel.
    Aqua,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollLook {
    Overlay,
    Classic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressLook {
    Line,
    Bar,
    /// Windows XP: blocks in the bar's colour, shaded across, on a bordered track.
    Blocks,
    /// Windows 7 and Mac OS X: a glossy fill on a shaded, bordered track.
    Gloss,
}

/// A drawn title bar's look: its buttons and where the title sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionLook {
    /// The system draws it (Windows' own frame, the window manager's elsewhere).
    System,
    /// Windows 8: a flat bar in the theme's caption colour, the title centred, flat buttons.
    Metro,
    /// GNOME's header bar: the title centred in bold, no icon, round buttons tinted at rest.
    Adwaita,
    /// KDE's title bar: the icon, the title centred, round buttons that fill with the title's
    /// colour under the pointer (Close red), their glyph then the bar's.
    Breeze,
    /// Windows XP: a shaded bar, the title in bold with a shadow, square buttons with a light
    /// edge (Close always red).
    Luna,
    /// Windows 7: a glassy bar, the title on a soft glow, glossy buttons hung from the bar's
    /// top (Close wider and red).
    Aero,
    /// Mac OS X: a shaded grey bar, the title centred, small round gel buttons whose glyphs
    /// show under the pointer.
    Aqua,
}

/// How the keyboard focus is drawn around a control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusLook {
    /// Windows: a dotted rectangle, every other pixel, in the focus ink.
    Dotted,
    /// GNOME: a 2-point ring in the focus ink, its corners the controls' radius.
    Ring,
    /// KDE: a 1-point frame in the focus ink, its corners the controls' radius.
    Frame,
}

impl Controls {
    pub const CLASSIC: Self = Self {
        field_focus: FieldFocus::Line,
        check: CheckLook::Fluent,
        progress: ProgressLook::Line,
        scroll: ScrollLook::Overlay,
        caption: CaptionLook::System,
        focus: FocusLook::Dotted,
        button: ButtonLook::Fluent,
    };

    /// Sets the control with this key to the style named `value`; false when either is unknown.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        match (key, value) {
            ("field-focus", "line") => self.field_focus = FieldFocus::Line,
            ("field-focus", "border") => self.field_focus = FieldFocus::Border,
            ("field-focus", "ring") => self.field_focus = FieldFocus::Ring,
            ("check", "fluent") => self.check = CheckLook::Fluent,
            ("check", "square") => self.check = CheckLook::Square,
            ("check", "adwaita") => self.check = CheckLook::Adwaita,
            ("check", "breeze") => self.check = CheckLook::Breeze,
            ("check", "luna") => self.check = CheckLook::Luna,
            ("check", "aero") => self.check = CheckLook::Aero,
            ("check", "aqua") => self.check = CheckLook::Aqua,
            ("button", "fluent") => self.button = ButtonLook::Fluent,
            ("button", "breeze") => self.button = ButtonLook::Breeze,
            ("button", "luna") => self.button = ButtonLook::Luna,
            ("button", "aero") => self.button = ButtonLook::Aero,
            ("button", "aqua") => self.button = ButtonLook::Aqua,
            ("progress", "line") => self.progress = ProgressLook::Line,
            ("progress", "bar") => self.progress = ProgressLook::Bar,
            ("progress", "blocks") => self.progress = ProgressLook::Blocks,
            ("progress", "gloss") => self.progress = ProgressLook::Gloss,
            ("scroll", "overlay") => self.scroll = ScrollLook::Overlay,
            ("scroll", "classic") => self.scroll = ScrollLook::Classic,
            ("caption", "system") => self.caption = CaptionLook::System,
            ("caption", "metro") => self.caption = CaptionLook::Metro,
            ("caption", "adwaita") => self.caption = CaptionLook::Adwaita,
            ("caption", "breeze") => self.caption = CaptionLook::Breeze,
            ("caption", "luna") => self.caption = CaptionLook::Luna,
            ("caption", "aero") => self.caption = CaptionLook::Aero,
            ("caption", "aqua") => self.caption = CaptionLook::Aqua,
            ("focus", "dotted") => self.focus = FocusLook::Dotted,
            ("focus", "ring") => self.focus = FocusLook::Ring,
            ("focus", "frame") => self.focus = FocusLook::Frame,
            _ => return false,
        }
        true
    }
}

/// A whole theme: its two variants, its sizes and its icons.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// Built-in: its fixed id; from a file: the file's name without its extension.
    pub id: Cow<'static, str>,
    /// What the theme calls itself.
    pub name: Cow<'static, str>,
    pub light: Palette,
    pub dark: Palette,
    pub metrics: Metrics,
    pub icons: IconStyle,
    /// The theme's icon set (`crate::themeicons`), by name; None: the classic icons.
    pub icon_set: Option<Cow<'static, str>>,
    /// How each kind of control is drawn.
    pub controls: Controls,
}

impl Theme {
    /// The variant for the mode: dark or light.
    pub fn palette(&self, dark: bool) -> &Palette {
        if dark { &self.dark } else { &self.light }
    }
}

/// The classic look, MDM's default.
pub static CLASSIC: Theme = Theme {
    id: Cow::Borrowed("classic"),
    name: Cow::Borrowed("Classic"),
    light: Palette::classic(false),
    dark: Palette::classic(true),
    metrics: Metrics::CLASSIC,
    icons: IconStyle::Color,
    icon_set: None,
    controls: Controls::CLASSIC,
};

/// The built-in themes that don't change by themselves, the default first: the classic look
/// (written out above), then the ones shipped as theme files (`assets/themes/`, in the format
/// users write, so they double as examples). Native follows the system and is made when chosen
/// (`crate::native`).
pub fn builtins() -> [&'static Theme; 9] {
    static HIGH_CONTRAST: std::sync::LazyLock<Theme> = std::sync::LazyLock::new(|| {
        crate::themefile::builtin(
            "high-contrast",
            include_str!("../assets/themes/high-contrast.toml"),
        )
    });
    // The style themes (#45): their era's look, the layout kept.
    static METRO: std::sync::LazyLock<Theme> = std::sync::LazyLock::new(|| {
        crate::themefile::builtin("metro", include_str!("../assets/themes/metro.toml"))
    });
    static ADWAITA: std::sync::LazyLock<Theme> = std::sync::LazyLock::new(|| {
        crate::themefile::builtin("adwaita", include_str!("../assets/themes/adwaita.toml"))
    });
    static BREEZE: std::sync::LazyLock<Theme> = std::sync::LazyLock::new(|| {
        crate::themefile::builtin("breeze", include_str!("../assets/themes/breeze.toml"))
    });
    static LUNA: std::sync::LazyLock<Theme> = std::sync::LazyLock::new(|| {
        crate::themefile::builtin("luna", include_str!("../assets/themes/luna.toml"))
    });
    static AERO: std::sync::LazyLock<Theme> = std::sync::LazyLock::new(|| {
        crate::themefile::builtin("aero", include_str!("../assets/themes/aero.toml"))
    });
    static AQUA: std::sync::LazyLock<Theme> = std::sync::LazyLock::new(|| {
        crate::themefile::builtin("aqua", include_str!("../assets/themes/aqua.toml"))
    });
    static EMERALD: std::sync::LazyLock<Theme> = std::sync::LazyLock::new(|| {
        crate::themefile::builtin("emerald", include_str!("../assets/themes/emerald.toml"))
    });
    [
        &CLASSIC,
        &HIGH_CONTRAST,
        &METRO,
        &ADWAITA,
        &BREEZE,
        &LUNA,
        &AERO,
        &AQUA,
        &EMERALD,
    ]
}

/// `#rrggbb` or `#rrggbbaa` (the `#` optional, as Base16 writes them) as a colour.
pub fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.trim().trim_start_matches('#');
    if !h.is_ascii() || !(h.len() == 6 || h.len() == 8) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    let (r, g, b) = (byte(0)?, byte(2)?, byte(4)?);
    let a = if h.len() == 8 { byte(6)? } else { 0xff };
    Some(Color32::from_rgba_unmultiplied(r, g, b, a))
}

/// A colour as `#rrggbb`, or `#rrggbbaa` when it isn't opaque.
pub fn to_hex(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 0xff {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_and_size_has_one_key() {
        let mut keys: Vec<&str> = ROLES.iter().map(|(k, _)| *k).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "a role key is used twice");
        let mut sizes: Vec<&str> = SIZES.iter().map(|(k, _)| *k).collect();
        sizes.sort_unstable();
        let n = sizes.len();
        sizes.dedup();
        assert_eq!(sizes.len(), n, "a size key is used twice");
        for (k, _) in ROLES {
            assert!(CLASSIC.light.role(k).is_some());
            let mut p = CLASSIC.dark;
            assert!(p.set_role(k, Color32::RED));
            assert_eq!(p.role(k), Some(Color32::RED));
        }
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(parse_hex("#0078d4"), Some(rgb(0x00, 0x78, 0xd4)));
        assert_eq!(parse_hex("0078D4"), Some(rgb(0x00, 0x78, 0xd4)));
        assert_eq!(parse_hex("#33"), None);
        assert_eq!(parse_hex("#gg0000"), None);
        let c = parse_hex("#3399ff40").unwrap();
        assert_eq!(c, rgba(0x33, 0x99, 0xff, 0x40));
        assert_eq!(to_hex(rgb(0x00, 0x78, 0xd4)), "#0078d4");
    }

    /// The classic look keeps the values the painters had before themes (#45): a few anchors
    /// of each kind, so a slip in the table shows here as well as in the pixel comparison.
    #[test]
    fn classic_keeps_the_old_values() {
        let (l, d) = (&CLASSIC.light, &CLASSIC.dark);
        assert_eq!(
            (l.accent, d.accent),
            (rgb(0x00, 0x78, 0xd4), rgb(0x60, 0xcd, 0xff))
        );
        assert_eq!((l.window, d.window), (grey(0xf0), grey(0x39)));
        assert_eq!(
            (l.selection, d.selection),
            (rgb(0x00, 0x78, 0xd4), rgb(0x00, 0x78, 0xd4))
        );
        assert_eq!(
            (l.field_readonly, d.field_readonly),
            (grey(0xf3), grey(0x30))
        );
        assert_eq!(l.segments.missing[0], hex(0xfafafc));
        assert_eq!(d.segments.missing[0], rgb(77, 77, 77));
        assert!(d.dark && !l.dark);
        assert_eq!(CLASSIC.metrics.control_radius, 3.0);
        assert_eq!(CLASSIC.metrics.list_row, 17.0);
    }

    /// Every built-in theme's text reads (WCAG 2.1 AA, 4.5:1), High contrast's at AAA (7:1),
    /// and its accent stands out from the background (3:1).
    #[test]
    fn built_in_themes_meet_wcag_contrast() {
        use crate::derive::contrast;
        let mut themes: Vec<Theme> = builtins().iter().map(|t| (*t).clone()).collect();
        // Native with a few accents, and with Windows' high-contrast colours.
        for accent in [
            rgb(0x00, 0x78, 0xd4),
            rgb(0xe7, 0x48, 0x56),
            rgb(0xff, 0xb9, 0x00),
        ] {
            themes.push(crate::native::theme(&crate::native::System {
                accent: Some(crate::native::Accent::from_color(accent)),
                ..Default::default()
            }));
        }
        themes.push(crate::native::theme(&crate::native::System {
            high_contrast: Some([
                grey(0x00),
                grey(0xff),
                rgb(0x1a, 0xeb, 0xff),
                grey(0x00),
                rgb(0xff, 0xff, 0x00),
                rgb(0x3f, 0xf2, 0x3f),
            ]),
            ..Default::default()
        }));
        for t in &themes {
            let high = t.id == "high-contrast" || t.light.focus_ring == t.light.accent;
            let min = if high { 7.0 } else { 4.5 };
            for p in [&t.light, &t.dark] {
                let pairs = [
                    ("text / window", p.text, p.window),
                    ("text / field", p.text, p.field),
                    ("text / list", p.text, p.list),
                    ("menu text / menu", p.menu_text, p.menu),
                    ("selection", p.selection_text, p.selection),
                    ("button text", p.button_text, p.button),
                    ("muted / window", p.muted, p.window),
                ];
                for (what, fg, bg) in pairs {
                    let r = contrast(fg, bg);
                    // The classic look keeps today's pixels: its dark grey notes on the dark
                    // window are 4.42:1 (#a0a0a0 on #393939), reported for its own fix.
                    if t.id == "classic" && p.dark && what == "muted / window" {
                        assert!(r >= 4.4, "{} {}: {what} {r:.2}", t.id, p.dark);
                        continue;
                    }
                    assert!(r >= min, "{} {}: {what} {r:.2}", t.id, p.dark);
                }
                let r = contrast(p.link, p.window);
                assert!(r >= 4.5, "{} {}: link {r:.2}", t.id, p.dark);
                let r = contrast(p.accent, p.window);
                assert!(r >= 3.0, "{} {}: accent {r:.2}", t.id, p.dark);
                let r = contrast(p.button_accent_text, p.button_accent);
                assert!(r >= 4.5, "{} {}: accent button {r:.2}", t.id, p.dark);
            }
        }
    }

    /// The style themes' keyboard focus ring is seen (WCAG 2.1 non-text contrast, 3:1) on the
    /// window and on the boxes, light and dark, its transparency included. Classic keeps IDM's
    /// dotted grey rectangle.
    #[test]
    fn style_themes_focus_rings_are_seen() {
        use crate::derive::contrast;
        let over = |fg: Color32, bg: Color32| {
            let a = fg.a() as f32 / 255.0;
            // Color32 is premultiplied: its channels already carry the alpha.
            let mix = |f: u8, b: u8| (f as f32 + b as f32 * (1.0 - a)).round().min(255.0) as u8;
            Color32::from_rgb(
                mix(fg.r(), bg.r()),
                mix(fg.g(), bg.g()),
                mix(fg.b(), bg.b()),
            )
        };
        for t in builtins() {
            if t.id == "classic" || t.id == "high-contrast" {
                continue;
            }
            for p in [&t.light, &t.dark] {
                for bg in [p.window, p.field] {
                    let r = contrast(over(p.focus_ring, bg), bg);
                    assert!(r >= 3.0, "{} {}: focus ring {r:.2} on {bg:?}", t.id, p.dark);
                }
            }
        }
    }

    /// A theme shipped as a file reads without a warning.
    #[test]
    fn built_in_theme_files_read_cleanly() {
        for (id, name, text) in [
            (
                "high-contrast",
                "High contrast",
                include_str!("../assets/themes/high-contrast.toml"),
            ),
            (
                "metro",
                "Metro",
                include_str!("../assets/themes/metro.toml"),
            ),
            (
                "adwaita",
                "Adwaita",
                include_str!("../assets/themes/adwaita.toml"),
            ),
            (
                "breeze",
                "Breeze",
                include_str!("../assets/themes/breeze.toml"),
            ),
            ("luna", "Luna", include_str!("../assets/themes/luna.toml")),
            ("aero", "Aero", include_str!("../assets/themes/aero.toml")),
            ("aqua", "Aqua", include_str!("../assets/themes/aqua.toml")),
            (
                "emerald",
                "Emerald",
                include_str!("../assets/themes/emerald.toml"),
            ),
        ] {
            let v: serde_json::Value = toml::from_str(text).unwrap();
            let l = crate::themefile::parse(id, &v, |_| {
                Ok(crate::themefile::Loaded {
                    theme: CLASSIC.clone(),
                    warnings: Vec::new(),
                })
            })
            .unwrap();
            assert!(l.warnings.is_empty(), "{id}: {:?}", l.warnings);
            assert_eq!(l.theme.name, name);
            assert!(l.theme.dark.dark && !l.theme.light.dark);
        }
        let metro = builtins()[2];
        assert_eq!(metro.icon_set.as_deref(), Some("metro"));
        assert_eq!(metro.controls.caption, CaptionLook::Metro);
        assert_eq!(metro.metrics.control_radius, 0.0);
        let adwaita = builtins()[3];
        assert_eq!(adwaita.icon_set.as_deref(), Some("adwaita"));
        assert_eq!(adwaita.controls.caption, CaptionLook::Adwaita);
        assert_eq!(adwaita.controls.focus, FocusLook::Ring);
        assert_eq!(adwaita.controls.check, CheckLook::Adwaita);
        assert_eq!(adwaita.controls.field_focus, FieldFocus::Ring);
        let breeze = builtins()[4];
        assert_eq!(breeze.icon_set.as_deref(), Some("breeze"));
        assert_eq!(breeze.controls.caption, CaptionLook::Breeze);
        assert_eq!(breeze.controls.focus, FocusLook::Frame);
        assert_eq!(breeze.controls.check, CheckLook::Breeze);
        assert_eq!(breeze.controls.button, ButtonLook::Breeze);
        assert_eq!(breeze.controls.field_focus, FieldFocus::Border);
        // The era themes: their own buttons, boxes, bars and title bars, and icon sets.
        for (i, id, button, check, progress, caption) in [
            (
                5,
                "luna",
                ButtonLook::Luna,
                CheckLook::Luna,
                ProgressLook::Blocks,
                CaptionLook::Luna,
            ),
            (
                6,
                "aero",
                ButtonLook::Aero,
                CheckLook::Aero,
                ProgressLook::Gloss,
                CaptionLook::Aero,
            ),
            (
                7,
                "aqua",
                ButtonLook::Aqua,
                CheckLook::Aqua,
                ProgressLook::Gloss,
                CaptionLook::Aqua,
            ),
        ] {
            let t = builtins()[i];
            assert_eq!(t.id, id);
            assert_eq!(t.icon_set.as_deref(), Some(id));
            assert_eq!(t.controls.button, button, "{id}");
            assert_eq!(t.controls.check, check, "{id}");
            assert_eq!(t.controls.progress, progress, "{id}");
            assert_eq!(t.controls.caption, caption, "{id}");
            // Their buttons shine: the gloss isn't the face.
            for p in [&t.light, &t.dark] {
                assert_ne!(p.gloss, p.button, "{id} {}", p.dark);
            }
        }
        // Aqua's keyboard focus is a ring, as Mac OS X draws it; Luna's and Aero's Windows'
        // dotted rectangle.
        assert_eq!(builtins()[7].controls.focus, FocusLook::Ring);
        assert_eq!(builtins()[5].controls.focus, FocusLook::Dotted);
        assert_eq!(builtins()[6].controls.focus, FocusLook::Dotted);
        // Emerald: flat 6-point corners, ring focus, and its chips inked rather than lettered
        // in white — the soft fill carries the near-black text in both variants.
        let emerald = builtins()[8];
        assert_eq!(emerald.metrics.control_radius, 6.0);
        assert_eq!(emerald.controls.field_focus, FieldFocus::Ring);
        assert_eq!(emerald.controls.focus, FocusLook::Ring);
        assert_eq!(emerald.controls.button, ButtonLook::Fluent);
        assert_eq!(emerald.controls.check, CheckLook::Fluent);
        assert_eq!(emerald.controls.progress, ProgressLook::Bar);
        assert_eq!(emerald.controls.scroll, ScrollLook::Overlay);
        use crate::derive::contrast;
        for p in [&emerald.light, &emerald.dark] {
            assert!(contrast(p.chip_text, p.chip_web) >= 4.5);
            assert!(contrast(p.chip_text, p.chip_torrent) >= 4.5);
            assert!(contrast(p.text, p.window) >= 4.5);
        }
    }

    /// The classic look's dark secondary and error texts read on every dark surface they sit on
    /// (WCAG 2.1 AA, 4.5:1): the window, a tab page, a text box.
    #[test]
    fn classic_dark_texts_read() {
        let ratio = |a: Color32, b: Color32| {
            let lum = |c: Color32| {
                let f = |v: u8| {
                    let v = v as f32 / 255.0;
                    if v <= 0.04045 {
                        v / 12.92
                    } else {
                        ((v + 0.055) / 1.055).powf(2.4)
                    }
                };
                0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
            };
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        };
        let d = &CLASSIC.dark;
        for fg in [d.muted, d.error, d.error_strong] {
            for bg in [d.window, d.tab_page, d.field] {
                assert!(ratio(fg, bg) >= 4.5, "{fg:?} on {bg:?}: {}", ratio(fg, bg));
            }
        }
    }

    /// The rows between a dark variant's menu bar and its toolbar (#379): none of them stands out
    /// against the surfaces around them. The classic dark palette's were near-white (the dark
    /// look of IDM's era retired on 2026-11-01): a bright band across the window, under the menu
    /// bar. High contrast's lines are meant to stand out.
    #[test]
    fn dark_variants_draw_no_bright_line_under_the_menu_bar() {
        let lum = |c: Color32| {
            let f = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
        };
        let ratio = |a: Color32, b: Color32| {
            let (x, y) = (lum(a), lum(b));
            (x.max(y) + 0.05) / (x.min(y) + 0.05)
        };
        for theme in builtins().iter().filter(|t| t.id != "high-contrast") {
            let d = &theme.dark;
            for (what, line) in [
                ("menubar.line", d.menubar_line),
                ("toolbar.line", d.toolbar_line),
                ("toolbar.line.light", d.toolbar_line_light),
            ] {
                for bg in [d.menubar, d.window] {
                    assert!(
                        ratio(line, bg) <= 2.5,
                        "{}: {what} {line:?} on {bg:?}: {}",
                        theme.id,
                        ratio(line, bg)
                    );
                }
            }
        }
    }
}
