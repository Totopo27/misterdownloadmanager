//! The look: light or dark (the mode), the active theme's colours and sizes (#45), the fonts,
//! and egui's own visuals built from them.

use egui::{CornerRadius, Stroke, Visuals};

use std::cell::Cell;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::palette::{CLASSIC, Metrics, Palette, Theme};

static DARK: AtomicBool = AtomicBool::new(true);

/// Is the UI in dark mode? IDM: dark when "View → Dark Mode support" is on and Windows' app
/// mode is dark (IDM 6.37); light otherwise. It picks the active theme's dark or light variant
/// ([`colors`]); whether that variant is a dark surface is [`dark_surface`].
pub fn dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

// ---- the active theme ------------------------------------------------------------------------
//
// Painters read colours and sizes hundreds of times a frame, from every window's thread: each
// thread keeps a reference to the active theme and looks again only when the generation moved
// (one atomic load per read, no lock, no allocation). A theme set from a file is leaked: it
// lives as long as the program (a few KB per change), so a reference to it never dangles.

static ACTIVE: RwLock<&'static Theme> = RwLock::new(&CLASSIC);
static GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static SEEN: Cell<(u64, &'static Theme)> = const { Cell::new((0, &CLASSIC)) };
}

/// The active theme.
pub fn current() -> &'static Theme {
    let generation = GENERATION.load(Ordering::Acquire);
    SEEN.with(|seen| {
        let (at, theme) = seen.get();
        if at == generation {
            return theme;
        }
        let theme = *ACTIVE.read().unwrap_or_else(|e| e.into_inner());
        seen.set((generation, theme));
        theme
    })
}

/// The active theme's colours in the current mode.
pub fn colors() -> &'static Palette {
    current().palette(dark())
}

/// The active theme's sizes.
pub fn metrics() -> &'static Metrics {
    &current().metrics
}

/// Is what is drawn now a dark surface (light text on dark)? The classic look's dark variant
/// is; a theme says for each of its variants. Glyph rendering and the window captions follow it.
pub fn dark_surface() -> bool {
    colors().dark
}

