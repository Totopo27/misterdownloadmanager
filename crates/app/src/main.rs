#![windows_subsystem = "windows"]

mod app;
mod batch;
mod categories;
mod checksum;
mod chrome;
mod clipbatch;
mod clipboard;
mod clock;
mod cnl;
mod countries;
mod crash;
mod crashui;
mod debrid;
mod defaultbrowser;
mod deferred_rename;
mod derive;
mod dialogs;
mod droptarget;
mod errors;
mod extension;
mod extractq;
mod fileicons;
mod find;
mod finish;
mod gallerydlg;
mod grabber;
mod headless;
mod help;
mod helpers;
mod hotkey;
mod i18n;
mod icons;
mod langpick;
mod launch;
mod list;
mod lookwatch;
mod model;
mod native;
mod netwatch;
mod options;
mod pacfetch;
mod palette;
mod playlistdlg;
mod progress;
mod properties;
mod queue;
mod quota;
mod regional;
mod remote;
mod renderer;
mod rss;
mod rssdlg;
mod scale;
mod scheduler;
mod schemes;
mod scrollbar;
mod secrets;
mod selection;
mod settings;
mod store;
mod theme;
mod themefile;
mod themeicons;
mod torrent_rename;
mod torrentdlg;
mod torrentfiles;
mod torrents;
mod transfer;
mod tray;
#[cfg(unix)]
mod unix;
mod update;
mod videodlg;
mod videolookup;
mod wake;
mod webmark;
mod whereis;
#[cfg(windows)]
mod win32;
mod winui;
#[cfg(any(target_os = "linux", test))]
mod xdgicons;

/// The main window's title (dialogs find their owner by it).
pub const MAIN_TITLE: &str = concat!("Mister Download Manager ", env!("CARGO_PKG_VERSION"));

/// The app icon (art/, made with ChatGPT; sizes by tools/icon/build_icon.py), for windows that
/// show one as IDM's do: the 256 px size, which the system scales down for the title bar, the
/// taskbar and Alt+Tab at any display scaling. On macOS none: the Dock keeps the app bundle's own
/// icon (AppIcon.icns, every size, tools/installer/build-macos.sh), which an icon set at run
/// time would replace.
pub fn app_icon() -> std::sync::Arc<egui::IconData> {
    if cfg!(target_os = "macos") {
        // eframe takes an empty icon as "none" (AppTitleIconSetter).
        static NONE: std::sync::OnceLock<std::sync::Arc<egui::IconData>> =
            std::sync::OnceLock::new();
        return NONE.get_or_init(Default::default).clone();
    }
    app_picture()
}

/// The app icon's picture, the 256 px size, on every system: what MDM draws or hands over as a
/// picture (the tray's icon, macOS's menu bar item; the crash report window's heading). Not
/// [`app_icon`], empty on macOS: an empty picture can't be made into the menu bar item, so macOS
/// had none and closing the window quit (#529).
pub fn app_picture() -> std::sync::Arc<egui::IconData> {
    static ICON: std::sync::OnceLock<std::sync::Arc<egui::IconData>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        let png = include_bytes!("../assets/icons/app/mdm-256.png");
        std::sync::Arc::new(eframe::icon_data::from_png_bytes(png).unwrap_or_default())
    })
    .clone()
}

/// MDM_OFFSCREEN=1, for screenshots and checks (tools/lab/docs-shots.ps1): every window opens
/// far off the screens, never takes the focus and has no taskbar button, and there is no tray
/// icon and no sound, so nothing shows while it runs. The windows still draw, and PrintWindow
/// captures them.
pub fn offscreen() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MDM_OFFSCREEN").is_some_and(|v| v == "1"))
}

