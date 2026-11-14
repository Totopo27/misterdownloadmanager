//! Rename row (`Rename`: main name + extension) in the Add torrent dialog (#517).
//!
//! - Placed directly below the `Save to` row.
//! - Splits the target name into two separate inputs:
//!   1. **Main name (`stem`)**
//!   2. **Extension (`ext`)**
//! - When the torrent is a directory (multi-file torrent):
//!   - The main name input edits the folder name.
//!   - The extension input is greyed out and disabled (`enabled = false`).
//! - When the torrent is a single file:
//!   - Both the main name and extension inputs are enabled and editable separately.

use std::path::{Path, PathBuf};
use std::sync::Once;

use egui::{Painter, Pos2, Ui};
use mdm_torrent::Meta;

use crate::winui::{self, r, text};

#[derive(Default)]
pub struct TorrentRenamePlugin {
    pub stem: String,
    pub ext: String,
    orig_stem: String,
    orig_ext: String,
    pub is_dir: bool,
    single_idx: Option<usize>,
    orig_parent: Option<PathBuf>,
}

/// Split a single file's name into `(stem, ext)`.
pub fn split_file_name(name: &str) -> (String, String) {
    if let Some((s, e)) = name.rsplit_once('.')
        && !s.is_empty()
        && !e.is_empty()
    {
        (s.to_owned(), e.to_owned())
    } else {
        (name.to_owned(), String::new())
    }
}

impl TorrentRenamePlugin {
    /// Translated label for the Rename row in Add torrent.
    pub fn label() -> &'static str {
        // i18n: noun: label before the new file or folder name in Add torrent
        crate::i18n::t("Rename")
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn on_meta_loaded(&mut self, meta: &Meta) {
        let non_padding: Vec<(usize, &mdm_torrent::FileEntry)> = meta
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.padding)
            .collect();
        if non_padding.len() == 1 {
            let (idx, entry) = non_padding[0];
            let file_name = entry
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| meta.name.clone());
            let parent = entry
                .path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(Path::to_path_buf);
            let (stem, ext) = split_file_name(&file_name);
            self.stem = stem.clone();
            self.ext = ext.clone();
            self.orig_stem = stem;
            self.orig_ext = ext;
            self.is_dir = false;
            self.single_idx = Some(idx);
            self.orig_parent = parent;
        } else {
            self.stem = meta.name.clone();
            self.ext = String::new();
            self.orig_stem = meta.name.clone();
            self.orig_ext = String::new();
            self.is_dir = true;
            self.single_idx = None;
            self.orig_parent = None;
        }
    }

    fn raw_candidate(&self) -> String {
        let stem = self.stem.trim();
        if self.is_dir {
            if stem.is_empty() {
                self.orig_stem.clone()
            } else {
                stem.to_owned()
            }
        } else {
            let stem_effective = if stem.is_empty() {
                self.orig_stem.as_str()
            } else {
                stem
            };
            let ext = self.ext.trim();
            if ext.is_empty() {
                stem_effective.to_owned()
            } else {
                format!("{stem_effective}.{ext}")
            }
        }
    }

    fn orig_file_name(&self) -> String {
        if self.orig_ext.is_empty() {
            self.orig_stem.clone()
        } else {
            format!("{}.{}", self.orig_stem, self.orig_ext)
        }
    }

    fn stem_unsafe(&self) -> Option<mdm_torrent::UnsafeName> {
        let stem = self.stem.trim();
        if stem.is_empty() {
            return None;
        }
        mdm_torrent::check_single_name(stem, self.is_dir).err()
    }

    fn ext_unsafe(&self) -> Option<mdm_torrent::UnsafeName> {
        if self.is_dir {
            return None;
        }
        let ext = self.ext.trim();
        if ext.is_empty() {
            return None;
        }
        mdm_torrent::check_single_name(ext, false).err()
    }

    fn candidate_unsafe(&self) -> Option<mdm_torrent::UnsafeName> {
        if let Some(u) = self.stem_unsafe() {
            return Some(u);
        }
        if let Some(u) = self.ext_unsafe() {
            return Some(u);
        }
        let candidate = self.raw_candidate();
        if candidate.is_empty() {
            return None;
        }
        if let Err(u) = mdm_torrent::check_single_name(&candidate, self.is_dir) {
            return Some(u);
        }
        if !self.is_dir
            && let Some(parent) = &self.orig_parent
        {
            return mdm_torrent::check_relative_path(&parent.join(&candidate)).err();
        }
        None
    }

    /// Check the uncleaned name directly against `mdm_torrent`'s safety rules (`names::check`),
    /// returning the translated refusal message if any part isn't safe to write as typed.
    pub fn candidate_error(&self) -> Option<String> {
        self.candidate_unsafe()
            .map(|u| crate::errors::torrent(&mdm_torrent::TorrentError::UnsafeName(u)))
    }

    /// Compute the effective target name and apply it to `Meta`.
    pub fn apply_to_meta(&self, meta: &mut Meta) -> String {
        let valid = self.candidate_unsafe().is_none();
        let candidate = self.raw_candidate();
        if self.is_dir {
            let effective = if !valid || candidate.is_empty() {
                self.orig_stem.clone()
            } else {
                candidate
            };
            meta.name = effective.clone();
            effective
        } else {
            let (file_name, rel_path) = if valid && !candidate.is_empty() {
                let rel = match &self.orig_parent {
                    Some(parent) => parent.join(&candidate),
                    None => PathBuf::from(&candidate),
                };
                (candidate, rel)
            } else {
                let orig = self.orig_file_name();
                let rel = match &self.orig_parent {
                    Some(parent) => parent.join(&orig),
                    None => PathBuf::from(&orig),
                };
                (orig, rel)
            };
            meta.name = file_name.clone();
            if let Some(idx) = self.single_idx
                && let Some(entry) = meta.files.get_mut(idx)
            {
                entry.path = rel_path;
            }
            file_name
        }
    }

    /// Render the `Rename` row at vertical position `y`.
    /// Returns `true` if the user edited either the main name or the extension.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        ui: &mut Ui,
        p: &Painter,
        o: Pos2,
        fx: f32,
        y: f32,
        has: bool,
        busy: bool,
        meta: &mut Option<Meta>,
    ) -> bool {
        let label = Self::label();
        text(p, o.x + 11.0, o.y + y + 14.0, label, winui::TEXT());

        let stem_enabled = has && !busy;
        let ext_enabled = has && !busy && !self.is_dir;

        let stem_rc = r(o, fx, y, 468.0 - fx, 21.0);
        let ext_rc = r(o, 480.0, y, 589.0 - 480.0, 21.0);

        // Main name (stem) input box
        let stem_changed = winui::edit(ui, stem_rc, &mut self.stem, stem_enabled, false).changed();

        // Dot separator between main name and extension
        let dot_color = if ext_enabled {
            winui::TEXT()
        } else {
            winui::LIGHT_DISABLED()
        };
        text(p, o.x + 472.0, o.y + y + 14.0, ".", dot_color); // not translated: file extension dot

        // Extension input box (greyed out / disabled when downloading a directory)
        let ext_changed = winui::edit(ui, ext_rc, &mut self.ext, ext_enabled, false).changed();

        if stem_enabled {
            let stem_bad = self.stem_unsafe().is_some();
            let ext_bad = ext_enabled && self.ext_unsafe().is_some();
            let combined_bad = !stem_bad && !ext_bad && self.candidate_unsafe().is_some();
            if stem_bad || combined_bad {
                winui::error_frame(p, stem_rc);
            }
            if ext_bad {
                winui::error_frame(p, ext_rc);
            }
        }

        let changed = (stem_changed || ext_changed) && has && !busy;
        if changed && let Some(m) = meta.as_mut() {
            self.apply_to_meta(m);
        }
        changed
    }
}