/// Makes `theme` the active one.
pub fn set(theme: Theme) {
    let theme: &'static Theme = match crate::palette::builtins()
        .into_iter()
        .find(|b| **b == theme)
    {
        Some(builtin) => builtin,
        None => Box::leak(Box::new(theme)),
    };
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = theme;
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// Makes the theme with this id the active one: a built-in, or a file in the themes folder
/// (`themefile`), or a theme file's path; Native is made from what the system says. A theme
/// that can't be read leaves the classic look, logged. Returns what was skipped or went wrong.
pub fn select(id: &str, ctx: &egui::Context) -> Vec<String> {
    let id = id.to_owned();
    // pywal's file is followed again only if this theme is made from it (`themefile::load_in`).
    crate::themefile::forget_pywal();
    *SELECTED.lock().unwrap_or_else(|e| e.into_inner()) = id.clone();
    if id.is_empty() || id == CLASSIC.id {
        set(CLASSIC.clone());
        return Vec::new();
    }
    if id == "native" {
        let system = crate::native::system(ctx);
        *NATIVE_SEEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(system);
        set(crate::native::theme(&system));
        return Vec::new();
    }
    match load(&id) {
        Ok(loaded) => {
            for w in &loaded.warnings {
                tracing::warn!("theme {id}: {w}");
            }
            set(loaded.theme);
            loaded.warnings
        }
        Err(e) => {
            tracing::warn!("theme {id}: {e}; the classic look instead");
            set(CLASSIC.clone());
            vec![e]
        }
    }
}

/// A theme file's theme, by its id or its path; pywal's file followed from now on when the
/// theme is made from it.
fn load(id: &str) -> Result<crate::themefile::Loaded, String> {
    let (loaded, followed) = crate::themefile::read(id);
    if loaded.is_ok() {
        crate::themefile::follow_pywal(followed);
    }
    loaded
}

/// The id (or path) [`select`] was last given.
static SELECTED: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// The id (or path) of the theme chosen last.
pub fn selected() -> String {
    SELECTED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// While the theme is made from pywal's colours (#45): read again when `wal` wrote them anew,
/// as it does for each new wallpaper; `lookwatch`'s thread reads it (#719), this takes what it
/// read while the theme and the file it follows are still those it was read for. A file that
/// doesn't read (half written as it was read) keeps the theme as it is, and is read again when
/// it is next written. True when the theme changed (the caller applies it: `apply_theme`,
/// `apply_mode`).
pub fn follow_pywal(files: Option<&crate::lookwatch::ThemeFiles>) -> bool {
    let Some(r) = files.and_then(|f| f.pywal.as_ref()) else {
        return false;
    };
    if selected() != r.id || crate::themefile::following_pywal().as_ref() != Some(&r.followed) {
        return false;
    }
    let id = &r.id;
    match &r.read {
        Ok((theme, _)) => {
            tracing::info!("theme {id}: pywal's colours changed, read again");
            set(theme.clone());
            crate::themefile::follow_pywal(r.follow.clone());
            true
        }
        Err(e) => {
            tracing::warn!("theme {id}: {e}; the theme kept until pywal's file is written again");
            crate::themefile::pywal_seen_as(r.written);
            false
        }
    }
}

/// The theme to start with: `MDM_THEME` (tests and screenshots: an id or a theme file's path),
/// else the setting.
pub fn startup_id(setting: &str) -> String {
    std::env::var("MDM_THEME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| setting.to_owned())
}

/// What the system said when Native was last made.
static NATIVE_SEEN: std::sync::Mutex<Option<crate::native::System>> = std::sync::Mutex::new(None);

/// While Native is the theme: made again when the system's accent or contrast changed. True
/// when it was (the caller applies it: `apply_theme`, `apply_mode`).
pub fn follow_system(ctx: &egui::Context) -> bool {
    if current().id != "native" {
        return false;
    }
    let system = crate::native::system(ctx);
    let mut seen = NATIVE_SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if *seen == Some(system) {
        return false;
    }
    *seen = Some(system);
    drop(seen);
    set(crate::native::theme(&system));
    true
}

/// Match egui's glyph rendering to the surface: a dark one thickens light-on-dark text
/// (`TwoCoverageMinusCoverageSq`), which makes black-on-white text look heavy, so a light one
/// uses plain coverage. The glyph atlas is shared, so this follows the main window's.
pub fn apply_mode(ctx: &egui::Context) {
    let dark_surface = dark_surface();
    let tf = if dark_surface {
        egui::epaint::FontColorTransferFunction::DARK_MODE_DEFAULT
    } else {
        egui::epaint::FontColorTransferFunction::LIGHT_MODE_DEFAULT
    };
    ctx.all_styles_mut(|s| s.visuals.text_options.color_transfer_function = tf);
    // egui's dark or light style is the mode's variant (`apply_theme` puts each variant into its
    // egui theme). It leaves the windows' captions alone (`apply`: `sync_window_theme`).
    ctx.set_theme(if dark() {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    });
    // The window's own theme (caption, DWM): otherwise winit follows the system theme.
    ctx.send_viewport_cmd_to(
        egui::ViewportId::ROOT,
        egui::ViewportCommand::SetTheme(system_theme(dark_surface)),
    );
}

/// The winit window theme for a dark (true) or light caption.
pub fn system_theme(dark: bool) -> egui::SystemTheme {
    if dark {
        egui::SystemTheme::Dark
    } else {
        egui::SystemTheme::Light
    }
}

/// Decide the mode from the setting and the system's: Windows' app mode; on Linux the desktop's
/// preference (the freedesktop portal), else what the window system reports; on macOS the
/// appearance the window system reports (light when nothing says). True if it changed.
/// `always_dark` (View → Always dark, #60): dark whatever the system says, for desktops that
/// report no preference.
/// On Windows the registry is read by `lookwatch`'s thread, never this one (#719).
pub fn update(dark_mode_support: bool, always_dark: bool, ctx: &egui::Context) -> bool {
    #[cfg(windows)]
    let system_dark = crate::lookwatch::system(ctx).dark;
    #[cfg(target_os = "linux")]
    let system_dark = crate::unix::desktop_dark(ctx)
        .unwrap_or_else(|| ctx.system_theme() == Some(egui::Theme::Dark));
    #[cfg(not(any(windows, target_os = "linux")))]
    let system_dark = ctx.system_theme() == Some(egui::Theme::Dark);
    let want = wanted(dark_mode_support, always_dark, system_dark);
    DARK.swap(want, Ordering::Relaxed) != want
}

/// Did the desktop say its look changed since the last call, or the theme files? The watchers
/// count each change: on Linux the portal's and the theme settings' (`unix::desktop_generation`),
/// everywhere `lookwatch`'s (Windows' registry, the themes folder, pywal's file). The app's theme
/// check, otherwise every 2 s, then runs in the frame the change woke (#156, #719).
pub fn system_changed() -> bool {
    static WATCHED: AtomicU64 = AtomicU64::new(0);
    let now = crate::lookwatch::generation();
    let watched = WATCHED.swap(now, Ordering::Relaxed) != now;
    #[cfg(target_os = "linux")]
    {
        static SEEN: AtomicU64 = AtomicU64::new(0);
        let now = crate::unix::desktop_generation();
        (SEEN.swap(now, Ordering::Relaxed) != now) | watched
    }
    #[cfg(not(target_os = "linux"))]
    watched
}

/// Dark or not: View → Always dark, else the system's mode while Dark Mode support is on.
fn wanted(dark_mode_support: bool, always_dark: bool, system_dark: bool) -> bool {
    always_dark || (dark_mode_support && system_dark)
}

/// A module's names for the active theme's colours: `pub NAME => role;` becomes `NAME()`, the
/// role's colour in the current mode ([`colors`]).
macro_rules! themed {
    ($($(#[$m:meta])* $vis:vis $name:ident => $role:ident;)*) => {$(
        $(#[$m])*
        #[allow(non_snake_case)]
        $vis fn $name() -> egui::Color32 {
            $crate::theme::colors().$role
        }
    )*};
}
pub(crate) use themed;

/// The system's fonts for the scripts the UI font lacks (D-019; nothing bundled): a key, the
/// files to try (in the fonts folder), the interface languages that need it. CJK fonts are large
/// (13–20 MB), so only the interface language's are loaded, and all of them while the language
/// picker shows every language by its own name ([`all_scripts`]). Windows: Segoe UI and Tahoma
/// already cover Arabic, Hebrew, Armenian and Georgian.
#[cfg(windows)]
const SCRIPT_FONTS: &[(&str, &[&str], &[&str])] = &[
    ("han-sc", &["msyh.ttc"], &["zh-hans"]),
    ("han-tc", &["msjh.ttc"], &["zh-hant"]),
    (
        "japanese",
        &["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"],
        &["ja"],
    ),
    ("hangul", &["malgun.ttf"], &["ko"]),
    (
        "indic",
        &["Nirmala.ttc", "Nirmala.ttf"],
        &[
            "as", "bn", "gu", "hi", "kn", "kok", "ml", "mr", "ne", "or", "pa", "ta", "te",
        ],
    ),
    ("southeast-asian", &["LeelawUI.ttf"], &["km", "lo", "th"]),
    ("ethiopic", &["ebrima.ttf"], &["am"]),
    ("cherokee", &["gadugi.ttf"], &["chr"]),
];

/// macOS's fonts, in /System/Library/Fonts: the specific ones first, then Arial Unicode for
/// every other script the system font lacks.
#[cfg(target_os = "macos")]
const SCRIPT_FONTS: &[(&str, &[&str], &[&str])] = &[
    (
        "han",
        &[
            "PingFang.ttc",
            "Hiragino Sans GB.ttc",
            "STHeiti Light.ttc",
            "Supplemental/Songti.ttc",
        ],
        &["zh-hans", "zh-hant"],
    ),
    ("japanese", &["ヒラギノ角ゴシック W3.ttc"], &["ja"]),
    ("hangul", &["AppleSDGothicNeo.ttc"], &["ko"]),
    ("khmer", &["Supplemental/Khmer Sangam MN.ttf"], &["km"]),
    ("lao", &["Supplemental/Lao Sangam MN.ttf"], &["lo"]),
    ("ethiopic", &["Supplemental/Kefa.ttc", "Kefa.ttc"], &["am"]),
    (
        "cherokee",
        &["Supplemental/Plantagenet Cherokee.ttf"],
        &["chr"],
    ),
    (
        "unicode",
        &["Supplemental/Arial Unicode.ttf"],
        &[
            "ar", "fa", "ur", "ug", "he", "hy", "ka", "as", "bn", "gu", "hi", "kn", "kok", "ml",
            "mr", "ne", "or", "pa", "ta", "te", "th", "ja", "ko", "zh-hans", "zh-hant",
        ],
    ),
];

/// Linux: fontconfig names the font for each language (`fc-match :lang=…`), whatever the
/// distribution installs; these are the languages whose script needs one, as fontconfig names
/// them (one per script is enough for the picker's list of all languages).
#[cfg(not(any(windows, target_os = "macos")))]
const SCRIPT_LANGS: &[(&str, &str)] = &[
    ("ar", "ar"),
    ("fa", "fa"),
    ("ur", "ur"),
    ("ug", "ug"),
    ("he", "he"),
    ("hy", "hy"),
    ("ka", "ka"),
    ("am", "am"),
    ("chr", "chr"),
    ("km", "km"),
    ("lo", "lo"),
    ("th", "th"),
    ("ja", "ja"),
    ("ko", "ko"),
    ("zh-hans", "zh-cn"),
    ("zh-hant", "zh-tw"),
    ("hi", "hi"),
    ("mr", "mr"),
    ("ne", "ne"),
    ("kok", "kok"),
    ("as", "as"),
    ("bn", "bn"),
    ("gu", "gu"),
    ("pa", "pa"),
    ("or", "or"),
    ("ta", "ta"),
    ("te", "te"),
    ("kn", "kn"),
    ("ml", "ml"),
];

/// Marks that neither the interface font nor the script fonts have (✔ ✖ ↩ ⇧ ⌘ ⌥ ⌦, and → on
/// Linux: #185): the system's symbol fonts, last in every family's chain, in every language,
/// each one that is there. macOS keeps ✔ and ✖ in Zapf Dingbats, not in Apple Symbols.
#[cfg(windows)]
const SYMBOL_FONTS: &[&str] = &["seguisym.ttf"];
#[cfg(target_os = "macos")]
const SYMBOL_FONTS: &[&str] = &["Apple Symbols.ttf", "ZapfDingbats.ttf"];
/// Linux: the font fontconfig picks for those marks (glyph_tests lists every one the windows
/// draw; a new one goes here too).
#[cfg(not(any(windows, target_os = "macos")))]
const SYMBOL_PATTERN: &str = ":charset=2024 2192 21a9 21e7 2318 2325 2326 2714 2716";

/// The symbol fonts' bytes, in fallback order ([`SYMBOL_FONTS`], [`SYMBOL_PATTERN`]).
fn symbol_fonts() -> Vec<Vec<u8>> {
    #[cfg(any(windows, target_os = "macos"))]
    return SYMBOL_FONTS
        .iter()
        .filter_map(|f| std::fs::read(font_dir().join(f)).ok())
        .collect();
    #[cfg(not(any(windows, target_os = "macos")))]
    return fc_match(SYMBOL_PATTERN)
        .and_then(|f| std::fs::read(f).ok())
        .into_iter()
        .collect();
}

/// The folder the font tables name files in.
#[cfg(any(windows, target_os = "macos"))]
fn font_dir() -> std::path::PathBuf {
    #[cfg(windows)]
    return std::env::var_os("WINDIR")
        .map(|w| std::path::PathBuf::from(w).join("Fonts"))
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows\Fonts"));
    #[cfg(target_os = "macos")]
    return std::path::PathBuf::from("/System/Library/Fonts");
}

/// Resolves a font file path in `dir`. On macOS 15+ (Sequoia / Tahoe), `PingFang.ttc` moved out
/// of `/System/Library/Fonts/PingFang.ttc` into `/System/Library/AssetsV2/com_apple_MobileAsset_Font*`.
#[cfg(any(windows, target_os = "macos"))]
fn font_path(dir: &std::path::Path, rel: &str) -> std::path::PathBuf {
    let direct = dir.join(rel);
    #[cfg(target_os = "macos")]
    if rel == "PingFang.ttc" && !direct.exists() {
        for base in [
            "/System/Library/AssetsV2/com_apple_MobileAsset_Font8",
            "/System/Library/AssetsV2/com_apple_MobileAsset_Font7",
            "/System/Library/AssetsV2/com_apple_MobileAsset_Font6",
        ] {
            if let Ok(entries) = std::fs::read_dir(base) {
                for entry in entries.flatten() {
                    let p = entry.path().join("AssetData/PingFang.ttc");
                    if p.is_file() {
                        return p;
                    }
                }
            }
        }
    }
    direct
}

/// The fonts for scripts outside the interface language, loaded the first time a character
/// needs one (#405): a file or video named in Bangla, Japanese, Thai... The interface language's
/// own script fonts are loaded up front ([`SCRIPT_FONTS`], kept small for #72); anything else
/// comes from the system on demand, only when text in that script is shown.
///
/// Windows and macOS: the blocks below name the system's font for each script. Linux: fontconfig
/// is asked for a font that has the character (`:charset=`), one question per Unicode block.
struct ScriptFallback;

/// One script's fonts on demand: a key, its code points, the files to try (in order).
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
struct FallbackFont {
    key: &'static str,
    ranges: &'static [(u32, u32)],
    files: &'static [&'static str],
}

/// Windows' fonts for the scripts egui's fonts and Segoe UI don't cover.
#[cfg(windows)]
const FALLBACK_FONTS: &[FallbackFont] = &[
    FallbackFont {
        key: "indic",
        ranges: &[(0x0900, 0x0DFF), (0x1C50, 0x1C7F)],
        files: &["Nirmala.ttc", "Nirmala.ttf"],
    },
    FallbackFont {
        key: "thai-lao-khmer",
        ranges: &[(0x0E00, 0x0EFF), (0x1780, 0x17FF), (0x19E0, 0x19FF)],
        files: &["LeelawUI.ttf", "leelawdb.ttf", "tahoma.ttf"],
    },
    FallbackFont {
        key: "myanmar",
        ranges: &[(0x1000, 0x109F), (0xA9E0, 0xA9FF), (0xAA70, 0xAA7F)],
        files: &["mmrtext.ttf"],
    },
    FallbackFont {
        key: "tibetan",
        ranges: &[(0x0F00, 0x0FFF)],
        files: &["himalaya.ttf"],
    },
    FallbackFont {
        key: "ethiopic",
        ranges: &[(0x1200, 0x139F), (0x2D80, 0x2DDF), (0xAB00, 0xAB2F)],
        files: &["ebrima.ttf"],
    },
    FallbackFont {
        key: "cherokee",
        ranges: &[(0x13A0, 0x13FF), (0xAB70, 0xABBF)],
        files: &["gadugi.ttf"],
    },
    FallbackFont {
        key: "hangul",
        ranges: &[
            (0x1100, 0x11FF),
            (0x3130, 0x318F),
            (0xA960, 0xA97F),
            (0xAC00, 0xD7FF),
        ],
        files: &["malgun.ttf"],
    },
    FallbackFont {
        key: "japanese",
        ranges: &[(0x3040, 0x30FF), (0x31F0, 0x31FF), (0xFF66, 0xFF9F)],
        files: &["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"],
    },
    FallbackFont {
        key: "han",
        ranges: &[
            (0x2E80, 0x2FFF),
            (0x3000, 0x303F),
            (0x3100, 0x312F),
            (0x31A0, 0x31BF),
            (0x3200, 0x33FF),
            (0x3400, 0x4DBF),
            (0x4E00, 0x9FFF),
            (0xF900, 0xFAFF),
            (0xFE30, 0xFE4F),
            (0xFF00, 0xFF65),
            (0xFFE0, 0xFFEF),
            (0x20000, 0x3FFFF),
        ],
        files: &["msyh.ttc", "msjh.ttc", "YuGothM.ttc", "msgothic.ttc"],
    },
];

/// macOS's fonts for the same scripts; Arial Unicode for what isn't listed. Compiled everywhere,
/// so the tests check it on every system.
#[cfg(target_os = "macos")]
const FALLBACK_FONTS: &[FallbackFont] = MACOS_FALLBACK_FONTS;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MACOS_FALLBACK_FONTS: &[FallbackFont] = &[
    FallbackFont {
        key: "japanese",
        ranges: &[(0x3040, 0x30FF), (0x31F0, 0x31FF), (0xFF66, 0xFF9F)],
        files: &["ヒラギノ角ゴシック W3.ttc", "Hiragino Sans GB.ttc"],
    },
    FallbackFont {
        key: "hangul",
        ranges: &[
            (0x1100, 0x11FF),
            (0x3130, 0x318F),
            (0xA960, 0xA97F),
            (0xAC00, 0xD7FF),
        ],
        files: &["AppleSDGothicNeo.ttc"],
    },
    FallbackFont {
        key: "han",
        ranges: &[
            (0x2E80, 0x303F),
            (0x3100, 0x312F),
            (0x3200, 0x4DBF),
            (0x4E00, 0x9FFF),
            (0xF900, 0xFAFF),
            (0xFE30, 0xFE4F),
            (0xFF00, 0xFFEF),
            (0x20000, 0x3FFFF),
        ],
        files: &[
            "PingFang.ttc",
            "Hiragino Sans GB.ttc",
            "STHeiti Light.ttc",
            "Supplemental/Songti.ttc",
        ],
    },
    FallbackFont {
        key: "khmer",
        ranges: &[(0x1780, 0x17FF), (0x19E0, 0x19FF)],
        files: &["Supplemental/Khmer Sangam MN.ttf"],
    },
    FallbackFont {
        key: "lao",
        ranges: &[(0x0E80, 0x0EFF)],
        files: &["Supplemental/Lao Sangam MN.ttf"],
    },
    FallbackFont {
        key: "ethiopic",
        ranges: &[(0x1200, 0x139F), (0x2D80, 0x2DDF), (0xAB00, 0xAB2F)],
        files: &["Supplemental/Kefa.ttc", "Kefa.ttc"],
    },
    // Armenian, Hebrew and the Arabic script (Persian, Urdu: #533) too: the system font (SF)
    // has none of them, and Windows' Segoe UI, which has them, isn't there. The same font an
    // Arabic or Persian interface loads ([`SCRIPT_FONTS`]).
    FallbackFont {
        key: "unicode",
        ranges: &[
            (0x0530, 0x13FF),
            (0x1800, 0x1FFF),
            (0xFB1D, 0xFDFF),
            (0xFE70, 0xFEFF),
        ],
        files: &["Supplemental/Arial Unicode.ttf"],
    },
];

/// The font in `fonts` for `c`: the first that lists its code point.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn fallback_entry(fonts: &'static [FallbackFont], c: char) -> Option<&'static FallbackFont> {
    let cp = c as u32;
    fonts
        .iter()
        .find(|f| f.ranges.iter().any(|&(a, b)| (a..=b).contains(&cp)))
}

/// The CJK script `c` belongs to, or `None`: Chinese and Japanese characters (`"han"`), kana
/// (`"japanese"`) and Hangul (`"hangul"`). The combining tone marks Japanese and Chinese put on
/// kana and letters (U+302A–302F, U+3099–309A) are `None`: a shaper must place them with their
/// base, so egui keeps them (as it keeps every multi-codepoint cluster).
#[cfg(windows)]
pub(crate) fn cjk_script(c: char) -> Option<&'static str> {
    let cp = c as u32;
    if matches!(cp, 0x302A..=0x302F | 0x3099..=0x309A) {
        return None;
    }
    if matches!(
        cp,
        0x3040..=0x30FF // hiragana, katakana
        | 0x31F0..=0x31FF // katakana phonetic extensions
        | 0xFF66..=0xFF9D // halfwidth katakana
    ) {
        return Some("japanese");
    }
    if matches!(cp, 0x3130..=0x318F | 0xAC00..=0xD7A3) {
        return Some("hangul");
    }
    if matches!(
        cp,
        0x2E80..=0x2FFF // radicals
        | 0x3000..=0x303F // CJK punctuation
        | 0x3100..=0x312F // bopomofo
        | 0x31A0..=0x31BF // bopomofo extended
        | 0x3200..=0x33FF // enclosed and compatibility characters
        | 0x3400..=0x4DBF // Han extension A
        | 0x4E00..=0x9FFF // Han
        | 0xF900..=0xFAFF // compatibility ideographs
        | 0xFE30..=0xFE4F // compatibility forms
        | 0xFF00..=0xFF65 // fullwidth forms and punctuation
        | 0xFFE0..=0xFFE6 // fullwidth signs
        | 0x20000..=0x3FFFF // Han extensions B and later, and the compatibility supplement
    ) {
        return Some("han");
    }
    None
}

/// The system font files to try for a CJK character, in order (Windows): the interface language's
/// own face first, then the fallback table's. It is the same one source the loaded script fonts
/// use ([`SCRIPT_FONTS`]), so a Chinese, Japanese or Korean interface keeps its language's glyph
/// variants. Empty for a character [`cjk_script`] leaves to egui.
#[cfg(windows)]
pub(crate) fn cjk_font_files(c: char) -> Vec<&'static str> {
    cjk_font_files_for(c, crate::i18n::code())
}

/// [`cjk_font_files`] for an interface language `code` (the production path and its tests).
#[cfg(windows)]
pub(crate) fn cjk_font_files_for(c: char, code: &str) -> Vec<&'static str> {
    let Some(script) = cjk_script(c) else {
        return Vec::new();
    };
    let own = match script {
        "han" => match code {
            "zh-hant" => "han-tc",
            "ja" => "japanese",
            "ko" => "hangul",
            _ => "han-sc",
        },
        other => other,
    };
    let mut files = Vec::new();
    let mut add = |list: &'static [&'static str]| {
        for file in list {
            if !files.contains(file) {
                files.push(*file);
            }
        }
    };
    if let Some((_, own_files, _)) = SCRIPT_FONTS.iter().find(|(key, ..)| *key == own) {
        add(own_files);
    }
    if let Some(entry) = fallback_entry(FALLBACK_FONTS, c) {
        add(entry.files);
    }
    files
}

