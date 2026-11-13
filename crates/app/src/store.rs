//! The download list on disk: `%APPDATA%\misterdownloadmanager\downloads.json`, written with craft-atomic.
//! Also holds the list's column layout (order, widths, visibility, sort).
//!
//! Generations: `downloads.json.bak` is the list before the last save; `.1.bak` and `.2.bak`
//! are older, rotated at most every `GENERATION_EVERY` and only when the list changed. Loading
//! takes the newest that parses. A damaged list is kept as `.corrupt-<unix time>.bak`, and a
//! list that exists but can't be read is never overwritten.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use mdm_engine::state::{ResumeState, state_path};
use serde::{Deserialize, Serialize};

use crate::list::Layout;
use crate::model::{Item, Status};
use crate::queue::Queue;
use crate::settings::Settings;

#[derive(Deserialize, Default)]
struct ListFile {
    next_id: u64,
    items: Vec<Item>,
    #[serde(default)]
    columns: Option<Layout>,
    #[serde(default)]
    settings: Settings,
    #[serde(default)]
    queues: Vec<Queue>,
    /// The list format: 2 since queues default to 4 files at once (D-012).
    #[serde(default)]
    version: u32,
}

/// The list format written now.
const VERSION: u32 = 2;

pub struct Loaded {
    pub next_id: u64,
    pub items: Vec<Item>,
    pub columns: Option<Layout>,
    pub settings: Settings,
    pub queues: Vec<Queue>,
}

pub fn path() -> Option<PathBuf> {
    mdm_ipc::data_dir().map(|d| d.join("downloads.json"))
}

/// Load the list; a missing file gives an empty list, a damaged one is kept aside.
pub fn load() -> Loaded {
    let empty = Loaded {
        next_id: 1,
        items: Vec::new(),
        columns: None,
        settings: Settings::default(),
        queues: Vec::new(),
    };
    let Some(p) = path() else { return empty };
    load_from(&p, empty)
}

/// Only the settings, read without touching anything (the command line's debrid accounts,
/// #44): the list as `load` would take it (the file, else its newest generation), but a damaged
/// file is left where it is for the app to deal with. None: no list, or none that reads.
pub fn settings_only() -> Option<Settings> {
    #[derive(Deserialize)]
    struct Only {
        #[serde(default)]
        settings: Settings,
    }
    let p = path()?;
    let read = |p: &Path| {
        let bytes = std::fs::read(p).ok()?;
        let json = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
        serde_json::from_slice::<Only>(json)
            .ok()
            .map(|o| o.settings)
    };
    read(&p).or_else(|| {
        [
            backup_path(&p),
            generation_path(&p, 1),
            generation_path(&p, 2),
        ]
        .iter()
        .find_map(|g| read(g))
    })
}

/// How often an older generation is kept.
const GENERATION_EVERY: Duration = Duration::from_secs(10 * 60);

/// Lists that exist but couldn't be read (permissions, a lock): never saved over.
fn blocked() -> &'static Mutex<HashSet<PathBuf>> {
    static BLOCKED: std::sync::OnceLock<Mutex<HashSet<PathBuf>>> = std::sync::OnceLock::new();
    BLOCKED.get_or_init(Default::default)
}