/// The clipboard's text, if any. Never read in an off-screen run ([`offscreen`]): its windows
/// open from scripts, and the clipboard belongs to the person at the computer (Add URL's
/// address came from it, and was fetched). It may wait for the clipboard's owner for ever:
/// never called on the window's thread, only by `clipboard`'s reader (#418, #661).
pub fn clipboard_text() -> Option<String> {
    // Off-screen test copies: MDM_TEST_CLIPBOARD=<file> is what they find in the clipboard
    // (#709's checks of Add batch download from clipboard), read when they ask, as the
    // clipboard would be; the person's own clipboard is never read.
    if offscreen()
        && let Some(file) = std::env::var_os("MDM_TEST_CLIPBOARD")
    {
        return std::fs::read_to_string(file).ok();
    }
    // Nor in unit tests: the person's clipboard isn't theirs, and tests opening a window that
    // reads it (Download video's) on parallel threads corrupted the heap (Windows' clipboard,
    // opened from two threads at once).
    read_clipboard(offscreen() || cfg!(test), || {
        arboard::Clipboard::new().ok()?.get_text().ok()
    })
}

/// A number that changes whenever the clipboard does, so the watch for copied links
/// (clipboard.rs) reads it only then: Windows' sequence number. None where the system keeps none
/// a program can ask for without opening the clipboard (Linux, macOS): read at each look.
/// Off-screen test copies: when their `MDM_TEST_CLIPBOARD` file last changed (or never).
pub fn clipboard_changes() -> Option<u64> {
    if offscreen() {
        let changed = std::env::var_os("MDM_TEST_CLIPBOARD")
            .and_then(|f| std::fs::metadata(f).ok()?.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64);
        return Some(changed);
    }
    #[cfg(windows)]
    return Some(u64::from(win32::clipboard_sequence()));
    // Linux and macOS: no such number without the clipboard's own protocol (X11's XFixes,
    // macOS's changeCount): the watch reads the text each time, off the window's thread.
    #[cfg(not(windows))]
    None
}

/// How long a read or write of the clipboard waits for the one under way in this process.
/// Ours take milliseconds; one that waits on a program that owns the clipboard and hangs
/// (#418) is given up on, so the window's thread is never held by it for longer.
const CLIPBOARD_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// One use of the clipboard at a time in the whole process, egui's copy and paste included
/// (the same lock, `egui_winit::clipboard::lock_system_clipboard`): Windows lets the threads
/// of a process open it at the same time, and two reads at once (the clipboard monitor's and
/// Add URL's) corrupted the heap and ended MDM (#629). None: the wait ran out.
fn clipboard_lock() -> Option<egui_winit::clipboard::SystemClipboardLock> {
    let lock = egui_winit::clipboard::lock_system_clipboard(CLIPBOARD_WAIT);
    if lock.is_none() {
        tracing::warn!("clipboard: still in use elsewhere in MDM, not read or written");
    }
    lock
}

fn read_clipboard(off: bool, read: impl FnOnce() -> Option<String>) -> Option<String> {
    if off {
        return None;
    }
    let _lock = clipboard_lock()?;
    read()
}

/// Put `text` on the clipboard (egui hands it to the system after the frame). An off-screen
/// run leaves the clipboard alone and logs what it would have copied, for a check to read.
pub fn copy_text(ctx: &egui::Context, text: String) {
    if let Some(text) = write_clipboard(offscreen(), text) {
        ctx.copy_text(text);
    }
}

/// [`copy_text`] at once, outside a frame; whether the clipboard took it (off-screen: logged,
/// and true).
pub fn set_clipboard(text: &str) -> bool {
    match write_clipboard(offscreen(), text.to_owned()) {
        Some(text) => clipboard_lock().is_some_and(|_lock| {
            arboard::Clipboard::new()
                .and_then(|mut c| c.set_text(text))
                .is_ok()
        }),
        None => true,
    }
}

fn write_clipboard(off: bool, text: String) -> Option<String> {
    if off {
        tracing::info!("clipboard (off-screen, not copied): {text}");
        None
    } else {
        Some(text)
    }
}

