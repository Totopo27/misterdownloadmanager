//! Theme files (#45): TOML or JSON in the data folder's `themes` folder, read into a
//! [`Theme`](crate::palette::Theme). A file names only what it changes; the rest comes from the
//! theme it `inherits` (a built-in's id or another file's name; the classic look by default).
//!
//! ```toml
//! name = "My theme"
//! inherits = "classic"
//! icons = "mono"            # color | mono | accent
//!
//! [sizes]
//! radius = 4
//!
//! [colors]                  # both variants
//! accent = "#2f7d32"
//!
//! [light]                   # the light variant only
//! window = "#f3f3f3"
//!
//! [dark]                    # the dark variant only
//! window = "#202020"
//! "segments.done" = "#3fa864"   # one colour, or a list of 13 (top to bottom)
//! ```
//!
//! Unknown keys and values that can't be read are skipped and reported as warnings, so a file
//! written for a newer MDM still loads; a file that isn't TOML or JSON is an error.
//!
//! A Base16 scheme (`.yaml`, `.yml`) and pywal's `colors.json` are themes too: `crate::schemes`
//! reads them into the same value tree, on the classic look. pywal's own file, where `wal`
//! writes it, is the theme `pywal`, read again when `wal` changes it ([`following_pywal`]).
//!
//! The folder and pywal's file are looked at by `lookwatch`'s thread, never the window's (#719).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::palette::{self, IconStyle, Palette, Theme};

/// A theme read from a file, with what in it was skipped.
#[derive(Debug)]
pub struct Loaded {
    pub theme: Theme,
    pub warnings: Vec<String>,
}

/// Where theme files are: the data folder's `themes` (portable copies' next to the program,
/// `MDM_DATA_DIR`'s in tests). None when there is no data folder (built-ins only).
pub fn dir() -> Option<PathBuf> {
    mdm_ipc::data_dir().map(|d| d.join("themes"))
}

/// A theme the user can pick: a built-in, or a file in [`dir`].
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    /// The file, for a theme from one.
    pub path: Option<PathBuf>,
}

/// The built-in themes, then pywal's colours when `wal` has written them, then the theme files
/// in [`dir`] (by name), each once: a file named as a built-in is skipped (the built-in keeps
/// its id). It reads every file of the folder: off the window's thread but for an action of the
/// user's (View → Theme → Add a theme file).
pub fn list() -> Vec<Entry> {
    list_in(dir().as_deref(), pywal_file())
}

/// [`list`] of the theme files in `dir`, with pywal's colours from `wal` when it is a file.
pub fn list_in(dir: Option<&Path>, wal: Option<PathBuf>) -> Vec<Entry> {
    let mut out: Vec<Entry> = palette::builtins()
        .iter()
        .map(|t| Entry {
            id: t.id.to_string(),
            name: t.name.to_string(),
            path: None,
        })
        .collect();
    let mut files: Vec<(String, PathBuf)> = dir
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            // A theme in a folder of its own (`<id>/theme.toml`, with its icons beside it).
            if path.is_dir() {
                let file = folder_file(&path)?;
                return Some((path.file_name()?.to_str()?.to_owned(), file));
            }
            let id = theme_id(&path)?;
            Some((id, path))
        })
        .collect();
    files.sort();
    // pywal's colours where `wal` keeps them, unless a file of the folder takes the name.
    if let Some(wal) = wal.filter(|p| p.is_file())
        && !files.iter().any(|(id, _)| id == PYWAL)
    {
        out.push(Entry {
            id: PYWAL.into(),
            name: PYWAL.into(),
            path: Some(wal),
        });
    }
    for (id, path) in files {
        // Built-ins keep their ids (Native too, which is made, not read).
        if out.iter().any(|e| e.id == id) || id == "native" {
            continue;
        }
        // The name the file gives itself, else its file name.
        let name = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| read_value(&path, &t).ok())
            .and_then(|(v, _)| v.get("name").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| id.clone());
        out.push(Entry {
            id,
            name,
            path: Some(path),
        });
    }
    out
}

/// When a themes folder last changed (a file added, removed or renamed); None without one.
pub fn folder_stamp(dir: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(dir).ok()?.modified().ok()
}