/// The bytes of the font for `c`, loaded once per font, shared by every family that asks.
fn fallback_font(c: char) -> Option<(String, egui::FontData)> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static LOADED: Mutex<Option<HashMap<String, Option<egui::FontData>>>> = Mutex::new(None);

    #[cfg(any(windows, target_os = "macos"))]
    let (key, files): (String, Vec<std::path::PathBuf>) = {
        let font = fallback_entry(FALLBACK_FONTS, c)?;
        let dir = font_dir();
        (
            format!("fallback:{}", font.key),
            font.files.iter().map(|f| font_path(&dir, f)).collect(),
        )
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let (key, files): (String, Vec<std::path::PathBuf>) = {
        // One question per Unicode block: a script's letters don't each need an fc-match.
        let block = (c as u32) >> 8;
        (
            format!("fallback:{block:x}"),
            fc_lookup(&format!(":charset={:x}", c as u32))
                .into_iter()
                .collect(),
        )
    };

    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    let loaded = loaded.get_or_insert_with(HashMap::new);
    let font = loaded
        .entry(key.clone())
        .or_insert_with(|| {
            let bytes = files.iter().find_map(|f| std::fs::read(f).ok())?;
            let mut tweak = egui::FontTweak::default();
            tweak.set_baseline_fallback(true);
            #[cfg(windows)]
            {
                tweak.hinting_target =
                    egui::epaint::text::HintingTarget::Smooth(egui::epaint::text::SmoothHinting {
                        light: false,
                        symmetric_rendering: false,
                        preserve_linear_metrics: false,
                    });
                tweak.subpixel_binning = Some(false);
            }
            Some(egui::FontData::from_owned(bytes).tweak(tweak))
        })
        .clone()?;
    Some((key, font))
}

