//! What the look follows outside MDM, read off the window's thread (#719): Windows' light or dark
//! app mode, its accent and contrast colours (the registry), and the theme files (the data
//! folder's `themes` folder, pywal's colours). A registry read can wait for seconds (#719: 8 s,
//! on the window's thread, froze the window), a folder on a slow or network disk as long; so a
//! thread of their own reads each ([`Watched`]), the window takes the last answer and is woken
//! when one changes. The rule of the clipboard's reads (#418) and the torrents' numbers (#693).
//!
//! Linux's desktop settings come from the portal's and the theme settings' own threads
//! (`unix::desktop_dark`), macOS's accent from `launch::Asked`; neither is read here.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::palette::Theme;
use crate::themefile::{self, Entry, Followed, Stamp};

/// How often each source is read again when nothing said it changed: the theme check's 2 s.
const EVERY: Duration = Duration::from_secs(2);

/// The longest the window waits for the system's first answer, once (as it starts): enough for a
/// registry that answers, so the window opens in the right mode; a light one until it comes.
#[cfg_attr(not(windows), allow(dead_code))]
const FIRST_WAIT: Duration = Duration::from_millis(500);

/// A value read again and again by a thread of its own: the window asks for the last answer
/// ([`Watched::latest`]), which never waits for a read, and is told when one changes. A read that
/// never returns holds up nothing but the next read.
pub struct Watched<T> {
    shared: Arc<Shared<T>>,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    came: Condvar,
}

struct State<T> {
    /// The last answer, and how many different answers came (its number).
    latest: Option<T>,
    generation: u64,
    closed: bool,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<T: Clone + PartialEq + Send + 'static> Watched<T> {
    /// `read` reads the source (it may wait, for ever); `wait` returns when it should be read
    /// again (a change notified, or its time); `wake` is told when an answer differs from the
    /// last one.
    pub fn new(
        name: &str,
        mut read: impl FnMut() -> T + Send + 'static,
        mut wait: impl FnMut() + Send + 'static,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                latest: None,
                generation: 0,
                closed: false,
            }),
            came: Condvar::new(),
        });
        let worker = shared.clone();
        let spawned = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                loop {
                    // Not under the lock: the window never waits for a read.
                    let value = read();
                    let changed = {
                        let mut s = worker.lock();
                        if s.closed {
                            return;
                        }
                        let changed = s.latest.as_ref() != Some(&value);
                        if changed {
                            s.latest = Some(value);
                            s.generation += 1;
                            worker.came.notify_all();
                        }
                        changed
                    };
                    if changed {
                        wake();
                    }
                    wait();
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("{name}: no thread to read it ({e}), not followed");
        }
        Watched { shared }
    }

    /// The last answer and its number (1 for the first, one more for each change); None before
    /// the first. Never waits for a read.
    pub fn latest(&self) -> Option<(u64, T)> {
        let s = self.shared.lock();
        s.latest.clone().map(|v| (s.generation, v))
    }

    /// [`Watched::latest`], waiting at most `within` for the first answer.
    // Windows' system look only: Linux's and macOS's come from their own watchers.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn first(&self, within: Duration) -> Option<(u64, T)> {
        let start = Instant::now();
        let mut s = self.shared.lock();
        while s.latest.is_none() {
            let left = within.saturating_sub(start.elapsed());
            if left.is_zero() {
                return None;
            }
            s = self
                .shared
                .came
                .wait_timeout(s, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        s.latest.clone().map(|v| (s.generation, v))
    }

    /// How many different answers came.
    pub fn generation(&self) -> u64 {
        self.shared.lock().generation
    }
}

impl<T> Drop for Watched<T> {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
    }
}

/// The window woken when an answer changes (set at the first ask).
static WINDOW: OnceLock<egui::Context> = OnceLock::new();

fn window(ctx: &egui::Context) {
    let _ = WINDOW.set(ctx.clone());
}

fn wake_window() {
    if let Some(ctx) = WINDOW.get() {
        ctx.request_repaint();
    }
}

/// How many times what the watchers here read changed: the app's theme check runs in the frame
/// such a change wakes (`theme::system_changed`), not at its next 2 s.
pub fn generation() -> u64 {
    #[cfg(windows)]
    let system = SYSTEM.get().map_or(0, Watched::generation);
    #[cfg(not(windows))]
    let system = 0;
    system + FILES.get().map_or(0, Watched::generation)
}