/// A theme folder's file: `theme.toml` or `theme.json`.
fn folder_file(dir: &Path) -> Option<PathBuf> {
    ["theme.toml", "theme.json"]
        .iter()
        .map(|f| dir.join(f))
        .find(|p| p.is_file())
}

/// The file kinds a theme can be: MDM's own (TOML, JSON, pywal's JSON too) and Base16's YAML.
const EXTENSIONS: [&str; 4] = ["toml", "json", "yaml", "yml"];

/// A theme file's id: its name without its extension ([`EXTENSIONS`]); None for any other file.
fn theme_id(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    Some(path.file_stem()?.to_str()?.to_owned())
}

/// The theme of pywal's own colours (`wal`'s `colors.json`).
pub const PYWAL: &str = "pywal";

/// Where pywal writes its colours, as pywal itself finds its cache: `$PYWAL_CACHE_DIR`, else
/// `$XDG_CACHE_HOME/wal`, else `.cache/wal` in the home folder (`$HOME`, else the user's
/// profile folder: `%USERPROFILE%\.cache\wal` on Windows, where winwal writes it too).
pub fn pywal_file() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let dir = var("PYWAL_CACHE_DIR")
        .or_else(|| var("XDG_CACHE_HOME").map(|c| c.join("wal")))
        .or_else(|| {
            var("HOME")
                .or_else(dirs::home_dir)
                .map(|h| h.join(".cache").join("wal"))
        })?;
    Some(dir.join("colors.json"))
}

/// The file's time and size: what says it was written again.
pub type Stamp = (Option<std::time::SystemTime>, u64);

/// pywal's file as a theme was read from it, and its stamp then: the file to follow.
pub type Followed = (PathBuf, Stamp);

/// pywal's file as last read for the active theme, with its stamp then: None when the active
/// theme isn't made from it.
static PYWAL_READ: std::sync::Mutex<Option<Followed>> = std::sync::Mutex::new(None);

/// A file's [`Stamp`] (a look at its details on the disk).
pub fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok();
    (
        meta.as_ref().and_then(|m| m.modified().ok()),
        meta.map_or(0, |m| m.len()),
    )
}

/// The active theme stops following pywal's file (a theme is being chosen).
pub fn forget_pywal() {
    *PYWAL_READ.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// pywal's file taken as seen at `written`: a file that didn't read is read again only once it
/// is written again.
pub fn pywal_seen_as(written: Stamp) {
    if let Some((_, seen)) = PYWAL_READ
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        *seen = written;
    }
}