/// `fc-match` for one pattern, asked right now (Linux, a character no loaded font has).
#[cfg(not(any(windows, target_os = "macos")))]
fn fc_lookup(pattern: &str) -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", pattern])
        .output()
        .ok()?;
    let file = String::from_utf8(out.stdout).ok()?;
    let file = file.trim();
    (out.status.success() && !file.is_empty()).then(|| file.into())
}

impl egui::FontProvider for ScriptFallback {
    fn font_for(&self, request: &egui::FallbackRequest<'_>) -> Option<egui::FontInsert> {
        let c = request.cluster.chars().next()?;
        let (key, data) = fallback_font(c)?;
        Some(egui::FontInsert::new(&key, data, Vec::new()))
    }
}

/// Tell the text system where the scripts outside the interface language come from, once.
fn add_script_fallback(ctx: &egui::Context) {
    static ADDED: std::sync::Once = std::sync::Once::new();
    ADDED.call_once(|| ctx.add_font_provider(std::sync::Arc::new(ScriptFallback)));
}

/// The interface font: Segoe UI as IDM has it on Windows, the system's own elsewhere.
fn ui_font() -> Option<Vec<u8>> {
    #[cfg(windows)]
    return std::fs::read(font_dir().join("segoeui.ttf")).ok();
    #[cfg(target_os = "macos")]
    return ["SFNS.ttf", "Helvetica.ttc"]
        .iter()
        .find_map(|f| std::fs::read(font_dir().join(f)).ok());
    #[cfg(not(any(windows, target_os = "macos")))]
    return fc_match("sans-serif").and_then(|f| std::fs::read(f).ok());
}