fn load_from(p: &Path, empty: Loaded) -> Loaded {
    let p = p.to_owned();
    let parsed = match std::fs::read(&p) {
        Ok(bytes) => parse_bytes(&bytes).or_else(|| {
            // Damaged: keep it aside (never overwrite an earlier one) and use a generation.
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let _ = std::fs::rename(&p, p.with_extension(format!("json.corrupt-{stamp}.bak")));
            newest_generation(&p)
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => newest_generation(&p),
        // There but unreadable: work from a generation, and don't save over the real list.
        Err(_) => {
            blocked().lock().unwrap().insert(p.clone());
            newest_generation(&p)
        }
    };
    match parsed {
        Some(mut f) => {
            for i in &mut f.items {
                // The page it came from is sent again (#795): the browser's other context
                // (cookies, User-Agent) is memory only.
                i.context.referer = i.referer.clone();
                // Downloads that were running when the app closed are now stopped (resumable).
                if i.status == Status::Downloading {
                    i.status = Status::Stopped;
                }
                // Live progress isn't written to this file; the engine's resume state is the
                // record of what is on disk. A video's download is yt-dlp's, which keeps no such
                // state: not looked for (a playlist's thousands cost a second at start, #69).
                if i.status != Status::Complete && i.video.is_none() {
                    // Started under the former codename: rename its files so it resumes.
                    mdm_engine::state::migrate_legacy(&i.dest);
                }
                if i.status != Status::Complete
                    && i.video.is_none()
                    && let Ok(st) = ResumeState::load(&state_path(&i.dest))
                    // A state under this name for another address is another row's (#442).
                    && st.url == i.url
                {
                    i.downloaded = st.downloaded();
                    i.size = Some(st.size);
                }
            }
            if f.version < 2 {
                // Before version 2 nothing could set "files at the same time" (no Scheduler),
                // so every 1 there is the old default: IDM's (and our) default is 4.
                for q in f.queues.iter_mut().filter(|q| q.files_at_once == 1) {
                    q.files_at_once = 4;
                }
            }
            let mut settings = f.settings;
            settings.list_known_browsers();
            // An earlier version's default File types, untouched, become today's (#208); a list
            // the user edited stays as it is.
            if let Some(types) = mdm_ipc::capture::upgraded_types(&settings.auto_types) {
                settings.auto_types = types.into();
            }
            Loaded {
                next_id: f.next_id.max(1),
                items: f.items,
                columns: f.columns,
                settings,
                queues: f.queues,
            }
        }
        None => empty,
    }
}

/// The list file at `p`, if it reads and parses.
fn parse(p: &Path) -> Option<ListFile> {
    parse_bytes(&std::fs::read(p).ok()?)
}

/// A UTF-8 byte-order mark is tolerated (editors and PowerShell add one).
fn parse_bytes(bytes: &[u8]) -> Option<ListFile> {
    let json = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    serde_json::from_slice::<ListFile>(json).ok()
}

/// The list before the last save.
fn backup_path(p: &Path) -> PathBuf {
    p.with_extension("json.bak")
}

/// Older generations: `.1.bak` (at least `GENERATION_EVERY` old when replaced), `.2.bak`.
fn generation_path(p: &Path, n: u32) -> PathBuf {
    p.with_extension(format!("json.{n}.bak"))
}

fn newest_generation(p: &Path) -> Option<ListFile> {
    [backup_path(p), generation_path(p, 1), generation_path(p, 2)]
        .iter()
        .find_map(|g| parse(g))
}

/// Before a save: the current list becomes `.bak`, and every `GENERATION_EVERY` (when it
/// changed) also `.1.bak`, the old `.1.bak` moving to `.2.bak`. Only a list that parses is kept:
/// a damaged file never replaces a good generation.
fn keep_generations(p: &Path) {
    let Ok(current) = std::fs::read(p) else {
        return;
    };
    // Read as it is on the disk: its keychain references aren't opened (secrets.rs), which
    // would ask the keychain on every save, and on the window's thread at the last one.
    if crate::secrets::prepared(|| parse_bytes(&current)).is_none() {
        return;
    }
    let _ = std::fs::write(backup_path(p), &current);
    let (g1, g2) = (generation_path(p, 1), generation_path(p, 2));
    let g1_age = std::fs::metadata(&g1)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok());
    let due = match g1_age {
        None => true,
        Some(age) => age >= GENERATION_EVERY && std::fs::read(&g1).ok().as_ref() != Some(&current),
    };
    if due {
        if g1.exists() {
            let _ = std::fs::rename(&g1, &g2);
        }
        let _ = std::fs::write(&g1, &current);
    }
}

pub fn save(
    next_id: u64,
    items: &[Item],
    columns: &Layout,
    settings: &Settings,
    queues: &[Queue],
) -> std::io::Result<()> {
    let Some(p) = path() else { return Ok(()) };
    save_to(&p, next_id, items, columns, settings, queues)
}

fn save_to(
    p: &Path,
    next_id: u64,
    items: &[Item],
    columns: &Layout,
    settings: &Settings,
    queues: &[Queue],
) -> std::io::Result<()> {
    let json = encode(p, next_id, items, columns, settings, queues)?;
    write(p, &json)
}

/// The list as it is written (refused when it couldn't be read at start: not overwritten).
fn encode(
    p: &Path,
    next_id: u64,
    items: &[Item],
    columns: &Layout,
    settings: &Settings,
    queues: &[Queue],
) -> std::io::Result<Vec<u8>> {
    if blocked().lock().unwrap().contains(p) {
        return Err(std::io::Error::other(
            "the download list could not be read at start; not overwriting it",
        ));
    }
    #[derive(Serialize)]
    struct ListRef<'a> {
        version: u32,
        next_id: u64,
        items: Vec<&'a Item>,
        columns: &'a Layout,
        settings: &'a Settings,
        queues: &'a [Queue],
    }
    // The secrets as the disk keeps them: keychain references, or sealed (secrets.rs, #44).
    let settings = &settings.for_disk();
    crate::secrets::prepared(|| {
        serde_json::to_vec_pretty(&ListRef {
            version: VERSION,
            next_id,
            // A private window's downloads live in memory only, as the browser keeps them.
            items: items.iter().filter(|i| !i.context.private).collect(),
            columns,
            settings,
            queues,
        })
    })
    .map_err(std::io::Error::other)
}