/// The pywal file the active theme is made from, with its stamp when it was read; None when the
/// theme isn't made from it. `lookwatch`'s thread compares it with the file's stamp now, and
/// reads the theme again when `wal` has written it since (its time or size changed).
pub fn following_pywal() -> Option<Followed> {
    PYWAL_READ.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The active theme follows this pywal file from now on (None: none), as [`read`] found it.
pub fn follow_pywal(followed: Option<Followed>) {
    *PYWAL_READ.lock().unwrap_or_else(|e| e.into_inner()) = followed;
}

/// The theme with this id (a built-in, or the file of that name in [`dir`]) or in the theme
/// file at this path (the themes it inherits from beside it); with pywal's file when the theme
/// is made from it, to follow ([`follow_pywal`]).
pub fn read(id: &str) -> (Result<Loaded, String>, Option<Followed>) {
    let mut followed = None;
    let path = Path::new(id);
    let loaded = if path.is_file() {
        match theme_id(path) {
            Some(id) => load_in(
                path.parent().unwrap_or(Path::new(".")),
                &id,
                0,
                &mut followed,
            ),
            None => Err(format!("{}: not a .toml or .json file", path.display())),
        }
    } else {
        load_in(&dir().unwrap_or_default(), id, 0, &mut followed)
    };
    (loaded, followed)
}

/// The theme `id` from `folder`, `depth` themes down an `inherits` chain; pywal's file into
/// `followed` when one of them is read from it.
fn load_in(
    folder: &Path,
    id: &str,
    depth: usize,
    followed: &mut Option<Followed>,
) -> Result<Loaded, String> {
    if let Some(t) = palette::builtins().into_iter().find(|t| t.id == id) {
        return Ok(Loaded {
            theme: t.clone(),
            warnings: Vec::new(),
        });
    }
    if depth > 8 {
        return Err(format!("{id}: too many themes inherit one another"));
    }
    let own = folder.join(id);
    let found = EXTENSIONS
        .iter()
        .map(|ext| folder.join(format!("{id}.{ext}")))
        .find(|p| p.is_file())
        .or_else(|| folder_file(&own));
    // pywal's colours where `wal` writes them, when no file in the folder has the name.
    let wal = if found.is_none() && id == PYWAL {
        pywal_file().filter(|p| p.is_file())
    } else {
        None
    };
    let path = found
        .or_else(|| wal.clone())
        .ok_or_else(|| format!("{id}: no such theme"))?;
    // Its stamp from before it is read: a write while it was read is seen at the next check.
    let before = wal.as_deref().map(stamp);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (value, mut warnings) = read_value(&path, &text)?;
    let mut loaded = parse(id, &value, |base| {
        load_in(folder, base, depth + 1, followed)
    })?;
    warnings.append(&mut loaded.warnings);
    loaded.warnings = warnings;
    // Followed from now on: read again when `wal` writes it.
    if let (Some(wal), Some(seen)) = (wal, before) {
        *followed = Some((wal, seen));
    }
    // A theme folder's own icons (`<id>/icons/16`, `24`), unless the file names a set.
    let icons = own.join("icons");
    if value.get("icon-set").is_none() && icons.is_dir() {
        let mono = value
            .get("icon-mono")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if crate::themeicons::load_folder(id, &icons, mono) {
            loaded.theme.icon_set = Some(id.to_owned().into());
        } else {
            loaded
                .warnings
                .push("icons: no icon MDM knows in the folder".into());
        }
    }
    Ok(loaded)
}

/// A file's contents as one value tree: TOML, JSON for a `.json` file (pywal's colours made a
/// theme's), a Base16 scheme made a theme's for a `.yaml` or `.yml` one; with what reading a
/// scheme changed (its contrast).
fn read_value(path: &Path, text: &str) -> Result<(Value, Vec<String>), String> {
    let at = |e: String| format!("{}: {e}", path.display());
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "json" => {
            let v: Value = serde_json::from_str(text).map_err(|e| at(e.to_string()))?;
            if crate::schemes::is_pywal(&v) {
                crate::schemes::pywal_theme(&v).map_err(at)
            } else {
                Ok((v, Vec::new()))
            }
        }
        "yaml" | "yml" => {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            crate::schemes::base16_theme(text, stem).map_err(at)
        }
        _ => toml::from_str(text)
            .map(|v| (v, Vec::new()))
            .map_err(|e| at(e.to_string())),
    }
}

/// View → Theme → Add a theme file (#45): a theme file or a colour scheme copied into [`dir`]
/// (made the first time), under its own file name, or with a number added when another theme
/// has that name; the same file already there is used as it is. The copy must load (with the
/// themes it inherits, beside it), or it is taken out again. Returns the theme's id.
pub fn add(source: &Path) -> Result<String, String> {
    let folder = dir().ok_or("MDM has no data folder")?;
    add_in(&folder, source)
}