/// The script fonts for `code` (every language's with `all`): (key, bytes), in fallback order.
fn script_fonts(code: &str, all: bool) -> Vec<(String, Vec<u8>)> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        let dir = font_dir();
        SCRIPT_FONTS
            .iter()
            .filter(|(_, _, langs)| all || langs.contains(&code))
            // The first file there is used (older systems name some differently); a .ttc's
            // first face is the regular one.
            .filter_map(|(key, files, _)| {
                let bytes = files
                    .iter()
                    .find_map(|f| std::fs::read(font_path(&dir, f)).ok())?;
                Some(((*key).to_owned(), bytes))
            })
            .collect()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let mut seen = std::collections::HashSet::new();
        SCRIPT_LANGS
            .iter()
            .filter(|(ours, _)| all || *ours == code)
            .filter_map(|(_, fc)| fc_match(&format!(":lang={fc}")))
            // Several languages share a font (Devanagari, the CJK collection): loaded once.
            .filter(|f| seen.insert(f.clone()))
            .filter_map(|f| {
                let bytes = std::fs::read(&f).ok()?;
                Some((format!("fc:{}", f.display()), bytes))
            })
            .collect()
    }
}

/// The font file fontconfig picks for `pattern` (one of [`fc_files`]'s).
#[cfg(not(any(windows, target_os = "macos")))]
fn fc_match(pattern: &str) -> Option<std::path::PathBuf> {
    fc_files().get(pattern).cloned()
}

/// Every font file the windows may need, as fontconfig picks it: the interface font
/// (`sans-serif`) and each script's (`:lang=…`). Each answer is an `fc-match` program; asked one
/// after the other on the window's thread, the thirty of them held up MDM's first frame for 2 s
/// on an idle computer and past the freeze report's 8 s on a busy one (#164). So they are asked
/// once, all together (a third of the time), and [`prepare_fonts`] starts that while the window
/// opens; the fonts only wait for what is still running. A font installed while MDM runs is
/// used from its next start.
#[cfg(not(any(windows, target_os = "macos")))]
fn fc_files() -> &'static std::collections::HashMap<String, std::path::PathBuf> {
    use std::process::{Command, Stdio};
    static FILES: std::sync::OnceLock<std::collections::HashMap<String, std::path::PathBuf>> =
        std::sync::OnceLock::new();
    FILES.get_or_init(|| {
        let running: Vec<_> = ["sans-serif", SYMBOL_PATTERN]
            .into_iter()
            .map(str::to_owned)
            .chain(SCRIPT_LANGS.iter().map(|(_, fc)| format!(":lang={fc}")))
            .filter_map(|pattern| {
                let child = Command::new("fc-match")
                    .args(["-f", "%{file}", &pattern])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .ok()?;
                Some((pattern, child))
            })
            .collect();
        running
            .into_iter()
            .filter_map(|(pattern, child)| {
                let out = child.wait_with_output().ok()?;
                let file = String::from_utf8(out.stdout).ok()?;
                let file = file.trim();
                (out.status.success() && !file.is_empty()).then(|| (pattern, file.into()))
            })
            .collect()
    })
}

/// Start looking up the system's fonts while the window opens (Linux: [`fc_files`]); Windows and
/// macOS read theirs from known files, quickly, when the fonts load.
pub fn prepare_fonts() {
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = std::thread::Builder::new().name("fonts".into()).spawn(|| {
        fc_files();
    });
}

/// The script fonts loaded now: the interface language's, or all of them.
static LOADED: std::sync::Mutex<Option<(String, bool)>> = std::sync::Mutex::new(None);

/// Load the fonts the interface language needs (after it changed).
pub fn language_fonts(ctx: &egui::Context) {
    let all = LOADED.lock().unwrap().as_ref().is_some_and(|l| l.1);
    load_fonts(ctx, all);
}

/// Load every script font, for a list of all languages by their own names, until
/// [`interface_fonts`] lets them go.
pub fn all_scripts(ctx: &egui::Context) {
    load_fonts(ctx, true);
}

/// Back to the interface language's script fonts only, once no list of every language shows
/// (#72: CJK fonts alone are 13–20 MB each; the language picker had kept them all loaded).
pub fn interface_fonts(ctx: &egui::Context) {
    load_fonts(ctx, false);
}

fn load_fonts(ctx: &egui::Context, all: bool) {
    #[cfg(windows)]
    classic_text(ctx);
    let code = crate::i18n::code().to_owned();
    let mut loaded = LOADED.lock().unwrap();
    // Only the script fonts depend on the language: rebuild when that set changes.
    let needs = |code: &str| -> Vec<&str> {
        #[cfg(any(windows, target_os = "macos"))]
        return SCRIPT_FONTS
            .iter()
            .filter(|(_, _, langs)| all || langs.contains(&code))
            .map(|(key, ..)| *key)
            .collect();
        #[cfg(not(any(windows, target_os = "macos")))]
        return SCRIPT_LANGS
            .iter()
            .filter(|(ours, _)| all || *ours == code)
            .map(|(ours, _)| *ours)
            .collect();
    };
    if let Some((old, old_all)) = loaded.as_ref()
        && *old_all == all
        && needs(old) == needs(&code)
    {
        *loaded = Some((code, all));
        return;
    }
    fonts(ctx, &code, all);
    *loaded = Some((code, all));
}

/// IDM draws with Segoe UI 9 pt (12 px at 100 %): the system's own copy, nothing bundled
/// (elsewhere the system's interface font, [`ui_font`]; else egui's default font). Every family
/// falls back to it (Tahoma has no Armenian or Georgian), then to the script fonts for `code`
/// (all with `all`).
fn fonts(ctx: &egui::Context, code: &str, all: bool) {
    let mut defs = egui::FontDefinitions::default();
    let ui = ui_font();
    if let Some(bytes) = &ui {
        defs.font_data.insert(
            "segoe-ui".into(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes.clone())),
        );
        for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            defs.families
                .entry(fam)
                .or_default()
                .insert(0, "segoe-ui".into());
        }
    }
    #[cfg(windows)]
    crate::winui::install_fonts(&mut defs, Some(&font_dir()));
    #[cfg(not(windows))]
    crate::winui::install_fonts(&mut defs, None);
    // A fallback's glyphs sit on the baseline of the family's first font (Japanese next to
    // Latin digits, Georgian in a Tahoma line), not centred in its row.
    let on_baseline = |bytes: Vec<u8>| {
        let mut tweak = egui::FontTweak::default();
        tweak.set_baseline_fallback(true);
        std::sync::Arc::new(egui::FontData::from_owned(bytes).tweak(tweak))
    };
    let mut fallbacks: Vec<String> = Vec::new();
    if let Some(bytes) = ui {
        defs.font_data
            .insert("segoe-ui-fallback".into(), on_baseline(bytes));
        fallbacks.push("segoe-ui-fallback".into());
    }
    for (key, bytes) in script_fonts(code, all) {
        defs.font_data.insert(key.clone(), on_baseline(bytes));
        fallbacks.push(key);
    }
    for (k, bytes) in symbol_fonts().into_iter().enumerate() {
        let key = format!("symbols-{k}");
        defs.font_data.insert(key.clone(), on_baseline(bytes));
        fallbacks.push(key);
    }
    for list in defs.families.values_mut() {
        for f in &fallbacks {
            // Segoe UI already leads the two built-in families (as itself, not a fallback).
            if f == "segoe-ui-fallback" && list.first().is_some_and(|l| l == "segoe-ui") {
                continue;
            }
            if !list.contains(f) {
                list.push(f.clone());
            }
        }
    }
    #[cfg(windows)]
    sharp_text(&mut defs);
    ctx.set_fonts(defs);
    text_styles(ctx);
}

