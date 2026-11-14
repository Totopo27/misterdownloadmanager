//! Single-file torrent rename support at the librqbit storage layer (#517).
//!
//! For multi-file (directory) torrents, renaming `Meta::name` already changes the target
//! directory via `Meta::folder_in`. For single-file torrents, the bencoded `.torrent` info
//! dictionary cannot be mutated without changing the SHA-1 `info_hash`. This module wraps
//! `librqbit`'s `FilesystemStorageFactory` so that single-file torrents with a user-chosen
//! filename write, read, check, and delete directly at the renamed path on disk.

use std::any::TypeId;
use std::collections::HashMap;
use std::io::IoSlice;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use librqbit::storage::filesystem::FilesystemStorageFactory;
use librqbit::storage::{BoxStorageFactory, StorageFactory, StorageFactoryExt, TorrentStorage};
use librqbit::{ManagedTorrentShared, TorrentMetadata};

const RENAMES_FILE: &str = "renames.json";

fn memory_map() -> &'static Mutex<HashMap<String, String>> {
    static MAP: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn load_disk_map(session_dir: &Path) -> HashMap<String, String> {
    let path = session_dir.join(RENAMES_FILE);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
        .map(|mut map| {
            map.retain(|k, v| {
                !k.trim().is_empty() && crate::check_relative_path(Path::new(v.trim())).is_ok()
            });
            map
        })
        .unwrap_or_default()
}