// ---- Windows' app mode and colours -----------------------------------------------------------

/// What Windows says about the look: dark apps, and the accent and contrast colours (Native).
#[cfg(windows)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SystemLook {
    pub dark: bool,
    pub native: crate::native::System,
}

#[cfg(windows)]
static SYSTEM: OnceLock<Watched<SystemLook>> = OnceLock::new();

/// What Windows said last: read again at once when the registry says one of its keys changed
/// (`win32::LookKeys`), else every 2 s. Waits only at the very first ask, at most
/// [`FIRST_WAIT`]; light with no accent until an answer comes.
#[cfg(windows)]
pub fn system(ctx: &egui::Context) -> SystemLook {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ASKED: AtomicBool = AtomicBool::new(false);
    window(ctx);
    let watched = SYSTEM.get_or_init(|| {
        // The keys are opened by the watcher's thread too: opening one can wait as a read does.
        let mut keys: Option<crate::win32::LookKeys> = None;
        Watched::new(
            "system look",
            || SystemLook {
                dark: crate::win32::apps_use_dark(),
                native: crate::native::windows_now(),
            },
            move || {
                keys.get_or_insert_with(crate::win32::LookKeys::open)
                    .wait(EVERY)
            },
            wake_window,
        )
    });
    let within = if ASKED.swap(true, Ordering::Relaxed) {
        Duration::ZERO
    } else {
        FIRST_WAIT
    };
    watched.first(within).map(|(_, v)| v).unwrap_or_default()
}

// ---- the theme files ---------------------------------------------------------------------------

/// The theme files as the watcher last found them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThemeFiles {
    /// View → Theme's themes ([`themefile::list`]).
    pub list: Vec<Entry>,
    /// The active theme read again after `wal` wrote pywal's file anew (the last time it did).
    pub pywal: Option<PywalRead>,
}

/// The active theme read again from pywal's rewritten file.
#[derive(Clone, Debug, PartialEq)]
pub struct PywalRead {
    /// The theme (its id or path, as chosen) and the file it followed, as they were then: the
    /// window takes it only while they still are.
    pub id: String,
    pub followed: Followed,
    /// The file's stamp that was new.
    pub written: Stamp,
    /// The theme and what in it was skipped, or why it didn't read; and the file to follow now.
    pub read: Result<(Theme, Vec<String>), String>,
    pub follow: Option<Followed>,
}

static FILES: OnceLock<Watched<ThemeFiles>> = OnceLock::new();

/// The theme files as the watcher last found them, with their number (it changes with them);
/// None before its first look. Never waits.
pub fn theme_files(ctx: &egui::Context) -> Option<(u64, ThemeFiles)> {
    window(ctx);
    FILES
        .get_or_init(|| {
            Watched::new(
                "theme files",
                files_reader(|| (themefile::dir(), themefile::pywal_file())),
                || std::thread::sleep(EVERY),
                wake_window,
            )
        })
        .latest()
}