/// Where [`offscreen`] puts the main window: far off the screens, or at `MDM_OFFSCREEN_AT=x,y`.
/// That is for a machine whose screen nobody watches (the docs screenshots on GitHub's runner,
/// .github/workflows/docs-shots.yml): there the software OpenGL (Mesa's llvmpipe) presents
/// through GDI, which draws nothing off the screens.
pub fn offscreen_at() -> [f32; 2] {
    std::env::var("MDM_OFFSCREEN_AT")
        .ok()
        .and_then(|v| {
            let (x, y) = v.split_once(',')?;
            Some([x.trim().parse().ok()?, y.trim().parse().ok()?])
        })
        .unwrap_or([-20000.0, -20000.0])
}

/// A window's builder as [`offscreen`] wants it; unchanged otherwise. Dialogs open centred on
/// the main window, so they are off the screens with it.
pub fn quiet(builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
    if offscreen() {
        builder.with_active(false).with_taskbar(false)
    } else {
        builder
    }
}

/// How this start shows the main window (#226).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// In front, as a start by the user does.
    Shown,
    /// Started with the session (the startup entry's [`mdm_ipc::STARTUP`]): hidden, downloads,
    /// queues and the Scheduler running. It stays in the notification area once MDM's icon is
    /// there, or shows minimized if none comes (app.rs `at_login`, decided there alone).
    Session,
    /// Started with the session where a window can't be hidden (Wayland): minimized.
    Minimized,
}

/// How a start shows the main window: `at_login` when the startup entry started it, `can_hide`
/// where a window can be hidden (not on Wayland).
fn start_mode(at_login: bool, can_hide: bool) -> Start {
    match (at_login, can_hide) {
        (false, _) => Start::Shown,
        (true, true) => Start::Session,
        (true, false) => Start::Minimized,
    }
}

/// Whether a window can be hidden here (Wayland can't hide one: it can only be minimized).
fn can_hide() -> bool {
    #[cfg(target_os = "linux")]
    return !unix::on_wayland();
    #[cfg(not(target_os = "linux"))]
    true
}

/// Whether the main window can wait hidden in the notification area (Options' tooltip): a tray
/// shows MDM's icon (Linux: a tray host is there, #59) and the window can be hidden.
pub fn can_start_in_tray() -> bool {
    can_hide() && tray::available()
}

fn main() -> std::process::ExitCode {
    renderer::mark_start();
    // PAC's myIpAddress from the routing table, not a UDP socket (a firewall prompt, #35).
    #[cfg(windows)]
    mdm_engine::net::set_source_lookup(win32::source_address);
    // misterdownloadmanager-cli's commands (crates/cli): its launcher starts this program for
    // them. Nothing of the app's runs: no window, instance, crash monitor or crash log.
    if let Some(cli) = mdm_cli::launched(std::env::args_os().collect()) {
        // The default browser's User-Agent (#668): a command's download starts at once, so it
        // waits for it a moment (Windows reads it from the registry; Linux and macOS may ask the
        // browser its version). Only `get` and `resume` download with it: the others don't wait.
        let found = defaultbrowser::start();
        if mdm_cli::downloads(&cli.args) {
            defaultbrowser::wait(&found, std::time::Duration::from_secs(3));
        }
        // A windowed program has no console: the launcher's, unless the output goes to a file
        // or a pipe. On Linux and macOS the launcher became this process, terminal and all.
        #[cfg(windows)]
        win32::cli_console(cli.launcher);
        // Downloads go through the debrid accounts of Options → Accounts (#44).
        return mdm_cli::main_with(cli.args, Some(&debrid::CliAccounts));
    }
    // The Windows uninstaller, when the user chose to remove the data too: MDM's sign-ins leave
    // the system's keychain with it (#44).
    if std::env::args().nth(1).as_deref() == Some("--forget-sign-ins") {
        return secrets::forget_all();
    }
    // The local time's offset, asked on a thread of its own from now on (#536).
    clock::start();
    // A server (#157): the app without a window.
    let all: Vec<String> = std::env::args().skip(1).collect();
    let code = if headless::asked(&all) {
        headless::main(all)
    } else {
        match app() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            // As `main` returning the error would.
            Err(e) => {
                eprintln!("Error: {e:?}");
                std::process::ExitCode::FAILURE
            }
        }
    };
    // The log's last lines, still on their way to the file (its own thread, #513).
    mdm_crash::logs::flush(LOG_AT_THE_END);
    code
}