fn add_in(folder: &Path, source: &Path) -> Result<String, String> {
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|e| EXTENSIONS.contains(&e.as_str()))
        .ok_or_else(|| {
            format!(
                "{}: not a theme file (.toml, .json, .yaml, .yml)",
                source.display()
            )
        })?;
    let bytes = std::fs::read(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    // Read before it is copied: a file that isn't a theme stays out of the folder.
    let (value, _) = read_value(source, &text)?;
    let raw: Option<Value> = (ext == "json")
        .then(|| serde_json::from_str(&text).ok())
        .flatten();
    // pywal's file is always `colors.json`: its copy is named after the wallpaper.
    let stem = match raw.filter(crate::schemes::is_pywal) {
        Some(_) => file_name_of(value.get("name").and_then(Value::as_str).unwrap_or("")),
        None => source
            .file_stem()
            .and_then(|s| s.to_str())
            .map(file_name_of)
            .unwrap_or_default(),
    };
    let stem = if stem.is_empty() {
        "theme".to_owned()
    } else {
        stem
    };
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    // The built-ins' ids, Native's and pywal's are never a file's (the file would be passed over).
    let reserved =
        |id: &str| palette::builtins().iter().any(|t| t.id == id) || id == "native" || id == PYWAL;
    let taken = |id: &str| {
        folder.join(id).exists()
            || EXTENSIONS
                .iter()
                .any(|e| folder.join(format!("{id}.{e}")).exists())
    };
    let mut id = stem.clone();
    let mut n = 1;
    loop {
        if !reserved(&id) {
            if std::fs::read(folder.join(format!("{id}.{ext}"))).is_ok_and(|b| b == bytes) {
                return Ok(id);
            }
            if !taken(&id) {
                break;
            }
        }
        n += 1;
        id = format!("{stem}-{n}");
    }
    let target = folder.join(format!("{id}.{ext}"));
    std::fs::write(&target, &bytes).map_err(|e| format!("{}: {e}", target.display()))?;
    if let Err(e) = load_in(folder, &id, 0, &mut None) {
        let _ = std::fs::remove_file(&target);
        return Err(e);
    }
    Ok(id)
}