/// Latin, Greek and Cyrillic text drawn the way Windows' classic controls draw it (#262, #323,
/// #334), by DirectWrite ([`crate::win32::text_rasterizer`]): the fonts' shapes and, with them,
/// whole-pixel widths, so a letter's width and the place it is drawn for finally agree. That
/// disagreement is what glued some letters together and pushed others apart. Set again when the
/// interface's direction changes: a right-to-left line needs its brackets mirrored by the shaper.
#[cfg(windows)]
fn classic_text(ctx: &egui::Context) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static DRAWN: AtomicU64 = AtomicU64::new(u64::MAX);
    let rtl = crate::i18n::rtl();
    let smooth = SMOOTH_TEXT.load(Ordering::Relaxed);
    // A CJK face depends on the interface language (its glyph variants), and the glyph atlas
    // keeps what was rasterized: a change of CJK language installs the rasterizer again.
    let lang = match crate::i18n::code() {
        "zh-hans" => 1,
        "zh-hant" => 2,
        "ja" => 3,
        "ko" => 4,
        _ => 0,
    };
    let now = u64::from(rtl) | (u64::from(smooth) << 1) | (lang << 2);
    if DRAWN.swap(now, Ordering::Relaxed) != now {
        ctx.set_glyph_rasterizers(
            crate::win32::text_rasterizer(font_dir(), rtl, smooth)
                .into_iter()
                .collect(),
        );
    }
}

/// View → Text: smooth (DirectWrite's natural look) or classic (hinted whole-pixel text, as
/// Windows' old controls, the default). Only Windows has the choice: elsewhere egui draws the text.
static SMOOTH_TEXT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The setting's value, before the first frame (it is read at start).
pub fn init_smooth_text(on: bool) {
    SMOOTH_TEXT.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Draw the text smooth (`true`) or classic from now on, in every window. Windows only: View →
/// Text exists only there, where DirectWrite draws the text; elsewhere egui draws it.
#[cfg(windows)]
pub fn set_smooth_text(ctx: &egui::Context, on: bool) {
    init_smooth_text(on);
    classic_text(ctx);
}

/// The text [`classic_text`] leaves to egui (Arabic, Hebrew, Indic, Thai, CJK; marks clustered
/// with their letter), fitted to whole pixels (#262) with hinting for egui's anti-aliased output.
/// Mono hinting targets black-and-white bitmaps: at small sizes it can distort curves and make
/// diagonals jagged. Smooth hinting keeps those outlines suited to coverage rendering, while
/// allowing horizontal grid-fitting so upright strokes stay sharp. Keep whole-pixel glyph origins
/// and the shaper's advances, so this changes neither layout nor the interface size. Linux and
/// macOS keep egui's defaults.
#[cfg(windows)]
fn sharp_text(defs: &mut egui::FontDefinitions) {
    for data in defs.font_data.values_mut() {
        let data = std::sync::Arc::make_mut(data);
        data.tweak.hinting_target =
            egui::epaint::text::HintingTarget::Smooth(egui::epaint::text::SmoothHinting {
                light: false,
                symmetric_rendering: false,
                preserve_linear_metrics: false,
            });
        data.tweak.subpixel_binning = Some(false);
    }
}

/// egui's text sizes from the theme (tooltips, multi-line boxes).
fn text_styles(ctx: &egui::Context) {
    let m = metrics();
    ctx.all_styles_mut(|s| {
        use egui::{FontId, TextStyle};
        s.text_styles = [
            (
                TextStyle::Small,
                FontId::proportional(font_size(m.font_small)),
            ),
            (
                TextStyle::Body,
                FontId::proportional(font_size(m.font_body)),
            ),
            (
                TextStyle::Button,
                FontId::proportional(font_size(m.font_body)),
            ),
            (
                TextStyle::Monospace,
                FontId::monospace(font_size(m.font_body)),
            ),
            (
                TextStyle::Heading,
                FontId::proportional(font_size(m.font_heading)),
            ),
        ]
        .into();
    });
}

/// The size a role is drawn at: in a Chinese, Japanese or Korean interface, no smaller than the
/// theme's `font.cjk` (their glyphs are denser than Latin's at the same size, #837). Other
/// languages keep the role's size exactly.
pub fn font_size(size: f32) -> f32 {
    font_size_for(crate::i18n::code(), size, metrics().font_cjk)
}

/// [`font_size`] for an interface language `code` and a theme's CJK floor (also the production
/// path, so the test covers what the UI calls).
fn font_size_for(code: &str, size: f32, cjk: f32) -> f32 {
    if crate::i18n::cjk_code(code) {
        size.max(cjk)
    } else {
        size
    }
}

pub fn apply(ctx: &egui::Context) {
    add_script_fallback(ctx);
    load_fonts(ctx, false);
    // A press held for a while is still a click when let go over the control, as in Windows
    // (egui gives it up after 0.8 s).
    ctx.options_mut(|o| {
        o.input_options.max_click_duration = f64::INFINITY;
        // Ctrl + / Ctrl − / Ctrl 0 step through View → Interface size and are saved (scale.rs),
        // not egui's own 10 % steps.
        o.zoom_with_keyboard = false;
        // Every window's theme (its caption) is MDM's to give: the main window's in
        // `apply_mode`, each dialog's in `dialogs::keep_theme`, the caption the theme's surface.
        // egui's own would set the mode's instead, behind them, and the caption would have to
        // be checked with DWM again and again (#633).
        o.sync_window_theme = false;
    });
    egui_extras::install_image_loaders(ctx);
    apply_theme(ctx);
}

/// egui's own visuals from the active theme: each variant into its egui theme (the mode
/// switches between them), so tooltips and a multi-line box's scroll bar follow the theme too.
pub fn apply_theme(ctx: &egui::Context) {
    let theme = current();
    let m = &theme.metrics;
    let radius = |r: f32| CornerRadius::same(r.round().clamp(0.0, 255.0) as u8);
    let visuals = |base: Visuals, c: &Palette| {
        let mut v = base;
        v.panel_fill = c.egui_panel;
        v.window_fill = c.egui_window;
        v.extreme_bg_color = c.egui_extreme;
        v.faint_bg_color = c.egui_faint;
        v.override_text_color = Some(c.egui_text);
        v.selection.bg_fill = c.selection;
        v.selection.stroke = Stroke::new(1.0, c.egui_selection_text);
        // Square-ish Windows controls rather than egui's rounded ones.
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = radius(m.widget_radius);
        }
        v.widgets.inactive.bg_fill = c.egui_widget;
        v.widgets.inactive.weak_bg_fill = c.egui_widget;
        // Transparent: egui's own (light widgets have none).
        if c.egui_widget_border != egui::Color32::TRANSPARENT {
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, c.egui_widget_border);
        }
        v.window_corner_radius = CornerRadius::same(0);
        // Tooltips as Windows 11's: rounded corners, a hairline border, roomy padding.
        v.menu_corner_radius = radius(m.popup_radius);
        v.window_stroke = Stroke::new(1.0, c.egui_window_border);
        v
    };
    ctx.set_visuals_of(egui::Theme::Dark, visuals(Visuals::dark(), &theme.dark));
    ctx.set_visuals_of(egui::Theme::Light, visuals(Visuals::light(), &theme.light));
    let margin = |v: f32| v.round().clamp(0.0, 127.0) as i8;
    ctx.all_styles_mut(|s| {
        s.spacing.tooltip_width = m.tooltip_width;
        s.spacing.menu_margin = egui::Margin {
            left: margin(m.tooltip_margin_x),
            right: margin(m.tooltip_margin_x),
            top: margin(m.tooltip_margin_top),
            bottom: margin(m.tooltip_margin_bottom),
        };
    });
    text_styles(ctx);
}