fn save_disk_map(session_dir: &Path, map: &HashMap<String, String>) {
    let path = session_dir.join(RENAMES_FILE);
    if map.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let _ = std::fs::create_dir_all(session_dir);
    let tmp = session_dir.join("renames.json.tmp");
    if let Ok(json) = serde_json::to_string_pretty(map)
        && std::fs::write(&tmp, json).is_ok()
    {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Extract the original single-file relative path from bencoded `.torrent` bytes.
pub fn original_single_file_path(torrent: &[u8]) -> Option<PathBuf> {
    let meta = librqbit::torrent_from_bytes(torrent).ok()?;
    let info = meta.info.data.validate().ok()?;
    let mut non_padding = info
        .iter_file_details()
        .filter(|f| !f.attrs().padding)
        .map(|f| f.filename.to_pathbuf());
    let first = non_padding.next()?;
    if non_padding.next().is_some() {
        return None;
    }
    Some(first)
}

/// Extract the original torrent name from bencoded `.torrent` bytes.
pub fn original_torrent_name(torrent: &[u8]) -> Option<String> {
    let meta = librqbit::torrent_from_bytes(torrent).ok()?;
    let info = meta.info.data.validate().ok()?;
    info.name().map(|n| n.to_string())
}

/// Load and validate all saved renames from `renames.json` in `session_dir` into memory.
pub fn load_into_memory(session_dir: &Path) {
    let map = load_disk_map(session_dir);
    let mut mem = memory_map().lock().unwrap();
    for (k, v) in map {
        mem.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
    }
}

/// Record a custom relative file name for a single-file torrent `hash`.
pub fn set_renamed_file(session_dir: &Path, hash: &str, relative_path: &str) {
    let key = hash.trim().to_ascii_lowercase();
    let val = relative_path.trim().to_owned();
    if key.is_empty() || crate::check_relative_path(Path::new(&val)).is_err() {
        return;
    }
    memory_map()
        .lock()
        .unwrap()
        .insert(key.clone(), val.clone());
    let mut map = load_disk_map(session_dir);
    map.insert(key, val);
    save_disk_map(session_dir, &map);
}

/// Record a rename only when `current_path` differs from the torrent's original file name;
/// otherwise remove any stale entry so unchanged torrents never write to `renames.json`.
pub fn sync_renamed_file(session_dir: &Path, hash: &str, torrent: &[u8], current_path: &Path) {
    if let Some(orig) = original_single_file_path(torrent)
        && orig == current_path
    {
        remove_renamed_file(session_dir, hash);
        return;
    }
    set_renamed_file(session_dir, hash, &current_path.to_string_lossy());
}

/// Remove a custom file name mapping when a torrent is forgotten.
pub fn remove_renamed_file(session_dir: &Path, hash: &str) {
    let key = hash.trim().to_ascii_lowercase();
    memory_map().lock().unwrap().remove(&key);
    let mut map = load_disk_map(session_dir);
    if map.remove(&key).is_some() {
        save_disk_map(session_dir, &map);
    }
}

/// Clear the in-memory cache (used by tests to verify disk reload from `renames.json`).
pub fn clear_memory_for_test() {
    memory_map().lock().unwrap().clear();
}

/// Look up the custom relative file path for `hash`, if any.
pub fn get_renamed_file(session_dir: &Path, hash: &str) -> Option<PathBuf> {
    let key = hash.trim().to_ascii_lowercase();
    if let Some(val) = memory_map().lock().unwrap().get(&key).cloned() {
        return Some(PathBuf::from(val));
    }
    let map = load_disk_map(session_dir);
    if let Some(val) = map.get(&key).cloned() {
        memory_map().lock().unwrap().insert(key, val.clone());
        return Some(PathBuf::from(val));
    }
    None
}

/// Apply the renamed path to `FileEntry` list for single-file torrents.
pub fn apply_to_entries(session_dir: &Path, hash: &str, entries: &mut [crate::FileEntry]) {
    if entries.iter().filter(|f| !f.padding).count() != 1 {
        return;
    }
    let Some(renamed) = get_renamed_file(session_dir, hash) else {
        return;
    };
    if let Some(entry) = entries.iter_mut().find(|f| !f.padding) {
        entry.path = renamed;
    }
}

/// Apply the renamed path from the in-memory map (used by `files_from_bytes`).
pub fn apply_from_memory(hash: &str, entries: &mut [crate::FileEntry]) {
    if entries.iter().filter(|f| !f.padding).count() != 1 {
        return;
    }
    let key = hash.trim().to_ascii_lowercase();
    let Some(val) = memory_map().lock().unwrap().get(&key).cloned() else {
        return;
    };
    if let Some(entry) = entries.iter_mut().find(|f| !f.padding) {
        entry.path = PathBuf::from(val);
    }
}

#[derive(Clone)]
pub struct RenameStorageFactory {
    session_dir: PathBuf,
}

pub fn storage_factory(session_dir: PathBuf) -> BoxStorageFactory {
    RenameStorageFactory { session_dir }.boxed()
}

impl StorageFactory for RenameStorageFactory {
    type Storage = RenameStorage;

    fn create(
        &self,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
    ) -> anyhow::Result<Self::Storage> {
        let hash = shared.info_hash.as_string();
        let custom = get_renamed_file(&self.session_dir, &hash);
        let inner = FilesystemStorageFactory::default().create(shared, metadata)?;
        Ok(RenameStorage {
            inner: Box::new(inner),
            custom,
        })
    }

    fn is_type_id(&self, type_id: TypeId) -> bool {
        FilesystemStorageFactory::default().is_type_id(type_id)
    }

    fn clone_box(&self) -> BoxStorageFactory {
        self.clone().boxed()
    }
}

pub struct RenameStorage {
    inner: Box<dyn TorrentStorage>,
    custom: Option<PathBuf>,
}

impl TorrentStorage for RenameStorage {
    fn init(
        &mut self,
        shared: &ManagedTorrentShared,
        metadata: &TorrentMetadata,
    ) -> anyhow::Result<()> {
        if let Some(ref renamed) = self.custom
            && metadata
                .file_infos
                .iter()
                .filter(|f| !f.attrs.padding)
                .count()
                == 1
        {
            let mut file_infos = metadata.file_infos.clone();
            if let Some(fi) = file_infos.iter_mut().find(|f| !f.attrs.padding) {
                fi.relative_filename = renamed.clone();
            }
            let modified = TorrentMetadata {
                info: metadata.info.clone(),
                torrent_bytes: metadata.torrent_bytes.clone(),
                info_bytes: metadata.info_bytes.clone(),
                file_infos,
            };
            return self.inner.init(shared, &modified);
        }
        self.inner.init(shared, metadata)
    }

    fn pread_exact(&self, file_id: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        self.inner.pread_exact(file_id, offset, buf)
    }

    fn pwrite_all(&self, file_id: usize, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        self.inner.pwrite_all(file_id, offset, buf)
    }

    fn pwrite_all_vectored(
        &self,
        file_id: usize,
        offset: u64,
        bufs: [IoSlice<'_>; 2],
    ) -> anyhow::Result<usize> {
        self.inner.pwrite_all_vectored(file_id, offset, bufs)
    }

    fn remove_file(&self, file_id: usize, filename: &Path) -> anyhow::Result<()> {
        if let Some(ref renamed) = self.custom {
            return self.inner.remove_file(file_id, renamed);
        }
        self.inner.remove_file(file_id, filename)
    }

    fn remove_directory_if_empty(&self, path: &Path) -> anyhow::Result<()> {
        self.inner.remove_directory_if_empty(path)
    }

    fn ensure_file_length(&self, file_id: usize, length: u64) -> anyhow::Result<()> {
        self.inner.ensure_file_length(file_id, length)
    }

    fn take(&self) -> anyhow::Result<Box<dyn TorrentStorage>> {
        Ok(Box::new(RenameStorage {
            inner: self.inner.take()?,
            custom: self.custom.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_persistence_and_storage_type_id() {
        let dir = tempfile::tempdir().unwrap();
        let hash = "0123456789abcdef0123456789abcdef01234567";
        set_renamed_file(dir.path(), hash, "RenamedVideo.mp4");
        assert_eq!(
            get_renamed_file(dir.path(), hash),
            Some(PathBuf::from("RenamedVideo.mp4"))
        );

        let mut entries = vec![crate::FileEntry {
            path: PathBuf::from("Original.mkv"),
            size: 100,
            padding: false,
        }];
        apply_to_entries(dir.path(), hash, &mut entries);
        assert_eq!(entries[0].path, PathBuf::from("RenamedVideo.mp4"));

        let factory = storage_factory(dir.path().to_path_buf());
        assert!(factory.is_type_id(TypeId::of::<FilesystemStorageFactory>()));

        remove_renamed_file(dir.path(), hash);
        assert_eq!(get_renamed_file(dir.path(), hash), None);
        assert!(!dir.path().join(RENAMES_FILE).exists());
    }

    #[test]
    fn loading_renames_json_rejects_absolute_paths_and_dot_dot() {
        let dir = tempfile::tempdir().unwrap();
        let bad_abs = "1111111111111111111111111111111111111111";
        let bad_win = "2222222222222222222222222222222222222222";
        let bad_rel = "3333333333333333333333333333333333333333";
        let good = "4444444444444444444444444444444444444444";
        let raw = serde_json::json!({
            bad_abs: "/etc/passwd",
            bad_win: r"C:\Windows\System32\cmd.exe",
            bad_rel: "../escape.mkv",
            good: "SafeName.mkv",
        });
        std::fs::write(dir.path().join(RENAMES_FILE), raw.to_string()).unwrap();
        clear_memory_for_test();

        assert_eq!(get_renamed_file(dir.path(), bad_abs), None);
        assert_eq!(get_renamed_file(dir.path(), bad_win), None);
        assert_eq!(get_renamed_file(dir.path(), bad_rel), None);
        assert_eq!(
            get_renamed_file(dir.path(), good),
            Some(PathBuf::from("SafeName.mkv"))
        );
    }
}