/// Write the encoded list: the earlier ones kept as generations, the new one atomically (and
/// flushed to the disk: slow while a big download writes, hence [`Saver`]).
fn write(p: &Path, json: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = p.parent() {
        mdm_ipc::private_dir(dir)?;
    }
    keep_generations(p);
    // The list holds the saved logins: its owner's only (the folder too).
    mdm_ipc::private_write(p, json)?;
    // On the disk now: keychain entries it no longer names can go.
    crate::secrets::vault().after_write();
    Ok(())
}

/// Saves the list on a thread of its own. The write ends with a flush to the disk, which waited
/// for every byte a big download had written meanwhile: on the window's thread it froze MDM for
/// seconds (a crash report caught it: "freeze in store::save"). The newest list waiting replaces
/// an older one not yet written. Any file the window keeps saving goes the same way
/// ([`Saver::writing`]): Windows holding every write on C: for a shadow copy held the window
/// with it (#513).
pub struct Saver {
    shared: std::sync::Arc<(std::sync::Mutex<SaverState>, std::sync::Condvar)>,
}

#[derive(Default)]
struct SaverState {
    waiting: Option<(std::path::PathBuf, Vec<u8>)>,
    writing: bool,
    /// The last write's outcome, not yet taken.
    outcome: Option<Result<(), String>>,
}

impl Saver {
    /// The download list's.
    pub fn new() -> Saver {
        Saver::writing("list saver", write)
    }