#[cfg(test)]
mod mode_tests {
    use super::{font_size_for, wanted};

    /// #60: Always dark wins whatever the system says; otherwise Dark Mode support follows it.
    #[test]
    fn always_dark_wins_else_the_system_decides() {
        for system in [false, true] {
            for support in [false, true] {
                assert!(wanted(support, true, system), "always dark");
            }
            assert_eq!(wanted(true, false, system), system, "following the system");
            assert!(!wanted(false, false, system), "support off: light");
        }
    }

    /// #837: Chinese, Japanese and Korean text keeps the theme's floor; English and the other
    /// languages keep their measured size. A role already larger keeps it.
    #[test]
    fn cjk_text_keeps_the_themes_floor() {
        assert_eq!(crate::palette::Metrics::CLASSIC.font_cjk, 12.0);
        for code in ["zh-hans", "zh-hant", "ja", "ko"] {
            assert_eq!(font_size_for(code, 11.0, 12.0), 12.0, "{code}");
            assert_eq!(font_size_for(code, 13.0, 12.0), 13.0, "{code}");
        }
        for code in ["en", "de", "ar", "ru", "th"] {
            assert_eq!(font_size_for(code, 11.0, 12.0), 11.0, "{code}");
        }
        // A theme that asks for a larger floor gets it; one that asks for less than the role
        // doesn't shrink the role.
        assert_eq!(font_size_for("zh-hans", 11.0, 14.0), 14.0);
        assert_eq!(font_size_for("zh-hans", 16.0, 14.0), 16.0);
    }
}

/// Every mark a window draws (punctuation, arrows, ✔, quotation marks…: what isn't a letter or a
/// digit) has a glyph in every language's font chain, so none comes out as an empty box (#185:
/// "✔ Connected" in the browser offer window; "→" on Linux).
#[cfg(test)]
mod glyph_tests {

    /// #405: a script the interface language doesn't use: Bangla in a file name, Japanese in a
    /// playlist, under an English interface. Before: nothing loaded it, so boxes.
    #[cfg(windows)]
    #[test]
    fn text_in_other_scripts_gets_its_font_on_demand() {
        use egui::epaint::text::{Fonts, TextOptions};
        use egui::{FontFamily, FontId};
        let mut fonts = Fonts::new(TextOptions::default(), egui::FontDefinitions::default())
            .with_font_providers(vec![std::sync::Arc::new(super::ScriptFallback)]);
        let id = FontId::new(13.0, FontFamily::Proportional);
        for text in [
            "\u{09AC}\u{09BE}\u{0982}\u{09B2}\u{09BE}", // Bangla
            "\u{65E5}\u{672C}\u{8A9E}\u{306E}\u{30BF}\u{30A4}\u{30C8}\u{30EB}", // Japanese
            "\u{D55C}\u{AD6D}\u{C5B4}",                 // Korean
            "\u{0E44}\u{0E17}\u{0E22}",                 // Thai
            "\u{4E2D}\u{6587}",                         // Chinese
        ] {
            assert!(fonts.has_glyphs(&id, text), "no font for {text}");
        }
    }

    /// Names in the Arabic script (a Persian file name, #533), Hebrew and Armenian, under an
    /// English interface on macOS: its system font has none of them, so the fallback must name a
    /// font for every letter. Before, macOS's list started at Devanagari, and they were boxes.
    /// Checked on every system (the table is data); the font itself in the macOS test below.
    const OTHER_SCRIPTS: [&str; 5] = [
        "\u{0641}\u{0627}\u{06CC}\u{0644} \u{067E}\u{0698}\u{0648}\u{0647}\u{0634}\u{06CC} \u{06F1}\u{06F4}\u{06F0}\u{06F3}", // Persian, with Persian digits
        "\u{06AF}\u{0632}\u{0627}\u{0631}\u{0634} \u{0686}\u{06A9}", // Persian
        "\u{0645}\u{0644}\u{0641}",                                  // Arabic
        "\u{05E7}\u{05D5}\u{05D1}\u{05E5}",                          // Hebrew
        "\u{0586}\u{0561}\u{0575}\u{056C}",                          // Armenian
    ];

    #[test]
    fn macos_names_a_font_for_the_arabic_script() {
        for text in OTHER_SCRIPTS {
            for c in text.chars().filter(|c| !c.is_whitespace()) {
                let font = super::fallback_entry(super::MACOS_FALLBACK_FONTS, c)
                    .unwrap_or_else(|| panic!("no macOS font for U+{:04X} in {text}", c as u32));
                // The file macOS ships (/System/Library/Fonts/Supplemental), which an Arabic or
                // Persian interface already loads.
                assert!(
                    font.files.contains(&"Supplemental/Arial Unicode.ttf"),
                    "U+{:04X}: {:?}",
                    c as u32,
                    font.files
                );
            }
        }
    }

    /// The same names drawn with macOS's own fonts: the fallback finds a file that is there and
    /// has the letters.
    #[cfg(target_os = "macos")]
    #[test]
    fn persian_names_get_a_font_on_macos() {
        use egui::epaint::text::{Fonts, TextOptions};
        use egui::{FontFamily, FontId};
        let mut fonts = Fonts::new(TextOptions::default(), egui::FontDefinitions::default())
            .with_font_providers(vec![std::sync::Arc::new(super::ScriptFallback)]);
        let id = FontId::new(13.0, FontFamily::Proportional);
        for text in OTHER_SCRIPTS {
            assert!(fonts.has_glyphs(&id, text), "no font for {text}");
        }
    }
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;
    use unicode_segmentation::UnicodeSegmentation;

    const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

    /// Drawn as nothing on purpose: bidi controls and isolates, joiners, the soft hyphen, the
    /// BOM (craft-i18n's isolates around data, the translations' own marks).
    fn invisible(c: char) -> bool {
        matches!(c, '\u{00ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2069}' | '\u{feff}' | '\u{fe00}'..='\u{fe0f}')
    }