/// How long MDM's end waits for its log's last lines (a disk that holds them longer loses them).
const LOG_AT_THE_END: std::time::Duration = std::time::Duration::from_secs(2);

fn app() -> eframe::Result {
    // MDM's crash monitor (crash.rs started it): the report window when the app crashes or
    // freezes; it never becomes MDM's instance.
    let all: Vec<String> = std::env::args().collect();
    if let Some(m) = mdm_crash::MonitorArgs::parse(&all) {
        return crashui::monitor_main(m);
    }

    // A windowed app has no console: record panics in %APPDATA%\misterdownloadmanager\crash.log.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(dir) = mdm_ipc::data_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let bt = std::backtrace::Backtrace::force_capture();
            let _ = std::fs::write(dir.join("crash.log"), format!("{info}\n\n{bt}"));
        }
        default_hook(info);
    }));

    // IDM-style command line; hand it to a running instance if there is one.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = mdm_ipc::parse_args(&args);
    // Started by an update (update.rs): the old copy may still be closing; this one waits for
    // it rather than handing it anything.
    let after_update = args.iter().any(|a| a.eq_ignore_ascii_case(update::UPDATED));
    if !after_update
        && (mdm_ipc::send(&cmd.clone().unwrap_or_default()).is_ok()
            || cmd.as_ref().is_some_and(|c| c.exit))
    {
        return Ok(());
    }
    // No instance answered. Only one launch may become the instance (two started together
    // would both get here); the other forwards its command once the winner listens.
    let lock = if after_update {
        update::lock_after_update()
    } else {
        mdm_ipc::instance_lock()
    };
    let Some(_instance) = lock else {
        let _ = mdm_ipc::send_waiting(&cmd.unwrap_or_default(), std::time::Duration::from_secs(10));
        return Ok(());
    };
    #[cfg(windows)]
    win32::mark_running();
    // Started with the session (#226): in the notification area, not in front.
    let start = start_mode(cmd.as_ref().is_some_and(|c| c.startup), can_hide());
    // This launch is MDM: its fonts are looked up while the rest starts (only the instance pays
    // for them, not a launch that hands its command over).
    theme::prepare_fonts();
    // And the file icons' themes (Linux: gsettings), off the window's thread.
    #[cfg(target_os = "linux")]
    unix::prepare_file_icons();
    // What an earlier update set aside (old programs, a Mac's old app, downloaded files).
    update::tidy();
    // The command channel opens now, before the window: the browser's host that started MDM
    // waits for it, and what arrives meanwhile is handled at the first frame. Without it the
    // app still works; it just can't receive commands.
    let (tx, commands) = std::sync::mpsc::channel();
    if let Some(cmd) = cmd {
        let _ = tx.send(mdm_ipc::Incoming { cmd, reply: None });
    }
    let window: std::sync::Arc<std::sync::OnceLock<egui::Context>> = Default::default();
    let wake = window.clone();
    let wake: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || {
        if let Some(ctx) = wake.get() {
            ctx.request_repaint();
        }
    });
    // macOS: so do the .torrent files and magnet links Finder and the browsers hand over (#698).
    #[cfg(target_os = "macos")]
    unix::open_events::start(tx.clone(), wake.clone());
    // The remote control (#141) sends its commands down the same channel.
    let sink = mdm_ipc::Sink::new(tx.clone(), wake.clone());
    let _ = mdm_ipc::serve(tx, move || wake());

    // Downloads run on a Tokio runtime; the UI thread only reads snapshots.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let handle = rt.handle().clone();
    // The list and settings, read before the window opens: it comes back where it was left.
    let loaded = store::load();
    // View → Interface size (#262), or MDM_ZOOM=2 (everything drawn twice as large: sharp 2×
    // screenshots, the windows' sizing at high display scaling). The window is made before the
    // zoom applies: its sizes are given zoomed here.
    let z = scale::startup(loaded.settings.interface_size);
    theme::init_smooth_text(loaded.settings.smooth_text);
    // The log (release builds too) and the crash monitor.
    if let Some(data) = mdm_ipc::data_dir() {
        mdm_crash::logs::init(&data, mdm_crash::logs::APP);
        tracing::info!(
            "Mister Download Manager {} ({}) starting",
            help::VERSION,
            help::COMMIT
        );
    }
    // The default browser, looked for once on a thread of its own (logged): its User-Agent is
    // the one links added by hand go with (#668).
    defaultbrowser::start();
    if let Some(data) = mdm_ipc::data_dir() {
        crash::start(
            &data,
            crash::session(
                loaded.settings.language.as_deref().unwrap_or("system"),
                loaded.settings.dark_mode_support,
            ),
        );
    }
    // Linux: the libraries the window loads when it opens (#265). Without them it can't open
    // (winit stopped at start): MDM says what's missing, in its language, and how to install it.
    #[cfg(target_os = "linux")]
    {
        i18n::set_lang(loaded.settings.language.as_deref());
        let ws = unix::window_system();
        if !ws.missing.is_empty() {
            crash::clean_exit();
            unix::tell_window_failed(&unix::missing_text(&ws.missing, true));
            std::process::exit(1);
        }
        if !ws.x11_missing.is_empty() {
            let text = unix::missing_text(&ws.x11_missing, false);
            eprintln!("{text}");
            tracing::warn!("{text}");
        }
    }
    // Secrets from before (sealed, or kept as they were) go to the system's keychain now; the
    // list is saved with their references once it holds them (secrets.rs, #44).
    let _ = loaded.settings.for_disk();
    #[cfg(unix)]
    use unix::on_a_screen;
    #[cfg(windows)]
    use win32::on_a_screen;
    let saved = loaded
        .settings
        .window
        .filter(|_| scale::test_zoom().is_none() && !offscreen())
        .filter(|[x, y, w, h]| *w >= 600 && *h >= 300 && on_a_screen(x + 40, y + 10));
    let size = saved.map_or([930.0 * z, 470.0 * z], |[_, _, w, h]| [w as f32, h as f32]);
    let mut viewport = quiet(
        egui::ViewportBuilder::default()
            .with_title(MAIN_TITLE)
            .with_inner_size(size)
            .with_min_inner_size([600.0 * z, 300.0 * z])
            .with_maximized(loaded.settings.window_maximized && !offscreen())
            .with_icon(app_icon()),
    );
    if let Some([x, y, ..]) = saved {
        viewport = viewport.with_position([x as f32, y as f32]);
    }
    if offscreen() {
        viewport = viewport.with_position(offscreen_at());
    }
    // Hidden from its first frame on (eframe keeps a root window built hidden so), until the
    // app knows whether its tray icon is shown (app.rs at_login).
    if start == Start::Session {
        tracing::info!("started with the session: hidden");
        viewport = viewport.with_visible(false);
    }
    // OpenGL or Direct3D (View → Draw with, #89), the next way when one can't start (Direct3D on
    // WARP before OpenGL, #847).
    let plan = renderer::plan(
        loaded.settings.renderer,
        renderer::overridden(&args).as_deref(),
        renderer::Known::read(),
    );
    let options = || eframe::NativeOptions {
        viewport: viewport.clone(),
        // Linux: X11 (XWayland) over Wayland when both are there, so menus open where clicked.
        #[cfg(target_os = "linux")]
        event_loop_builder: unix::event_loop_hook(),
        ..Default::default()
    };
    let result = renderer::run("misterdownloadmanager", &plan, options, move |cc| {
        #[cfg(windows)]
        win32::set_main_window(cc);
        scale::start(&cc.egui_ctx, z);
        let _ = window.set(cc.egui_ctx.clone());
        clipboard::wake(&cc.egui_ctx);
        renderer::test_slow_start();
        app::App::new(&cc.egui_ctx, handle, commands, sink, loaded, start)
    });
    crash::clean_exit();
    result
}