fn session_dir() -> Option<PathBuf> {
    crate::torrents::state_dir().map(|d| d.join("session"))
}

/// Ensure saved single-file renames are loaded into memory on startup.
pub fn ensure_loaded() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if let Some(dir) = session_dir() {
            mdm_torrent::rename_plugin::load_into_memory(&dir);
        }
    });
}

/// Return the custom file or folder name chosen in Add torrent when it differs from the
/// torrent's original name.
pub fn renamed_target(meta: &Meta) -> Option<String> {
    let non_padding: Vec<&mdm_torrent::FileEntry> =
        meta.files.iter().filter(|f| !f.padding).collect();
    if let [single] = non_padding.as_slice() {
        let changed = mdm_torrent::rename_plugin::original_single_file_path(&meta.torrent)
            .is_none_or(|orig| orig != single.path);
        changed
            .then(|| {
                single
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .flatten()
            .filter(|n| !n.is_empty())
    } else {
        let changed = mdm_torrent::rename_plugin::original_torrent_name(&meta.torrent)
            .is_none_or(|orig| orig != meta.name);
        changed.then(|| meta.name.clone()).filter(|n| !n.is_empty())
    }
}

/// Persist single-file torrent rename when adding a torrent from `TorrentDialog`,
/// only when the file name was actually changed from the torrent's original name.
pub fn on_add_torrent(meta: &Meta) {
    ensure_loaded();
    let Some(dir) = session_dir() else { return };
    on_add_torrent_in(&dir, meta);
}

pub fn on_add_torrent_in(session_dir: &Path, meta: &Meta) {
    let non_padding: Vec<&mdm_torrent::FileEntry> =
        meta.files.iter().filter(|f| !f.padding).collect();
    if let [single] = non_padding.as_slice() {
        mdm_torrent::rename_plugin::sync_renamed_file(
            session_dir,
            &meta.info_hash,
            &meta.torrent,
            &single.path,
        );
    }
}

/// Clean up rename mapping when a torrent is removed from the list.
pub fn on_forget_torrent(hash: &str) {
    ensure_loaded();
    if let Some(dir) = session_dir() {
        mdm_torrent::rename_plugin::remove_renamed_file(&dir, hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdm_torrent::FileEntry;

    fn sample_meta(name: &str, files: &[(&str, u64)]) -> Meta {
        Meta {
            name: name.to_owned(),
            info_hash: "a".repeat(40),
            files: files
                .iter()
                .map(|(p, s)| FileEntry {
                    path: PathBuf::from(p),
                    size: *s,
                    padding: false,
                })
                .collect(),
            piece_length: 16384,
            torrent: Vec::new(),
            peers: Vec::new(),
        }
    }

    #[test]
    fn single_file_splits_stem_and_extension_and_renames_independently() {
        let file = "[LoliHouse] Dark Summoner to Dekiteiru - 06 [WebRip 1080p HEVC-10bit AAC].mkv";
        let mut meta = sample_meta(file, &[(file, 424_433_664)]);
        let mut plugin = TorrentRenamePlugin::default();
        plugin.on_meta_loaded(&meta);

        assert!(!plugin.is_dir);
        assert_eq!(
            plugin.stem,
            "[LoliHouse] Dark Summoner to Dekiteiru - 06 [WebRip 1080p HEVC-10bit AAC]"
        );
        assert_eq!(plugin.ext, "mkv");

        // Modify stem and extension separately
        plugin.stem = "Dark Summoner - EP06".to_owned();
        plugin.ext = "mp4".to_owned();
        plugin.apply_to_meta(&mut meta);

        assert_eq!(meta.name, "Dark Summoner - EP06.mp4");
        assert_eq!(
            meta.files[0].path,
            PathBuf::from("Dark Summoner - EP06.mp4")
        );
    }

    #[test]
    fn directory_torrent_disables_extension_and_renames_folder() {
        let mut meta = sample_meta(
            "Season.01.1080p.Pack",
            &[("ep01.mkv", 100), ("ep02.mkv", 200)],
        );
        let mut plugin = TorrentRenamePlugin::default();
        plugin.on_meta_loaded(&meta);

        assert!(plugin.is_dir);
        assert_eq!(plugin.stem, "Season.01.1080p.Pack");
        assert_eq!(plugin.ext, "");

        plugin.stem = "My Renamed Folder".to_owned();
        plugin.apply_to_meta(&mut meta);

        assert_eq!(meta.name, "My Renamed Folder");
        assert_eq!(
            meta.folder_in(Path::new("/tmp/dl")),
            PathBuf::from("/tmp/dl/My Renamed Folder")
        );
    }

    #[test]
    fn refuses_separators_dot_dot_and_unclean_folder_names_before_cleaning() {
        let mut meta = sample_meta("Sample Clip.mkv", &[("Sample Clip.mkv", 1024)]);
        let mut plugin = TorrentRenamePlugin::default();
        plugin.on_meta_loaded(&meta);

        for bad_stem in ["../x", "a/b", r"a\b", "..", "."] {
            plugin.stem = bad_stem.to_owned();
            plugin.ext = "mkv".to_owned();
            assert!(
                plugin.candidate_error().is_some(),
                "stem {bad_stem:?} must be refused with an error message"
            );
            plugin.apply_to_meta(&mut meta);
            assert_eq!(meta.files[0].path, PathBuf::from("Sample Clip.mkv"));
        }

        plugin.stem = "Sample Clip".to_owned();
        for bad_ext in ["x/../y", "a/b", "..", "."] {
            plugin.ext = bad_ext.to_owned();
            assert!(
                plugin.candidate_error().is_some(),
                "extension {bad_ext:?} must be refused with an error message"
            );
        }

        let mut dir_meta = sample_meta("Pack", &[("1.bin", 10), ("2.bin", 20)]);
        let mut dir_plugin = TorrentRenamePlugin::default();
        dir_plugin.on_meta_loaded(&dir_meta);
        for bad_folder in ["../Pack", "a/b", "..", "Pack:1", "Pack."] {
            dir_plugin.stem = bad_folder.to_owned();
            assert!(
                dir_plugin.candidate_error().is_some(),
                "folder name {bad_folder:?} must be refused rather than silently rewritten"
            );
            dir_plugin.apply_to_meta(&mut dir_meta);
            assert_eq!(dir_meta.name, "Pack");
        }
    }
}