    /// The marks in `text`: each grapheme's first character that is neither a letter, a digit,
    /// a space nor invisible (a grapheme's other characters are its accents and joiners).
    fn marks(text: &str, out: &mut BTreeSet<char>) {
        for g in text.graphemes(true) {
            if let Some(c) = g.chars().next()
                && !c.is_alphanumeric()
                && !c.is_whitespace()
                && !c.is_control()
                && !invisible(c)
            {
                out.insert(c);
            }
        }
    }

    /// A catalog's texts: the translations (and the English sources, which a missing row shows).
    fn catalog(code: &str) -> String {
        let file = Path::new(ROOT).join("i18n").join(format!("{code}.tsv"));
        let text = std::fs::read_to_string(&file).unwrap();
        text.lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with('@'))
            .flat_map(|l| l.split('\t').skip(1))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The marks the code draws itself, beside the catalogs' ("\u{2714} {}", "→"): every
    /// string literal in the app's sources, escapes read.
    fn literals() -> BTreeSet<char> {
        let mut out = BTreeSet::new();
        let dir = Path::new(ROOT).join("crates/app/src");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            // The tests' own data (other scripts, emoji in file names) is not drawn.
            let src = src.split("#[cfg(test)]").next().unwrap_or_default();
            for lit in src.split('"').skip(1).step_by(2) {
                let mut s = String::new();
                let mut rest = lit;
                while let Some(at) = rest.find("\\u{") {
                    s.push_str(&rest[..at]);
                    let end = rest[at..].find('}').map_or(rest.len(), |e| at + e);
                    if let Some(c) = u32::from_str_radix(&rest[at + 3..end], 16)
                        .ok()
                        .and_then(char::from_u32)
                    {
                        s.push(c);
                    }
                    rest = &rest[(end + 1).min(rest.len())..];
                }
                s.push_str(rest);
                // Only what isn't plain ASCII: every font has that.
                let mut found = BTreeSet::new();
                marks(&s, &mut found);
                out.extend(found.into_iter().filter(|c| !c.is_ascii()));
            }
        }
        out
    }

    fn languages() -> Vec<String> {
        let file = Path::new(ROOT).join("i18n/languages.tsv");
        std::fs::read_to_string(file)
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .filter_map(|l| l.split('\t').next().map(str::to_owned))
            .collect()
    }

    #[test]
    fn every_language_draws_its_marks() {
        let code_marks = literals();
        let families = [
            egui::FontFamily::Name("tahoma".into()),
            egui::FontFamily::Name("tahoma-bold".into()),
            egui::FontFamily::Proportional,
        ];
        let mut missing: BTreeMap<String, BTreeSet<char>> = BTreeMap::new();
        let mut skipped = Vec::new();
        for code in languages() {
            let text = catalog(&code);
            let mut needed = code_marks.clone();
            marks(&text, &mut needed);
            let ctx = egui::Context::default();
            super::fonts(&ctx, &code, false);
            let mut out = ctx.run_ui(Default::default(), |_| {});
            out.textures_delta.clear();
            // A computer without the language's script font draws none of it (a CI image without
            // CJK fonts): nothing to check there.
            let letters: Vec<char> = text
                .chars()
                .filter(|c| c.is_alphabetic() && !c.is_ascii())
                .take(200)
                .collect();
            let has_script = ctx.fonts_mut(|f| {
                let id = egui::FontId::new(11.0, families[0].clone());
                letters.is_empty()
                    || letters.iter().filter(|c| f.has_glyph(&id, **c)).count() * 10
                        >= letters.len() * 9
            });
            if !has_script {
                skipped.push(code);
                continue;
            }
            for family in &families {
                let id = egui::FontId::new(11.0, family.clone());
                let lacks: Vec<char> = ctx.fonts_mut(|f| {
                    needed
                        .iter()
                        .copied()
                        .filter(|c| !f.has_glyph(&id, *c))
                        .collect()
                });
                if !lacks.is_empty() {
                    missing
                        .entry(format!("{code} ({family:?})"))
                        .or_default()
                        .extend(lacks);
                }
            }
        }
        if !skipped.is_empty() {
            eprintln!(
                "no font for these languages' script on this computer, not checked: {skipped:?}"
            );
        }
        let report: Vec<String> = missing
            .iter()
            .map(|(k, v)| {
                format!(
                    "{k}: {}",
                    v.iter()
                        .map(|c| format!("{c} U+{:04X}", *c as u32))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect();
        assert!(
            report.is_empty(),
            "marks without a glyph:\n{}",
            report.join("\n")
        );
    }
}

#[cfg(test)]
mod status_width_tests {
    /// #736: the list's Status cell while a torrent's files or a finished file are checked or a
    /// video joined, a short word and how far ("Checking 45%", "Merging 63%"), fits its column
    /// at the default width in English, German, French, Russian and Arabic, at 100 % and at
    /// 150 % (View → Interface size), the widest percentage ("100 %") and "..." before it is
    /// known included. "63% merged" was cut to "63% me..." at 65 px, and a torrent's "Wird
    /// überprüft 45 %" in German.
    ///
    /// In the list's font as this system draws it: Segoe UI on Windows, the system font on
    /// macOS, fontconfig's interface font on Linux (DejaVu Sans on a bare Ubuntu, where German
    /// needs 147 px). The default grows with the font there ([`crate::list::status_width`]); on
    /// Windows it stays 132 px, so the list's other columns keep their room.
    #[test]
    fn finishing_fits_the_status_column() {
        let i18n = crate::i18n::i18n();
        #[cfg(not(any(windows, target_os = "macos")))]
        println!("interface font: {:?}", super::fc_match("sans-serif"));
        let mut widest = (0.0f32, String::new());
        for code in ["en", "de", "fr", "ru", "ar"] {
            let lang = i18n.from_code(code).expect("a catalog");
            let full = craft_i18n::Format::from_tag(code).percent(1.0, 0);
            for zoom in [1.0, 1.5] {
                let ctx = egui::Context::default();
                ctx.set_pixels_per_point(zoom);
                super::fonts(&ctx, code, false);
                let mut out = ctx.run_ui(Default::default(), |_| {});
                out.textures_delta.clear();
                let font = crate::winui::tahoma();
                let measure = |text: &str| {
                    ctx.fonts_mut(|f| {
                        f.layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
                            .size()
                            .x
                    })
                };
                let column = crate::list::status_width(code, &full, measure);
                let room = column - crate::list::CELL_PAD;
                println!("{code} at {zoom}: Status {column} px");
                #[cfg(windows)]
                assert_eq!(
                    column,
                    crate::list::DEFAULT_WIDTHS[crate::list::STATUS],
                    "{code} at {zoom}: Segoe UI fits in the default"
                );
                for source in ["Checking {percent}", "Merging {percent}"] {
                    for percent in [full.as_str(), "100%", "..."] {
                        let text = i18n.t(lang, source).replace("{percent}", percent);
                        let width = measure(&text);
                        if width > widest.0 {
                            widest = (width, format!("{code} at {zoom}: {text}"));
                        }
                        assert!(
                            width <= room,
                            "{code} at {zoom}: {text}: {width} px, {room} px of room"
                        );
                    }
                }
            }
        }
        println!("widest: {} px, {}", widest.0, widest.1);
    }
}