#[cfg(test)]
mod tests {
    use super::{Start, start_mode};

    /// The tests that take the process-wide clipboard lock, one at a time: they run in parallel
    /// in one process, and the reads of one waited out the others' (more than the 500 ms a read
    /// waits), so a read meant to go through came back empty.
    static CLIPBOARD_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clipboard_test() -> std::sync::MutexGuard<'static, ()> {
        CLIPBOARD_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Started with the session (#226): hidden (the tray decided by the app), or minimized
    /// where a window can't be hidden; a start by the user shows the window.
    #[test]
    fn a_start_with_the_session_waits_in_the_tray() {
        assert_eq!(start_mode(true, true), Start::Session);
        assert_eq!(start_mode(true, false), Start::Minimized);
        assert_eq!(start_mode(false, true), Start::Shown);
        assert_eq!(start_mode(false, false), Start::Shown);
    }

    /// An off-screen run neither reads the clipboard (Add URL, Batch, the clipboard monitor…)
    /// nor writes it (Copy magnet link, Copy…): it logs what it would have copied.
    #[test]
    fn offscreen_runs_leave_the_clipboard_alone() {
        let _one = clipboard_test();
        let mut asked = false;
        let got = super::read_clipboard(true, || {
            asked = true;
            Some("https://meet.example/abc".into())
        });
        assert_eq!(got, None);
        assert!(!asked, "the clipboard isn't even opened");
        assert_eq!(
            super::read_clipboard(false, || Some("x".into())),
            Some("x".into())
        );
        assert_eq!(
            super::write_clipboard(true, "magnet:?xt=urn:btih:ab".into()),
            None
        );
        assert_eq!(
            super::write_clipboard(false, "magnet:?xt=urn:btih:ab".into()),
            Some("magnet:?xt=urn:btih:ab".into())
        );
    }

    /// #629: two reads of the clipboard at the same moment (the clipboard monitor's thread and
    /// Add URL on the window's) corrupted the heap. Every read waits for the one under way.
    #[test]
    fn clipboard_reads_are_one_at_a_time() {
        let _one = clipboard_test();
        use std::sync::atomic::{AtomicUsize, Ordering};
        let inside = std::sync::Arc::new(AtomicUsize::new(0));
        let most = std::sync::Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (inside, most) = (inside.clone(), most.clone());
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        super::read_clipboard(false, || {
                            let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                            most.fetch_max(now, Ordering::SeqCst);
                            std::thread::sleep(std::time::Duration::from_millis(1));
                            inside.fetch_sub(1, Ordering::SeqCst);
                            None
                        });
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(most.load(Ordering::SeqCst), 1, "reads at the same moment");
    }

    /// #629 on the real clipboard: several threads read it at once for a few seconds, and the
    /// process lives. Ignored by default (it reads the clipboard of the person at the computer;
    /// it never writes it, and what it reads is dropped unseen):
    /// `cargo test -p mdm-app --bin misterdownloadmanager -- --ignored real_clipboard`.
    #[test]
    #[ignore = "reads the real clipboard"]
    fn real_clipboard_reads_from_many_threads_live() {
        let _one = clipboard_test();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let threads: Vec<_> = (0..6)
            .map(|_| {
                std::thread::spawn(move || {
                    let mut reads = 0u32;
                    while std::time::Instant::now() < until {
                        std::hint::black_box(super::clipboard_text().map(|t| t.len()));
                        reads += 1;
                    }
                    reads
                })
            })
            .collect();
        let reads: u32 = threads.into_iter().map(|t| t.join().unwrap()).sum();
        assert!(reads > 0);
    }
}
