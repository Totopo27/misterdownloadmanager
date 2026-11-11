//! The Native theme (#45): the classic layout and controls in the system's own colours. On
//! Windows its accent (Settings → Personalization → Colors) and, when it is on, its high
//! contrast theme; on Linux the desktop's accent and its "higher contrast" setting (the
//! freedesktop portal); on macOS the accent chosen in System Settings. Light or dark follows the
//! system as every theme's mode does (`theme::update`).

use egui::Color32;

use crate::palette::{CLASSIC, Palette, Theme};

/// The system's accent and, when the system gives them, its lighter and darker shades.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Accent {
    pub base: Color32,
    /// Light1, Light2, Light3 (Windows' names): lighter and lighter.
    pub light: [Color32; 3],
    /// Dark1, Dark2, Dark3: darker and darker.
    pub dark: [Color32; 3],
}

impl Accent {
    /// An accent the system gives as one colour: its shades mixed in (Linux, macOS; Windows
    /// gives its shades).
    pub fn from_color(base: Color32) -> Self {
        use crate::derive::shade;
        Accent {
            base,
            light: [shade(base, 0.08), shade(base, 0.16), shade(base, 0.24)],
            dark: [shade(base, -0.08), shade(base, -0.16), shade(base, -0.24)],
        }
    }
}

/// What the system says about its colours now.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct System {
    pub accent: Option<Accent>,
    /// The system's high-contrast colours, when high contrast is on: background, text,
    /// selection, selection's text, links, greyed text.
    pub high_contrast: Option<[Color32; 6]>,
    /// High contrast is asked for without colours of its own (Linux's portal): the built-in
    /// High contrast theme's then.
    pub more_contrast: bool,
}

/// Windows' `AccentPalette` bytes: 8 colours of R G B and an unused byte, lightest first
/// (Light3, Light2, Light1, the accent, Dark1, Dark2, Dark3, one more).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn accent_palette_bytes(b: &[u8; 32]) -> [[u8; 3]; 7] {
    let mut out = [[0u8; 3]; 7];
    for (i, c) in out.iter_mut().enumerate() {
        *c = [b[i * 4], b[i * 4 + 1], b[i * 4 + 2]];
    }
    out
}

/// What the system says (what the watcher of Windows' registry or the portal's thread last
/// heard: never read on the window's thread, #719).
/// For tests and screenshots, `MDM_NATIVE_ACCENT=#rrggbb` stands for the system's accent and
/// `MDM_NATIVE_CONTRAST=1` for its high contrast, so no system setting is touched.
pub fn system(ctx: &egui::Context) -> System {
    let mut sys = system_says(ctx);
    if let Some(c) = std::env::var("MDM_NATIVE_ACCENT")
        .ok()
        .and_then(|v| crate::palette::parse_hex(&v))
    {
        sys.accent = Some(Accent::from_color(c));
    }
    if std::env::var_os("MDM_NATIVE_CONTRAST").is_some_and(|v| v == "1") {
        sys.more_contrast = true;
    }
    sys
}