/// A name as a file name: letters and digits kept, lower case, the rest one dash
/// (`pywal: lake.jpg` → `pywal-lake-jpg`).
fn file_name_of(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() || c == '_' {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// A built-in theme shipped as a theme file (inheriting the classic look at most): a file that
/// doesn't read is a bug, caught by the tests, and falls back to the classic look.
pub fn builtin(id: &str, text: &str) -> Theme {
    let read = toml::from_str::<Value>(text)
        .map_err(|e| e.to_string())
        .and_then(|v| {
            parse(id, &v, |base| {
                if base == "classic" {
                    Ok(Loaded {
                        theme: palette::CLASSIC.clone(),
                        warnings: Vec::new(),
                    })
                } else {
                    Err(format!(
                        "{base}: a built-in theme inherits the classic look only"
                    ))
                }
            })
        });
    match read {
        Ok(l) => {
            debug_assert!(l.warnings.is_empty(), "{id}: {:?}", l.warnings);
            l.theme
        }
        Err(e) => {
            debug_assert!(false, "{id}: {e}");
            palette::CLASSIC.clone()
        }
    }
}

/// A theme from a file's value tree; `base` loads the theme it inherits.
pub fn parse(
    id: &str,
    v: &Value,
    base: impl FnOnce(&str) -> Result<Loaded, String>,
) -> Result<Loaded, String> {
    let top = v
        .as_object()
        .ok_or_else(|| format!("{id}: a theme is a table of keys"))?;
    let mut warnings = Vec::new();
    let inherits = match top.get("inherits") {
        None => "classic",
        Some(Value::String(s)) => s.as_str(),
        Some(_) => return Err(format!("{id}: inherits names a theme")),
    };
    if inherits == id {
        return Err(format!("{id}: a theme can't inherit itself"));
    }
    let Loaded {
        mut theme,
        warnings: inherited,
    } = base(inherits)?;
    warnings.extend(inherited);
    theme.id = id.to_owned().into();
    theme.name = match top.get("name").and_then(Value::as_str) {
        Some(n) => n.to_owned().into(),
        None => id.to_owned().into(),
    };
    // A few colours, every role derived from them (before the roles a file sets itself).
    if let Some(v) = top.get("seeds") {
        seeds(v, &mut theme, &mut warnings);
    }
    // High contrast: every edge in the text's colour (then the file's own roles still win).
    match top.get("contrast").map(|c| c.as_str()) {
        None | Some(Some("normal")) => {}
        Some(Some("high")) => {
            crate::derive::strengthen(&mut theme.light);
            crate::derive::strengthen(&mut theme.dark);
        }
        Some(_) => warnings.push("contrast: \"normal\" or \"high\"".into()),
    }
    for (key, value) in top {
        match key.as_str() {
            "name" | "inherits" | "seeds" | "contrast" | "icon-mono" => {}
            // A built-in icon set by name (a theme folder's own icons are read by `load_in`).
            "icon-set" => match value.as_str() {
                Some(name) => theme.icon_set = Some(name.to_owned().into()),
                None => warnings.push("icon-set: a set's name".into()),
            },
            "icons" => match value.as_str().and_then(IconStyle::from_key) {
                Some(s) => theme.icons = s,
                None => warnings.push(format!("icons: {value} is not color, mono or accent")),
            },
            "sizes" => sizes(value, &mut theme, &mut warnings),
            "controls" => match value.as_object() {
                Some(table) => {
                    for (k, v) in table {
                        let ok = v.as_str().is_some_and(|v| theme.controls.set(k, v));
                        if !ok {
                            warnings.push(format!("controls.{k}: {v} is not a style of it"));
                        }
                    }
                }
                None => warnings.push("controls: not a table".into()),
            },
            "colors" | "colours" => {
                colors(key, value, &mut theme.light, &mut warnings);
                colors(key, value, &mut theme.dark, &mut warnings);
            }
            "light" => colors(key, value, &mut theme.light, &mut warnings),
            "dark" => colors(key, value, &mut theme.dark, &mut warnings),
            _ => warnings.push(format!("{key}: not a theme key")),
        }
    }
    Ok(Loaded { theme, warnings })
}

/// `[seeds]` (both variants), `[seeds.light]` and `[seeds.dark]`: each variant given any is
/// derived again from its seeds (`crate::derive`), the ones not given kept from the theme it
/// inherits.
fn seeds(v: &Value, theme: &mut Theme, warnings: &mut Vec<String>) {
    let Some(table) = v.as_object() else {
        warnings.push("seeds: not a table".into());
        return;
    };
    // Shared seeds are read for both variants: their warnings once.
    let mut found = Vec::new();
    for (variant, pal) in [("light", &mut theme.light), ("dark", &mut theme.dark)] {
        let mut s = crate::derive::Seeds::of(pal);
        let mut given = false;
        let both = table.iter().filter(|(k, _)| *k != "light" && *k != "dark");
        let own = table
            .get(variant)
            .and_then(Value::as_object)
            .into_iter()
            .flatten();
        for (key, value) in both.chain(own) {
            let Some(c) = value.as_str().and_then(palette::parse_hex) else {
                found.push(format!("seeds.{key}: {value} is not a colour"));
                continue;
            };
            let slot = match key.as_str() {
                "background" => &mut s.background,
                "surface" => &mut s.surface,
                "header" => &mut s.header,
                "text" => &mut s.text,
                "accent" => &mut s.accent,
                "error" => &mut s.error,
                "warning" => &mut s.warning,
                "success" => &mut s.success,
                "selection" => {
                    s.selection = Some(c);
                    given = true;
                    continue;
                }
                _ => {
                    found.push(format!("seeds.{key}: not a seed"));
                    continue;
                }
            };
            *slot = c;
            given = true;
        }
        if given {
            *pal = crate::derive::derive(&s);
        }
    }
    for w in found {
        if !warnings.contains(&w) {
            warnings.push(w);
        }
    }
}

fn sizes(v: &Value, theme: &mut Theme, warnings: &mut Vec<String>) {
    let Some(table) = v.as_object() else {
        warnings.push("sizes: not a table".into());
        return;
    };
    for (key, value) in table {
        match value.as_f64() {
            // A size from 0 to 400 points (a tooltip's width is the largest).
            Some(n) if (0.0..=400.0).contains(&n) => {
                if !theme.metrics.set(key, n as f32) {
                    warnings.push(format!("sizes.{key}: not a size"));
                }
            }
            _ => warnings.push(format!("sizes.{key}: {value} is not a size")),
        }
    }
}

/// A `[colors]`, `[light]` or `[dark]` table into `pal`.
fn colors(table_name: &str, v: &Value, pal: &mut Palette, warnings: &mut Vec<String>) {
    let Some(table) = v.as_object() else {
        warnings.push(format!("{table_name}: not a table"));
        return;
    };
    for (key, value) in table {
        let flag = match key.as_str() {
            "dark" => Some(&mut pal.dark),
            "focus-line-straight" => Some(&mut pal.focus_line_straight),
            "access-keys-always" => Some(&mut pal.access_keys_always),
            "tree-chevrons" => Some(&mut pal.tree_chevrons),
            _ => None,
        };
        if let Some(flag) = flag {
            match value.as_bool() {
                Some(b) => *flag = b,
                None => warnings.push(format!("{table_name}.{key}: true or false")),
            }
            continue;
        }
        let rows = match key.as_str() {
            "segments.missing" => Some(&mut pal.segments.missing),
            "segments.done" => Some(&mut pal.segments.done),
            "segments.start" => Some(&mut pal.segments.start),
            _ => None,
        };
        if let Some(rows) = rows {
            if !gradient(value, rows) {
                warnings.push(format!(
                    "{table_name}.{key}: one colour or a list of 13 colours"
                ));
            }
            continue;
        }
        match value.as_str().and_then(palette::parse_hex) {
            Some(c) => {
                if !pal.set_role(key, c) {
                    warnings.push(format!("{table_name}.{key}: not a colour role"));
                }
            }
            None => warnings.push(format!(
                "{table_name}.{key}: {value} is not a colour (#rrggbb or #rrggbbaa)"
            )),
        }
    }
}

/// One colour for every row, or 13 colours top to bottom.
fn gradient(v: &Value, rows: &mut [egui::Color32; 13]) -> bool {
    if let Some(c) = v.as_str().and_then(palette::parse_hex) {
        *rows = [c; 13];
        return true;
    }
    let Some(list) = v.as_array().filter(|l| l.len() == 13) else {
        return false;
    };
    let parsed: Option<Vec<_>> = list
        .iter()
        .map(|c| c.as_str().and_then(palette::parse_hex))
        .collect();
    match parsed {
        Some(cs) => {
            rows.copy_from_slice(&cs);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::CLASSIC;
    use egui::Color32;

    /// [`super::load_in`], pywal's file not followed.
    fn load_in(folder: &Path, id: &str, depth: usize) -> Result<Loaded, String> {
        super::load_in(folder, id, depth, &mut None)
    }

    /// The theme in this file, as [`read`] gives it.
    fn load_file(path: &Path) -> Result<Loaded, String> {
        read(path.to_str().unwrap()).0
    }

    fn toml_theme(id: &str, text: &str) -> Result<Loaded, String> {
        let v: Value = toml::from_str(text).map_err(|e| e.to_string())?;
        parse(id, &v, |base| load_in(Path::new("nowhere"), base, 1))
    }

    #[test]
    fn a_file_changes_only_what_it_names() {
        let l = toml_theme(
            "mine",
            r##"
            name = "Mine"
            icons = "mono"
            [sizes]
            radius = 5
            "list.row" = 20
            [colors]
            accent = "#2f7d32"
            [dark]
            window = "202020"
            "segments.done" = "#3fa864"
            "##,
        )
        .unwrap();
        assert!(l.warnings.is_empty(), "{:?}", l.warnings);
        let t = l.theme;
        assert_eq!((t.id.as_ref(), t.name.as_ref()), ("mine", "Mine"));
        assert_eq!(t.icons, IconStyle::Mono);
        assert_eq!(t.metrics.control_radius, 5.0);
        assert_eq!(t.metrics.list_row, 20.0);
        assert_eq!(t.metrics.font_dialog, CLASSIC.metrics.font_dialog);
        let green = Color32::from_rgb(0x2f, 0x7d, 0x32);
        assert_eq!((t.light.accent, t.dark.accent), (green, green));
        assert_eq!(t.dark.window, Color32::from_gray(0x20));
        assert_eq!(t.light.window, CLASSIC.light.window);
        assert_eq!(
            t.dark.segments.done,
            [Color32::from_rgb(0x3f, 0xa8, 0x64); 13]
        );
        assert_eq!(t.dark.text, CLASSIC.dark.text);
    }

    #[test]
    fn json_reads_the_same() {
        let v: Value = serde_json::from_str(
            r##"{ "name": "J", "light": { "accent": "#ff0000", "dark": true } }"##,
        )
        .unwrap();
        let l = parse("j", &v, |b| load_in(Path::new("nowhere"), b, 1)).unwrap();
        assert_eq!(l.theme.light.accent, Color32::RED);
        assert!(l.theme.light.dark);
    }

    #[test]
    fn unknown_keys_and_bad_values_are_warnings() {
        let l = toml_theme(
            "w",
            r##"
            future = 1
            icons = "sparkly"
            [sizes]
            nope = 3
            radius = -1
            [light]
            "no.such.role" = "#000000"
            text = "black"
            "segments.missing" = ["#000000"]
            dark = "yes"
            "##,
        )
        .unwrap();
        assert_eq!(l.warnings.len(), 8, "{:?}", l.warnings);
        assert_eq!(l.theme.light, CLASSIC.light);
        assert_eq!(l.theme.metrics, CLASSIC.metrics);
    }

    #[test]
    fn broken_files_and_loops_are_errors() {
        assert!(toml_theme("x", "name = ").is_err());
        assert!(toml_theme("x", r#"inherits = "x""#).is_err());
        assert!(toml_theme("x", r#"inherits = "not-there""#).is_err());
    }

    #[test]
    fn files_in_the_folder_inherit_one_another() {
        let dir = std::env::temp_dir().join(format!("mdm-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("base.toml"), "[colors]\naccent = \"#112233\"\n").unwrap();
        std::fs::write(
            dir.join("child.json"),
            r##"{ "inherits": "base", "dark": { "text": "#eeeeee" } }"##,
        )
        .unwrap();
        std::fs::write(dir.join("a.toml"), "inherits = \"b\"").unwrap();
        std::fs::write(dir.join("b.toml"), "inherits = \"a\"").unwrap();
        let t = load_in(&dir, "child", 0).unwrap().theme;
        assert_eq!(t.light.accent, Color32::from_rgb(0x11, 0x22, 0x33));
        assert_eq!(t.dark.text, Color32::from_gray(0xee));
        assert_eq!(load_file(&dir.join("child.json")).unwrap().theme, t);
        assert!(load_file(&dir.join("child.txt")).is_err());
        assert!(load_in(&dir, "a", 0).is_err());
        assert_eq!(load_in(&dir, "classic", 0).unwrap().theme, CLASSIC);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mdm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const SCHEME: &str = "scheme: \"Test Scheme\"\nbase00: \"1d2021\"\nbase01: \"32302f\"\n\
        base02: \"504945\"\nbase03: \"665c54\"\nbase04: \"bdae93\"\nbase05: \"d5c4a1\"\n\
        base06: \"ebdbb2\"\nbase07: \"fbf1c7\"\nbase08: \"fb4934\"\nbase09: \"fe8019\"\n\
        base0A: \"fabd2f\"\nbase0B: \"b8bb26\"\nbase0C: \"8ec07c\"\nbase0D: \"83a598\"\n\
        base0E: \"d3869b\"\nbase0F: \"d65d0e\"\n";

    const WAL: &str = r##"{ "wallpaper": "C:\\Users\\someone\\Pictures\\dunes.png",
        "special": { "background": "#f2efe9", "foreground": "#3a3530" },
        "colors": { "color0": "#f2efe9", "color1": "#a8483c", "color2": "#4f7a3a",
            "color3": "#8a6a1e", "color4": "#3d6a9a", "color8": "#8c867c" } }"##;

    #[test]
    fn schemes_in_the_folder_are_themes() {
        let dir = temp_dir("schemes");
        std::fs::write(dir.join("gruvish.yaml"), SCHEME).unwrap();
        std::fs::write(dir.join("colors.json"), WAL).unwrap();
        std::fs::write(dir.join("broken.yml"), "base00: \"1d2021\"\n").unwrap();
        // A theme file built on a scheme: its colours, the file's own changes on top.
        std::fs::write(
            dir.join("mine.toml"),
            "inherits = \"gruvish\"\nicons = \"accent\"\n[sizes]\nradius = 6\n",
        )
        .unwrap();
        let t = load_in(&dir, "gruvish", 0).unwrap().theme;
        assert_eq!(t.name, "Test Scheme");
        assert_eq!(t.dark.field, Color32::from_rgb(0x1d, 0x20, 0x21));
        assert_eq!(t.light, t.dark);
        let mine = load_in(&dir, "mine", 0).unwrap().theme;
        assert_eq!(
            (mine.dark.field, mine.light.accent),
            (t.dark.field, t.light.accent)
        );
        assert_eq!(mine.icons, IconStyle::Accent);
        assert_eq!(mine.metrics.control_radius, 6.0);
        // pywal's file, here a light one: a light theme named after its wallpaper.
        let wal = load_in(&dir, "colors", 0).unwrap();
        assert_eq!(wal.theme.name, "pywal: dunes.png");
        assert!(!wal.theme.light.dark);
        assert_eq!(wal.theme.light.accent, Color32::from_rgb(0x3d, 0x6a, 0x9a));
        assert!(load_in(&dir, "broken", 0).unwrap_err().contains("base05"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adding_copies_into_the_folder_once() {
        let from = temp_dir("add-from");
        let to = from.join("themes");
        std::fs::write(from.join("gruvish.yaml"), SCHEME).unwrap();
        std::fs::write(from.join("colors.json"), WAL).unwrap();
        std::fs::write(from.join("metro.toml"), "[colors]\naccent = \"#123456\"\n").unwrap();
        std::fs::write(from.join("bad.yaml"), "just: words\n").unwrap();
        std::fs::write(from.join("lonely.toml"), "inherits = \"not-here\"\n").unwrap();
        std::fs::write(from.join("notes.txt"), "hi").unwrap();
        assert_eq!(add_in(&to, &from.join("gruvish.yaml")).unwrap(), "gruvish");
        // The same file again: the copy there; another of the same name: a number added.
        assert_eq!(add_in(&to, &from.join("gruvish.yaml")).unwrap(), "gruvish");
        let other = from.join("other").join("gruvish.yml");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, SCHEME.replace("83a598", "458588")).unwrap();
        assert_eq!(add_in(&to, &other).unwrap(), "gruvish-2");
        // pywal's colors.json: named after its wallpaper. A built-in's name: a number added.
        assert_eq!(
            add_in(&to, &from.join("colors.json")).unwrap(),
            "pywal-dunes-png"
        );
        assert_eq!(add_in(&to, &from.join("metro.toml")).unwrap(), "metro-2");
        assert_eq!(
            load_in(&to, "metro-2", 0).unwrap().theme.light.accent,
            Color32::from_rgb(0x12, 0x34, 0x56)
        );
        // Not themes, or not loading once copied: nothing left in the folder.
        for f in ["bad.yaml", "lonely.toml", "notes.txt", "missing.toml"] {
            assert!(add_in(&to, &from.join(f)).is_err(), "{f}");
        }
        let mut names: Vec<String> = std::fs::read_dir(&to)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "gruvish-2.yml",
                "gruvish.yaml",
                "metro-2.toml",
                "pywal-dunes-png.json"
            ]
        );
        let _ = std::fs::remove_dir_all(&from);
    }

    #[test]
    fn a_rewritten_pywal_file_is_seen() {
        let dir = temp_dir("wal-follow");
        let file = dir.join("colors.json");
        std::fs::write(&file, WAL).unwrap();
        let changed = || following_pywal().is_some_and(|(path, seen)| stamp(&path) != seen);
        follow_pywal(Some((file.clone(), stamp(&file))));
        assert!(!changed());
        // `wal` run again: other colours, another size.
        std::fs::write(
            &file,
            WAL.replace("#3d6a9a", "#7b4d9e\", \"color5\": \"#000"),
        )
        .unwrap();
        assert!(changed());
        // Taken as seen (a file that didn't read): not again until it is written again.
        pywal_seen_as(stamp(&file));
        assert!(!changed());
        forget_pywal();
        assert_eq!(following_pywal(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_names_from_names() {
        assert_eq!(file_name_of("pywal: lake.jpg"), "pywal-lake-jpg");
        assert_eq!(file_name_of("Gruvbox dark, medium"), "gruvbox-dark-medium");
        assert_eq!(file_name_of("  ..  "), "");
    }
}