/// A reader of the themes folder and pywal's file, which `places` names (asked on the reader's
/// thread: finding them can look at the disk). The folder is listed again when its time or
/// pywal's file changed (a file added, removed or renamed; `wal` run for the first time or its
/// colours removed); the active theme is read again when it follows pywal's file and `wal`
/// wrote it since ([`themefile::following_pywal`]).
fn files_reader(
    places: impl FnOnce() -> (Option<PathBuf>, Option<PathBuf>) + Send + 'static,
) -> impl FnMut() -> ThemeFiles + Send + 'static {
    let mut places = Some(places);
    let mut found: Option<(Option<PathBuf>, Option<PathBuf>)> = None;
    let mut seen = None;
    let mut files = ThemeFiles::default();
    let mut reread: Option<(Followed, Stamp, String)> = None;
    move || {
        if found.is_none() {
            found = places.take().map(|f| f());
        }
        let (dir, wal) = found.clone().unwrap_or_default();
        let now = (
            dir.as_deref().and_then(themefile::folder_stamp),
            wal.as_deref().is_some_and(Path::is_file),
        );
        if seen != Some(now) {
            seen = Some(now);
            files.list = themefile::list_in(dir.as_deref(), wal);
        }
        if let Some(followed) = themefile::following_pywal() {
            let written = themefile::stamp(&followed.0);
            let id = crate::theme::selected();
            let key = (followed.clone(), written, id.clone());
            // Once for each write: the window takes it in the frame it wakes.
            if written != followed.1 && reread.as_ref() != Some(&key) {
                reread = Some(key);
                let (read, follow) = themefile::read(&id);
                files.pywal = Some(PywalRead {
                    id,
                    followed,
                    written,
                    read: read.map(|l| (l.theme, l.warnings)),
                    follow,
                });
            }
        }
        files.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    /// A source that answers once, then never again (a registry read or a folder that hangs, as
    /// in #719): the window's asks still answer at once, with the last answer.
    #[test]
    fn a_read_that_never_returns_holds_up_nothing() {
        let (woke_tx, woke_rx) = mpsc::channel();
        let (_hold, never) = mpsc::channel::<()>();
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = reads.clone();
        let w = Watched::new(
            "test hang",
            move || {
                if counted.fetch_add(1, Ordering::SeqCst) > 0 {
                    // Hangs for good: nothing is ever sent.
                    let _ = never.recv();
                }
                true
            },
            || {},
            move || {
                let _ = woke_tx.send(());
            },
        );
        // The first answer came, and woke the window.
        woke_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(w.first(Duration::from_secs(5)), Some((1, true)));
        // The second read hangs. Every frame's ask answers at once all the same.
        while reads.load(Ordering::SeqCst) < 2 {
            std::thread::yield_now();
        }
        let start = Instant::now();
        for _ in 0..1000 {
            assert_eq!(w.latest(), Some((1, true)));
            assert_eq!(w.generation(), 1);
        }
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    /// A source that hangs from its very first read: the window gets no answer (the defaults),
    /// at once, or after the bounded first wait.
    #[test]
    fn a_first_read_that_never_returns_holds_up_nothing() {
        let (_hold, never) = mpsc::channel::<()>();
        let w: Watched<bool> = Watched::new(
            "test hang first",
            move || {
                let _ = never.recv();
                true
            },
            || {},
            || {},
        );
        let start = Instant::now();
        assert_eq!(w.latest(), None);
        assert_eq!(w.first(Duration::from_millis(100)), None);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    /// Only a different answer counts as a change, and wakes the window.
    #[test]
    fn changes_are_counted_and_wake() {
        let (tx, rx) = mpsc::channel::<u32>();
        let (woke_tx, woke_rx) = mpsc::channel();
        let rx = Mutex::new(rx);
        let w = Watched::new(
            "test changes",
            move || rx.lock().unwrap().recv().unwrap_or(0),
            || {},
            move || {
                let _ = woke_tx.send(());
            },
        );
        for v in [1, 1, 1, 2] {
            tx.send(v).unwrap();
        }
        woke_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        woke_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(w.latest(), Some((2, 2)));
        assert!(woke_rx.recv_timeout(Duration::from_millis(100)).is_err());
        drop(tx);
    }

    /// The reader lists a theme added to the folder at its next look, and pywal's colours once
    /// `wal` writes them; a folder that didn't change isn't listed again.
    #[test]
    fn the_files_reader_sees_a_theme_added() {
        let dir = std::env::temp_dir().join(format!("mdm-lookwatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let themes = dir.join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        let wal = dir.join("wal").join("colors.json");
        let mut read = files_reader({
            let (themes, wal) = (themes.clone(), wal.clone());
            move || (Some(themes), Some(wal))
        });
        let builtins = read().list;
        assert!(builtins.iter().all(|e| e.path.is_none()));
        std::fs::write(themes.join("sunny.toml"), "name = \"Sunny\"\n").unwrap();
        // A folder's time can be coarse: written until the folder says it changed.
        let start = Instant::now();
        let list = loop {
            let list = read().list;
            if list.len() > builtins.len() || start.elapsed() > Duration::from_secs(5) {
                break list;
            }
            std::thread::sleep(Duration::from_millis(50));
            std::fs::write(themes.join("touch.txt"), "").unwrap();
        };
        let sunny = list.iter().find(|e| e.id == "sunny").expect("listed");
        assert_eq!(sunny.name, "Sunny");
        std::fs::create_dir_all(wal.parent().unwrap()).unwrap();
        std::fs::write(&wal, "{}").unwrap();
        assert!(read().list.iter().any(|e| e.id == themefile::PYWAL));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