fn system_says(ctx: &egui::Context) -> System {
    #[cfg(windows)]
    {
        crate::lookwatch::system(ctx).native
    }
    #[cfg(target_os = "linux")]
    {
        System {
            accent: crate::unix::desktop_accent(ctx).map(Accent::from_color),
            high_contrast: None,
            more_contrast: crate::unix::desktop_contrast(ctx),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = ctx;
        System {
            accent: crate::unix::mac_accent().map(Accent::from_color),
            high_contrast: None,
            more_contrast: false,
        }
    }
}

/// What Windows says now: its accent from the registry, its high-contrast colours. For the
/// thread that watches them (`lookwatch`): a registry read can wait for seconds.
#[cfg(windows)]
pub fn windows_now() -> System {
    let rgb = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
    System {
        accent: crate::win32::accent_palette().map(|p| Accent {
            base: rgb(p[3]),
            light: [rgb(p[2]), rgb(p[1]), rgb(p[0])],
            dark: [rgb(p[4]), rgb(p[5]), rgb(p[6])],
        }),
        high_contrast: crate::win32::high_contrast_colors(),
        more_contrast: false,
    }
}

/// The Native theme for what the system says.
pub fn theme(sys: &System) -> Theme {
    if let Some([bg, text, selection, _selection_text, link, _greyed]) = sys.high_contrast {
        // Windows' own high-contrast colours, the same in both modes (Windows has one set).
        let mut t = crate::palette::builtins()[1].clone();
        let mut p = crate::derive::derive(&crate::derive::Seeds {
            background: bg,
            surface: bg,
            header: bg,
            text,
            accent: link,
            selection: Some(selection),
            error: text,
            warning: text,
            success: text,
        });
        crate::derive::strengthen(&mut p);
        t.light = p;
        t.dark = p;
        t.id = "native".into();
        t.name = "Native".into();
        return t;
    }
    if sys.more_contrast {
        let mut t = crate::palette::builtins()[1].clone();
        t.id = "native".into();
        t.name = "Native".into();
        return t;
    }
    let mut t = CLASSIC.clone();
    t.id = "native".into();
    t.name = "Native".into();
    // Secondary text at 4.5:1 at least (the classic dark notes are 4.42:1).
    for p in [&mut t.light, &mut t.dark] {
        p.muted = crate::derive::readable(p.muted, p.window, 4.5);
    }
    if let Some(a) = sys.accent {
        // Windows 11 fills with the accent itself in light mode and with Light2 in dark mode
        // (the classic look's #0078D4 and #60CDFF are the default blue's).
        with_accent(&mut t.light, a.base, false);
        with_accent(&mut t.dark, a.light[1], true);
    }
    t
}

/// The accent's roles in `p` from `accent`, with the hover, pressed and tint shades the classic
/// look derives from its blue (the classic values come back for the default blue).
fn with_accent(p: &mut Palette, accent: Color32, dark: bool) {
    use crate::derive::{ink_on, mix};
    // A light accent (yellow) on a light window, or a dark one on a dark window, is moved until
    // it stands out (3:1), as focus lines and check boxes must.
    let accent = crate::derive::readable(accent, p.window, 3.0);
    let white = Color32::WHITE;
    let black = Color32::BLACK;
    let scale = |c: Color32, f: f32| {
        Color32::from_rgb(
            (c.r() as f32 * f).round() as u8,
            (c.g() as f32 * f).round() as u8,
            (c.b() as f32 * f).round() as u8,
        )
    };
    let lerp = |a: Color32, b: Color32, t: f32| {
        let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
        Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
    };
    let (hot, pressed, check_pressed, bottom) = if dark {
        (
            scale(accent, 0.9),
            scale(accent, 0.82),
            scale(accent, 0.8),
            scale(accent, 0.88),
        )
    } else {
        (
            lerp(accent, white, 0.1),
            scale(accent, 0.9),
            lerp(accent, white, 0.2),
            scale(accent, 0.75),
        )
    };
    let text = ink_on(accent);
    p.accent = accent;
    p.selection = if dark {
        mix(accent, black, 0.45)
    } else {
        accent
    };
    p.selection = if contrast_ok(p.selection) {
        p.selection
    } else {
        accent
    };
    p.selection_text = ink_on(p.selection);
    p.progress_bar = accent;
    p.check_on = accent;
    p.check_on_hot = hot;
    p.check_on_pressed = check_pressed;
    p.check_mark = text;
    p.button_accent = accent;
    p.button_accent_hot = hot;
    p.button_accent_pressed = pressed;
    p.button_accent_bottom = bottom;
    p.button_accent_text = text;
    p.button_accent_text_pressed = mix(text, pressed, 0.1);
    p.button_default_border = accent;
    p.button_default_bottom = bottom;
    p.marquee = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 0x40);
    p.marquee_border = accent;
    let base = if dark { p.window } else { white };
    p.highlight_hot = lerp(base, accent, if dark { 0.25 } else { 0.1 });
    p.highlight_pressed = lerp(base, accent, 0.2);
    p.toolbar_hot = lerp(p.window, accent, if dark { 0.18 } else { 0.1 });
    p.toolbar_hot_faint = lerp(p.window, accent, 0.05);
    p.toolbar_pressed = lerp(p.window, accent, if dark { 0.12 } else { 0.2 });
    p.toolbar_hot_edge = lerp(p.window, accent, if dark { 0.35 } else { 0.2 });
    if !dark {
        p.toolbar_hot_border = p.toolbar_hot_edge;
    }
    p.link = crate::derive::readable(accent, p.window, 4.5);
}

/// White text on a selection keeps 4.5:1 (a light accent's dark-mode selection gets darker).
fn contrast_ok(c: Color32) -> bool {
    crate::derive::contrast(Color32::WHITE, c) >= 4.5
        || crate::derive::contrast(Color32::BLACK, c) >= 4.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_accent_palette_bytes() {
        // The default blue's palette as Windows 11 stores it.
        let mut b = [0u8; 32];
        let rows: [[u8; 3]; 8] = [
            [0x99, 0xeb, 0xff],
            [0x4c, 0xc2, 0xff],
            [0x00, 0x91, 0xf8],
            [0x00, 0x78, 0xd4],
            [0x00, 0x5a, 0x9e],
            [0x00, 0x42, 0x75],
            [0x00, 0x26, 0x42],
            [0xf7, 0x63, 0x0c],
        ];
        for (i, r) in rows.iter().enumerate() {
            b[i * 4..i * 4 + 3].copy_from_slice(r);
        }
        let p = accent_palette_bytes(&b);
        assert_eq!(p[3], [0x00, 0x78, 0xd4]);
        assert_eq!(p[0], [0x99, 0xeb, 0xff]);
    }

    /// The default blue gives the classic look's light accent roles back.
    #[test]
    fn the_default_blue_is_the_classic_look() {
        let mut p = CLASSIC.light;
        with_accent(&mut p, Color32::from_rgb(0x00, 0x78, 0xd4), false);
        // Within one step of 255 per channel (the classic shades were rounded by hand).
        let near = |a: Color32, b: Color32| {
            (0..3).all(|i| (a.to_array()[i] as i32 - b.to_array()[i] as i32).abs() <= 1)
        };
        let c = &CLASSIC.light;
        assert_eq!(p.accent, c.accent);
        assert_eq!(p.selection, c.selection);
        for (mine, classic) in [
            (p.button_accent_hot, c.button_accent_hot),
            (p.button_accent_pressed, c.button_accent_pressed),
            (p.check_on_hot, c.check_on_hot),
            (p.check_on_pressed, c.check_on_pressed),
            (p.highlight_hot, c.highlight_hot),
        ] {
            assert!(near(mine, classic), "{mine:?} vs {classic:?}");
        }
    }
}