    /// A saver thread of its own, `name`, writing what it is handed with `write` (the RSS feeds',
    /// rss.rs).
    pub fn writing(name: &str, write: fn(&Path, &[u8]) -> std::io::Result<()>) -> Saver {
        let shared = std::sync::Arc::new((
            std::sync::Mutex::new(SaverState::default()),
            std::sync::Condvar::new(),
        ));
        let worker = shared.clone();
        let _ = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let (lock, cv) = &*worker;
                loop {
                    let (p, json) = {
                        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
                        while s.waiting.is_none() {
                            s = cv.wait(s).unwrap_or_else(|e| e.into_inner());
                        }
                        s.writing = true;
                        s.waiting.take().expect("checked")
                    };
                    let r = write(&p, &json).map_err(|e| e.to_string());
                    let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
                    s.writing = false;
                    s.outcome = Some(r);
                    cv.notify_all();
                }
            });
        Saver { shared }
    }

    /// Hand the list to the saver thread; an error here is one of encoding (the list refused).
    pub fn save(
        &self,
        next_id: u64,
        items: &[Item],
        columns: &Layout,
        settings: &Settings,
        queues: &[Queue],
    ) -> std::io::Result<()> {
        let Some(p) = path() else { return Ok(()) };
        let json = encode(&p, next_id, items, columns, settings, queues)?;
        self.queue(p, json);
        Ok(())
    }

    /// Hand `json` to the saver thread, to be written as `p`.
    pub fn queue(&self, p: std::path::PathBuf, json: Vec<u8>) {
        let (lock, cv) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).waiting = Some((p, json));
        cv.notify_all();
    }

    /// The outcome of the last write, once.
    pub fn outcome(&self) -> Option<Result<(), String>> {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .outcome
            .take()
    }

    /// Wait until nothing is waiting or being written (MDM closing).
    pub fn flush(&self) {
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
        while s.waiting.is_some() || s.writing {
            s = cv.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Loaded {
        Loaded {
            next_id: 1,
            items: Vec::new(),
            columns: None,
            settings: Settings::default(),
            queues: Vec::new(),
        }
    }

    #[test]
    fn the_saver_writes_the_newest_list_off_the_calling_thread() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("downloads.json");
        let layout = crate::list::ListState::default().layout();
        let saver = Saver::new();
        for id in [3, 5] {
            let json = encode(&p, id, &[], &layout, &Settings::default(), &[]).unwrap();
            saver.queue(p.clone(), json);
        }
        saver.flush();
        assert_eq!(saver.outcome(), Some(Ok(())));
        assert_eq!(load_from(&p, empty()).next_id, 5);
    }

    /// #513: handing a file to a saver never waits for the disk (Windows held every write on C:
    /// for 8 s while it made a shadow copy, and the window with them when it wrote itself); it
    /// is written once the disk lets go, the newest handed over.
    #[test]
    fn a_saver_hands_over_at_once_while_the_disk_holds_its_writes() {
        use std::sync::{Condvar, Mutex};
        static HELD: (Mutex<bool>, Condvar) = (Mutex::new(true), Condvar::new());
        fn held_write(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
            let (held, cv) = &HELD;
            let mut held = held.lock().unwrap();
            while *held {
                held = cv.wait(held).unwrap();
            }
            crate::rss::write(p, bytes)
        }
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rss.json");
        let saver = Saver::writing("test saver", held_write);
        let start = std::time::Instant::now();
        for n in 0..50u8 {
            saver.queue(p.clone(), vec![b'0' + n % 10]);
        }
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(
            !p.exists() && saver.outcome().is_none(),
            "the disk holds it"
        );
        *HELD.0.lock().unwrap() = false;
        HELD.1.notify_all();
        saver.flush();
        assert_eq!(saver.outcome(), Some(Ok(())));
        assert_eq!(std::fs::read(&p).unwrap(), b"9");
    }

    #[test]
    fn a_damaged_list_falls_back_to_the_previous_generation() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("downloads.json");
        let layout = crate::list::ListState::default().layout();
        save_to(&p, 7, &[], &layout, &Settings::default(), &[]).unwrap();
        save_to(&p, 9, &[], &layout, &Settings::default(), &[]).unwrap();
        assert_eq!(load_from(&p, empty()).next_id, 9);
        std::fs::write(&p, b"{ not json").unwrap();
        let back = load_from(&p, empty());
        assert_eq!(
            back.next_id, 7,
            "the backup holds the generation before the last save"
        );
        let kept = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains(".corrupt-"));
        assert!(kept, "damaged file kept aside");
    }

    /// #69: a playlist of thousands, one download each: the list is encoded (on the window's
    /// thread, at most once a second) and read back quickly. Measured, printed with
    /// `--nocapture`; the bound is loose for debug builds.
    #[test]
    fn a_list_of_thousands_saves_and_loads_quickly() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("downloads.json");
        let layout = crate::list::ListState::default().layout();
        let items: Vec<Item> = (1..=6000u64)
            .map(|id| {
                let url =
                    url::Url::parse(&format!("https://www.youtube.com/watch?v=v{id:010}")).unwrap();
                let dest = dir
                    .path()
                    .join("A long playlist")
                    .join(format!("Video number {id} of a long playlist.mp4"));
                let mut i = Item::new(id, url, dest, crate::model::Category::Video);
                i.note("Added from a playlist (yt-dlp)");
                i.video = Some(crate::model::VideoSpec {
                    selector: "bv*[height<=1080]+ba/b[height<=1080]".into(),
                    subs: Default::default(),
                    parts: Vec::new(),
                });
                i
            })
            .collect();
        let t = std::time::Instant::now();
        let json = encode(&p, 6001, &items, &layout, &Settings::default(), &[]).unwrap();
        let encoded = t.elapsed();
        write(&p, &json).unwrap();
        let written = t.elapsed() - encoded;
        let t = std::time::Instant::now();
        assert!(parse_bytes(&json).is_some());
        let parsed = t.elapsed();
        let t = std::time::Instant::now();
        let _ = std::fs::read(&p).unwrap();
        let first_read = t.elapsed();
        let t = std::time::Instant::now();
        let back = load_from(&p, empty());
        let loaded = t.elapsed();
        println!("first read of the new file (an antivirus may scan it): {first_read:?}");
        assert_eq!(back.items.len(), 6000);
        println!(
            "6000 downloads: {} KB, encoded in {encoded:?}, written in {written:?}; parsed in {parsed:?}, loaded (parse and checks) in {loaded:?}",
            json.len() / 1024
        );
        assert!(encoded < Duration::from_secs(3), "{encoded:?}");
    }

    fn save_n(p: &Path, n: u64) {
        let layout = crate::list::ListState::default().layout();
        save_to(p, n, &[], &layout, &Settings::default(), &[]).unwrap();
    }

    /// Pretend `.1.bak` was written `age` ago.
    fn age_generation(p: &Path, age: Duration) {
        let f = std::fs::File::options()
            .write(true)
            .open(generation_path(p, 1))
            .unwrap();
        f.set_modified(std::time::SystemTime::now() - age).unwrap();
    }

    #[test]
    fn older_generations_rotate_at_most_every_ten_minutes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("downloads.json");
        save_n(&p, 1);
        save_n(&p, 2); // .1 = list 1
        save_n(&p, 3); // too soon: .1 stays list 1
        assert_eq!(parse(&generation_path(&p, 1)).unwrap().next_id, 1);
        age_generation(&p, Duration::from_secs(11 * 60));
        save_n(&p, 4); // .1 = list 3, .2 = list 1
        assert_eq!(parse(&generation_path(&p, 1)).unwrap().next_id, 3);
        assert_eq!(parse(&generation_path(&p, 2)).unwrap().next_id, 1);
        assert_eq!(parse(&backup_path(&p)).unwrap().next_id, 3);
    }

    #[test]
    fn loading_falls_back_through_the_generations() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("downloads.json");
        save_n(&p, 1);
        save_n(&p, 2);
        std::fs::write(&p, b"broken").unwrap();
        std::fs::write(backup_path(&p), b"broken too").unwrap();
        assert_eq!(load_from(&p, empty()).next_id, 1, "from .1.bak");
    }

    /// #208: 0.2.x's File types, never edited, gain today's types (DMG, DEB...); an edited list
    /// is kept as it is.
    #[test]
    fn an_untouched_file_types_list_gains_todays_types() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("downloads.json");
        let old = "3GP 7Z AAC ACE AIF APK ARJ ASF AVI BIN BZ2 EXE GZ GZIP IMG ISO LZH M4A M4V MKV MOV MP3 MP4 MPA MPE MPEG MPG MSI MSU OGG OGV PDF PLJ PPS PPT QT R0* R1* RA RAR RM RMVB SEA SIT SITX TAR TIF TIFF WAV WMA WMV Z ZIP";
        let list = |types: &str| {
            serde_json::json!({ "next_id": 1, "items": [], "settings": { "auto_types": types } })
                .to_string()
        };
        std::fs::write(&p, list(old)).unwrap();
        let s = load_from(&p, empty()).settings;
        assert_eq!(s.auto_types, mdm_ipc::capture::DEFAULT_AUTO_TYPES);
        assert!(
            mdm_ipc::capture::type_listed(&s.auto_types, "MDM-0.3.0.dmg")
                && mdm_ipc::capture::type_listed(&s.auto_types, "mdm_0.3.0_amd64.deb")
        );
        let edited = format!("{old} NZB");
        std::fs::write(&p, list(&edited)).unwrap();
        assert_eq!(load_from(&p, empty()).settings.auto_types, edited);
    }

    /// #442: two rows of one name, a resume state on the disk: only the row of its address
    /// takes the progress (the other would take the data for its own and resume it).
    #[test]
    fn a_resume_state_is_read_by_the_row_it_belongs_to() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("downloads.json");
        let dest = dir.path().join("master.mp4");
        let url = "http://files.test/a/master.mp4";
        let row = |id: u64, url: &str| {
            serde_json::json!({
                "id": id, "url": url, "file_name": "master.mp4", "dest": dest, "size": 100,
                "downloaded": 0, "status": "Queued", "category": "Video", "description": "",
                "last_try": null, "error": null, "resumable": null,
            })
        };
        let list = serde_json::json!({
            "next_id": 3, "version": 2,
            "items": [row(1, url), row(2, "http://files.test/b/master.mp4")],
        });
        std::fs::write(&p, list.to_string()).unwrap();
        let (a, b): (url::Url, url::Url) = (url.parse().unwrap(), url.parse().unwrap());
        ResumeState::new(a, b, dest.clone(), 777, Default::default())
            .write(&state_path(&dest))
            .unwrap();
        let loaded = load_from(&p, empty());
        assert_eq!(loaded.items[0].size, Some(777));
        assert_eq!(loaded.items[1].size, Some(100));
    }

    #[test]
    fn old_lists_get_four_files_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("downloads.json");
        let old = r#"{"next_id":3,"items":[],"queues":[{"name":"Main download queue",
            "files_at_once":1,"items":[],"sync":false}]}"#;
        std::fs::write(&p, old).unwrap();
        assert_eq!(load_from(&p, empty()).queues[0].files_at_once, 4);
        // Written as version 2, a 1 chosen in the Scheduler stays.
        let mut q = Queue::new("Main download queue", false);
        q.files_at_once = 1;
        let layout = crate::list::ListState::default().layout();
        save_to(&p, 3, &[], &layout, &Settings::default(), &[q]).unwrap();
        assert_eq!(load_from(&p, empty()).queues[0].files_at_once, 1);
    }

    #[test]
    fn an_unreadable_list_is_never_saved_over() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the file should be: it exists, but reading it fails.
        let p = dir.path().join("downloads.json");
        std::fs::create_dir(&p).unwrap();
        let _ = load_from(&p, empty());
        let layout = crate::list::ListState::default().layout();
        assert!(save_to(&p, 5, &[], &layout, &Settings::default(), &[]).is_err());
        assert!(p.is_dir());
    }

    #[test]
    fn a_damaged_file_never_replaces_a_good_backup() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("downloads.json");
        let layout = crate::list::ListState::default().layout();
        save_to(&p, 3, &[], &layout, &Settings::default(), &[]).unwrap();
        save_to(&p, 4, &[], &layout, &Settings::default(), &[]).unwrap();
        std::fs::write(&p, b"garbage").unwrap();
        save_to(&p, 5, &[], &layout, &Settings::default(), &[]).unwrap();
        assert_eq!(parse(&backup_path(&p)).unwrap().next_id, 3);
    }
}
