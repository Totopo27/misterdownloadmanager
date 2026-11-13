//! The notification-area icon (IDM: "IDM menu pops up when you right click on IDM icon"
//! [IDM-doc]). Left click shows the window again, or hides it there while it shows; right click
//! shows the menu. The menu's
//! contents are ours ([unknown] IDM's exact items, lab). Windows, macOS (menu bar) and Linux
//! (StatusNotifierItem over D-Bus: KDE, GNOME with its AppIndicator extension).

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

mod beside;
mod watch;

pub use watch::Icon;

use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// What the tray asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCmd {
    Show,
    /// Left click on the icon: the window back when hidden or behind other
    /// windows, hidden there when frontmost.
    Toggle,
    AddUrl,
    Batch,
    /// Add batch download from clipboard (#709).
    BatchClipboard,
    LimiterOn,
    LimiterOff,
    Scheduler,
    StartQueue,
    StopQueue,
    Options,
    Exit,
    /// "Downloads in progress" → a download: its progress window, shown again (#63).
    ShowDownload(u64),
}

/// Whether a tray shows MDM's icon at all, so hiding a window there can be undone. Windows and
/// macOS (the menu bar) always have one; on Linux only when a StatusNotifierItem host runs (#59),
/// asked each time: it may come and go. (Linux's icon itself is made beside the window: `Setup`.)
pub fn available() -> bool {
    #[cfg(target_os = "linux")]
    return crate::unix::tray_host();
    #[cfg(not(target_os = "linux"))]
    true
}

/// What closing the main window does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnClose {
    /// Hidden in the notification area (the menu bar on macOS); downloads go on.
    Hide,
    /// Left in the taskbar (the Dock on macOS), minimized: the tray has no icon to come back
    /// from; downloads go on.
    Minimize,
    Quit,
}

/// What closing the main window does: `keep_running` is Options' "Keep running when closing the
/// window", `tray` whether a tray object exists, `icon` what the shell last said of its icon
/// ([`Tray::icon`]; Linux: there when a tray host runs), `taskbar` whether a window can wait in the
/// taskbar or the Dock instead (not Linux: #59, a window there that merely minimized could never
/// be closed).
///
/// Never a hidden window with no icon to bring it back: the taskbar (or quitting) only when the
/// shell said the icon is gone. No answer, or an old one, hides the window as an icon that is
/// there does (#750, #718, #754: a shell that couldn't confirm the icon left the window in the
/// taskbar with the icon right there); such a window still comes back from the icon once the
/// shell answers, and a second start of MDM always brings it back.
pub fn on_close(keep_running: bool, tray: bool, icon: Icon, taskbar: bool) -> OnClose {
    if !(keep_running && tray) {
        OnClose::Quit
    } else if icon != Icon::Gone {
        OnClose::Hide
    } else if taskbar {
        OnClose::Minimize
    } else {
        OnClose::Quit
    }
}

/// The longest download name in the tray menu, in characters (the rest ends in "…").
const MENU_NAME_MAX: usize = 48;

/// The tray icon as the window holds it: the icon and its menu (Windows, macOS: made on the
/// window's thread; Linux: on a thread of its own, see `Setup`), and the commands they send.
pub struct Tray {
    native: Held,
    rx: Receiver<TrayCmd>,
    /// The tip shown and when, so the shell is told at most once a second.
    tip: std::cell::RefCell<(String, Option<std::time::Instant>)>,
    /// What Start and Stop main download queue were last set to (told only when it changes).
    queue_running: std::cell::Cell<Option<bool>>,
    /// The downloads "Downloads in progress" was last given (rebuilt only when they change).
    shown_downloads: std::cell::RefCell<Vec<(u64, String)>>,
}

/// The icon and its menu, as the system's tray library makes them.
struct Native {
    limiter_on: CheckMenuItem,
    limiter_off: CheckMenuItem,
    /// Start and Stop main download queue: one of them enabled, as the queue runs.
    start_queue: MenuItem,
    stop_queue: MenuItem,
    /// The icon itself, kept for as long as the tray (Windows asks and tells the shell about it
    /// from the watcher's thread, through its hidden window).
    #[cfg_attr(windows, allow(dead_code))]
    tray: TrayIcon,
    /// "Downloads in progress": one entry per running download, rebuilt when they change.
    downloads: Submenu,
    download_ids: Arc<Mutex<Vec<(MenuId, u64)>>>,
    /// Every entry with its English text, to translate again when the language changes.
    labels: Vec<(Entry, &'static str)>,
    /// What [`Tray::icon`] last saw and when: the minimize path asks every frame, and the menu
    /// bar shouldn't be asked that often (a second of staleness is enough).
    #[cfg(target_os = "macos")]
    visible_checked: std::cell::Cell<Option<(Icon, std::time::Instant)>>,
    /// The menu bar's last answer, logged when it changes.
    #[cfg(target_os = "macos")]
    answer: std::cell::Cell<Option<bool>>,
    /// The thread that asks the shell about the icon and sets its tip (#747: never the window's).
    #[cfg(windows)]
    watch: watch::Watch,
    /// The last answer the window took from it, for a frame where the watcher is busy writing
    /// the next.
    #[cfg(windows)]
    seen: std::cell::Cell<Option<watch::Seen>>,
}

/// A menu entry whose text is translated.
enum Entry {
    Item(MenuItem),
    Check(CheckMenuItem),
    Sub(Submenu),
}

impl Tray {
    /// The icon and its menu; events wake `ctx`. None if the shell refuses the icon (logged).
    /// Linux makes it beside the window instead: [`Setup`].
    #[cfg(not(target_os = "linux"))]
    pub fn new(ctx: &egui::Context, limiter: bool) -> Option<Tray> {
        let (tx, rx) = channel();
        Some(Tray::with(Native::new(ctx, limiter, tx)?, rx))
    }

    fn with(native: Held, rx: Receiver<TrayCmd>) -> Tray {
        Tray {
            native,
            rx,
            tip: Default::default(),
            queue_running: std::cell::Cell::new(None),
            shown_downloads: Default::default(),
        }
    }

    /// Run `job` on the icon: here, or on its own thread on Linux (never waiting for it there).
    fn at(&self, job: impl FnOnce(&Native) + Send + 'static) {
        #[cfg(not(target_os = "linux"))]
        job(&self.native);
        #[cfg(target_os = "linux")]
        self.native.run(job);
    }

    /// What the shell says of the icon: [`Native::icon`].
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn icon(&self) -> Icon {
        self.native.icon()
    }

    /// [`Native::icon_now`].
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn icon_now(&self) -> Icon {
        self.native.icon_now()
    }

    /// [`Native::rescue`].
    #[cfg(windows)]
    pub fn rescue(&self) -> bool {
        self.native.rescue()
    }

    /// The commands since the last call.
    pub fn poll(&self) -> Vec<TrayCmd> {
        self.rx.try_iter().collect()
    }

    /// Translate the menu again (the interface language changed).
    pub fn relabel(&self) {
        self.at(Native::relabel);
    }

    /// Keep Start / Stop main download queue in step with the queue (the menu is only told
    /// when it changes).
    pub fn set_queue(&self, running: bool) {
        if self.queue_running.replace(Some(running)) != Some(running) {
            self.at(move |n| n.set_queue(running));
        }
    }

    /// The icon's tip: the app's name, and while downloads run how many and how fast.
    pub fn set_tooltip(&self, tip: &str) {
        // The shell draws it: without isolates (#919).
        let tip = crate::i18n::os_text(tip);
        let tip = tip.as_str();
        let mut last = self.tip.borrow_mut();
        let due = last
            .1
            .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(1));
        if last.0 != tip && due {
            // Windows: the watcher's thread sets it, as setting it waits for Explorer (#747).
            #[cfg(windows)]
            if !self.native.tip(tip) {
                return;
            }
            #[cfg(not(windows))]
            {
                let tip = tip.to_owned();
                self.at(move |n| {
                    n.tip(&tip);
                });
            }
            *last = (tip.to_owned(), Some(std::time::Instant::now()));
        }
    }

    /// "Downloads in progress": the running downloads (id, name), the menu rebuilt only when they
    /// change; greyed when none runs.
    pub fn set_downloads(&self, running: &[(u64, String)]) {
        if *self.shown_downloads.borrow() == running {
            return;
        }
        let running = running.to_vec();
        *self.shown_downloads.borrow_mut() = running.clone();
        self.at(move |n| n.set_downloads(&running));
    }

    /// Keep the Speed Limiter checks in step with the app.
    pub fn set_limiter(&self, on: bool) {
        self.at(move |n| n.set_limiter(on));
    }
}

/// The icon as [`Tray`] holds it: itself (Windows, macOS), or the thread that keeps it (Linux).
#[cfg(not(target_os = "linux"))]
type Held = Native;
#[cfg(target_os = "linux")]
type Held = beside::Beside<Native>;

/// Linux's tray icon on its way (#828): made on a thread of its own, as its library waits for the
/// session bus, for ever where none answers. Until it is made the window behaves as with no tray
/// host; [`Setup::poll`] hands it over once it is, however late.
#[cfg(target_os = "linux")]
pub struct Setup {
    native: Option<beside::Beside<Native>>,
    rx: Option<Receiver<TrayCmd>>,
    started: std::time::Instant,
    /// The interface language the menu was made in, to translate it again if it changed since.
    lang: &'static str,
    /// The phase last seen, to log each change once.
    told: beside::Phase,
}

/// What [`Setup::poll`] found.
#[cfg(target_os = "linux")]
pub enum Came {
    /// Not yet: the window behaves as with no tray host.
    Waiting,
    /// The icon, made.
    Up(Tray),
    /// No icon this run (logged).
    Failed,
}

#[cfg(target_os = "linux")]
impl Setup {
    /// Start making the icon and its menu beside the window; its events, and its coming, wake
    /// `ctx`.
    pub fn start(ctx: &egui::Context, limiter: bool) -> Setup {
        let (tx, rx) = channel();
        let made = ctx.clone();
        let wake = ctx.clone();
        let native = beside::Beside::start(
            "tray icon",
            move || Native::new(&made, limiter, tx),
            move || wake.request_repaint(),
        );
        // To log a long wait once it is long, even with nothing else to draw.
        ctx.request_repaint_after(beside::WAIT);
        Setup {
            native: Some(native),
            rx: Some(rx),
            started: std::time::Instant::now(),
            lang: crate::i18n::code(),
            told: beside::Phase::Waiting,
        }
    }

    /// Whether the icon is made. Never waits for it.
    pub fn poll(&mut self) -> Came {
        let Some(native) = &self.native else {
            return Came::Failed;
        };
        let waited = self.started.elapsed();
        let phase = beside::phase(native.made(), waited);
        let was = std::mem::replace(&mut self.told, phase);
        match phase {
            beside::Phase::Waiting => Came::Waiting,
            beside::Phase::GaveUp => {
                if was != phase {
                    tracing::warn!(
                        "tray icon: the session bus hasn't answered in {} s: carrying on without the icon (closing quits) until it does",
                        beside::WAIT.as_secs()
                    );
                }
                Came::Waiting
            }
            beside::Phase::Failed => {
                self.native = None;
                Came::Failed
            }
            beside::Phase::Up => {
                let (Some(native), Some(rx)) = (self.native.take(), self.rx.take()) else {
                    return Came::Failed;
                };
                if was == beside::Phase::GaveUp {
                    tracing::info!(
                        "tray icon: the session bus answered late, {} ms on",
                        waited.as_millis()
                    );
                } else {
                    tracing::info!("tray icon: made in {} ms", waited.as_millis());
                }
                let tray = Tray::with(native, rx);
                if crate::i18n::code() != self.lang {
                    tray.relabel();
                }
                Came::Up(tray)
            }
        }
    }
}

impl Native {
    /// What the shell says of the icon, as the window may act on it: closing, Minimize to the
    /// notification area and the start with the session ask it every frame. Windows: the last
    /// answer of the watcher's thread, never waiting for the shell; macOS: the menu bar item's
    /// frame (AppKit's, asked on the window's thread), a second old at most. Linux has no such
    /// question: its [`available`] stands for it.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn icon(&self) -> Icon {
        if let Some(v) = test_visible() {
            return v;
        }
        #[cfg(windows)]
        return watch::icon(self.seen(), std::time::Instant::now());
        #[cfg(target_os = "macos")]
        {
            if let Some((icon, checked)) = self.visible_checked.get()
                && checked.elapsed() < std::time::Duration::from_secs(1)
            {
                return icon;
            }
            self.icon_now()
        }
    }

    /// [`Tray::icon`], for closing: on macOS without the one-second cache (an icon gone since must
    /// not hide the window); on Windows the same last answer, whose age it weighs.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn icon_now(&self) -> Icon {
        if let Some(v) = test_visible() {
            return v;
        }
        #[cfg(windows)]
        return self.icon();
        #[cfg(target_os = "macos")]
        {
            // MDM_TEST_TRAY_RECT=0: the menu bar gives the item no place.
            let placed = self.tray.rect().is_some()
                && std::env::var_os("MDM_TEST_TRAY_RECT").is_none_or(|v| v != "0");
            if self.answer.replace(Some(placed)) != Some(placed) {
                tracing::info!("tray icon: placed {placed}");
            }
            let icon = if placed { Icon::There } else { Icon::Gone };
            self.visible_checked
                .set(Some((icon, std::time::Instant::now())));
            icon
        }
    }

    /// Whether a window hidden in the notification area should come back to the taskbar: the
    /// shell, running and answering, has said for some seconds that it has no icon of MDM's
    /// (`watch::rescue`). Never on no answer. Windows only: macOS leaves a window hidden in the
    /// menu bar as it was.
    #[cfg(windows)]
    pub fn rescue(&self) -> bool {
        test_visible().is_none() && watch::rescue(self.seen(), std::time::Instant::now())
    }

    /// The watcher's last answer, or the one the window took before while it is busy writing.
    #[cfg(windows)]
    fn seen(&self) -> Option<watch::Seen> {
        if let Some(seen) = self.watch.latest() {
            self.seen.set(Some(seen));
        }
        self.seen.get()
    }

    /// The icon and its menu; events go to `tx` and wake `ctx`. None if the shell refuses the icon
    /// (logged).
    fn new(ctx: &egui::Context, limiter: bool, tx: Sender<TrayCmd>) -> Option<Native> {
        let menu = Menu::new();
        let mut items = Vec::new();
        let mut labels = Vec::new();
        // Labels are translated here and again by `relabel` when the language changes.
        let mut add = |m: &Menu, label: &'static str, cmd: TrayCmd| {
            let item = MenuItem::new(crate::i18n::t(label), true, None);
            items.push((item.id().clone(), cmd));
            m.append(&item).ok();
            labels.push((Entry::Item(item.clone()), label));
            item
        };
        add(
            &menu,
            crate::i18n::mark("Open Mister Download Manager"),
            TrayCmd::Show,
        );
        menu.append(&PredefinedMenuItem::separator()).ok();
        let mark = crate::i18n::mark;
        add(&menu, mark("Add new download"), TrayCmd::AddUrl);
        add(&menu, mark("Add batch download"), TrayCmd::Batch);
        // i18n: tray menu: the links copied to the clipboard, to tick in Link grabber (as Tasks → Add batch download from clipboard)
        add(
            &menu,
            mark("Add batch download from clipboard"),
            TrayCmd::BatchClipboard,
        );
        // i18n: tray menu: a submenu of the downloads now running; choosing one shows its progress window
        let downloads_label = mark("Downloads in progress");
        let downloads = Submenu::new(crate::i18n::t(downloads_label), false);
        menu.append(&downloads).ok();
        menu.append(&PredefinedMenuItem::separator()).ok();
        let limiter_label = mark("Speed Limiter");
        let limiter_menu = Submenu::new(crate::i18n::t(limiter_label), true);
        // i18n: verb: turn the Speed Limiter on
        let on_label = mark("Turn on");
        // i18n: verb: turn the Speed Limiter off
        let off_label = mark("Turn off");
        let limiter_on = CheckMenuItem::new(crate::i18n::t(on_label), true, limiter, None);
        let limiter_off = CheckMenuItem::new(crate::i18n::t(off_label), true, !limiter, None);
        limiter_menu.append(&limiter_on).ok();
        limiter_menu.append(&limiter_off).ok();
        menu.append(&limiter_menu).ok();
        // i18n: noun: opens the Scheduler (the tool that starts queues on time)
        add(&menu, mark("Scheduler"), TrayCmd::Scheduler);
        let start_queue = add(
            &menu,
            mark("Start main download queue"),
            TrayCmd::StartQueue,
        );
        let stop_queue = add(&menu, mark("Stop main download queue"), TrayCmd::StopQueue);
        // i18n: plural noun: opens the Options window (settings)
        add(&menu, mark("Options"), TrayCmd::Options);
        menu.append(&PredefinedMenuItem::separator()).ok();
        // i18n: verb: quit the app
        add(&menu, mark("Exit"), TrayCmd::Exit);
        drop(add);
        items.push((limiter_on.id().clone(), TrayCmd::LimiterOn));
        items.push((limiter_off.id().clone(), TrayCmd::LimiterOff));

        let icon = match picture() {
            Ok(icon) => icon,
            Err(e) => {
                tracing::warn!("tray icon: no picture: {e}");
                return None;
            }
        };
        let tray = match TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip(crate::MAIN_TITLE)
            .with_icon(icon)
            .build()
        {
            Ok(tray) => tray,
            Err(e) => {
                tracing::warn!("tray icon: not made: {e}");
                return None;
            }
        };

        // Only the hidden window's handle crosses to the watcher's thread: TrayIcon stays here.
        #[cfg(windows)]
        let watch = {
            let wake = ctx.clone();
            watch::Watch::new(
                WindowsShell {
                    hwnd: tray.window_handle() as isize,
                    id: None,
                    // MDM_TEST_TRAY_RECT=0: the shell gives the icon no place, as the reporters'
                    // did for one behind ^ (the close test, with a real icon the shell holds).
                    rect_withheld: std::env::var_os("MDM_TEST_TRAY_RECT").is_some_and(|v| v == "0"),
                },
                move || wake.request_repaint(),
            )
        };

        let wake = |tx: &Sender<TrayCmd>, ctx: &egui::Context| (tx.clone(), ctx.clone());
        let (icon_tx, icon_ctx) = wake(&tx, ctx);
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                let _ = icon_tx.send(TrayCmd::Toggle);
                icon_ctx.request_repaint();
            }
        }));
        let (menu_tx, menu_ctx) = wake(&tx, ctx);
        let download_ids: Arc<Mutex<Vec<(MenuId, u64)>>> = Arc::default();
        let ids = download_ids.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let cmd = items
                .iter()
                .find(|(id, _): &&(MenuId, TrayCmd)| *id == e.id)
                .map(|(_, c)| *c)
                .or_else(|| {
                    ids.lock()
                        .ok()?
                        .iter()
                        .find(|(id, _)| *id == e.id)
                        .map(|(_, d)| TrayCmd::ShowDownload(*d))
                });
            if let Some(cmd) = cmd {
                let _ = menu_tx.send(cmd);
                menu_ctx.request_repaint();
            }
        }));
        labels.push((Entry::Sub(limiter_menu), limiter_label));
        labels.push((Entry::Sub(downloads.clone()), downloads_label));
        labels.push((Entry::Check(limiter_on.clone()), on_label));
        labels.push((Entry::Check(limiter_off.clone()), off_label));
        Some(Native {
            tray,
            limiter_on,
            limiter_off,
            start_queue,
            stop_queue,
            downloads,
            download_ids,
            labels,
            #[cfg(target_os = "macos")]
            visible_checked: Default::default(),
            #[cfg(target_os = "macos")]
            answer: Default::default(),
            #[cfg(windows)]
            watch,
            #[cfg(windows)]
            seen: Default::default(),
        })
    }

    /// Translate the menu again (the interface language changed).
    fn relabel(&self) {
        for (entry, source) in &self.labels {
            let text = crate::i18n::t(source);
            match entry {
                Entry::Item(i) => i.set_text(text),
                Entry::Check(i) => i.set_text(text),
                Entry::Sub(i) => i.set_text(text),
            }
        }
    }

    /// Start / Stop main download queue, as the queue runs.
    fn set_queue(&self, running: bool) {
        self.start_queue.set_enabled(!running);
        self.stop_queue.set_enabled(running);
    }

    /// Set the icon's tip; false if the shell didn't take it (Windows: the watcher was busy, asked
    /// again at the next frame).
    fn tip(&self, tip: &str) -> bool {
        #[cfg(windows)]
        return self.watch.tip(tip);
        #[cfg(not(windows))]
        {
            let _ = self.tray.set_tooltip(Some(tip));
            true
        }
    }

    /// "Downloads in progress": the running downloads (id, name); greyed when none runs.
    fn set_downloads(&self, running: &[(u64, String)]) {
        for _ in 0..self.downloads.items().len() {
            let _ = self.downloads.remove_at(0);
        }
        let mut ids = Vec::with_capacity(running.len());
        for (id, name) in running {
            let item = MenuItem::new(short(name), true, None);
            ids.push((item.id().clone(), *id));
            self.downloads.append(&item).ok();
        }
        if let Ok(mut d) = self.download_ids.lock() {
            *d = ids;
        }
        self.downloads.set_enabled(!running.is_empty());
    }

    /// The Speed Limiter checks.
    fn set_limiter(&self, on: bool) {
        self.limiter_on.set_checked(on);
        self.limiter_off.set_checked(!on);
    }
}

/// What the tests say of the icon, as off-screen tests can't make the shell answer reliably
/// (MDM_TEST_TRAY_VISIBLE: 1 there, anything else gone; the startup and close tests use it with
/// MDM_TEST_TRAY).
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn test_visible() -> Option<Icon> {
    std::env::var_os("MDM_TEST_TRAY_VISIBLE")
        .map(|v| if v == "1" { Icon::There } else { Icon::Gone })
}

/// Windows' shell, as the watcher's thread asks it: the tray's hidden window (one icon) and the
/// icon's id once found.
#[cfg(windows)]
struct WindowsShell {
    hwnd: isize,
    id: Option<u32>,
    rect_withheld: bool,
}

#[cfg(windows)]
impl watch::Port for WindowsShell {
    fn ask(&mut self) -> watch::Shell {
        use crate::win32::TrayAnswer;
        match crate::win32::tray_icon_ask(self.hwnd, self.id, self.rect_withheld) {
            TrayAnswer::NoTaskbar => watch::Shell::NoTaskbar,
            TrayAnswer::Silent => watch::Shell::Silent,
            TrayAnswer::Answered { held, placed } => {
                if held.is_some() {
                    self.id = held;
                }
                watch::Shell::Answered {
                    placed,
                    held: held.is_some(),
                }
            }
        }
    }

    fn tip(&mut self, tip: &str) -> bool {
        self.id
            .is_some_and(|id| crate::win32::tray_icon_tip(self.hwnd, id, tip))
    }
}

/// A download's name as the tray menu shows it: at most MENU_NAME_MAX characters, the rest "…";
/// "&" doubled, which menus would read as a shortcut mark.
fn short(name: &str) -> String {
    let mut s: String = name.chars().take(MENU_NAME_MAX).collect();
    if name.chars().count() > MENU_NAME_MAX {
        s.push('…');
    }
    s.replace('&', "&&")
}

/// The icon's picture: the app icon's ([`crate::app_picture`]), square. Never an empty one:
/// tray-icon takes a 0×0 picture, and macOS then fails to make the menu bar item of it (it
/// encodes the picture as a PNG, which can't be 0 wide), so before #529 macOS had no menu bar
/// icon and closing the window quit.
fn picture() -> Result<tray_icon::Icon, String> {
    let p = crate::app_picture();
    if p.width == 0 || p.width != p.height {
        return Err(format!("{}×{} px", p.width, p.height));
    }
    tray_icon::Icon::from_rgba(p.rgba.clone(), p.width, p.height).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The close decision (#750, #718, #754, #545): an icon the shell says is there, or no answer
    /// at all, hides the window; only the shell saying the icon is gone leaves it in the taskbar.
    #[test]
    fn closing_hides_unless_the_shell_says_the_icon_is_gone() {
        for icon in [Icon::There, Icon::Unknown] {
            assert_eq!(on_close(true, true, icon, true), OnClose::Hide, "{icon:?}");
        }
        assert_eq!(on_close(true, true, Icon::Gone, true), OnClose::Minimize);
    }

    /// The rest of the close decision, on every system (#457, #59).
    #[test]
    fn closing_quits_with_the_option_off_no_tray_or_nowhere_to_wait() {
        // No icon and no taskbar to wait in (Linux with no tray host): quit.
        assert_eq!(on_close(true, true, Icon::Gone, false), OnClose::Quit);
        assert_eq!(on_close(true, true, Icon::There, false), OnClose::Hide);
        // The option off, or no tray object at all: closing quits, whatever the shell says.
        for icon in [Icon::There, Icon::Unknown, Icon::Gone] {
            assert_eq!(on_close(false, true, icon, true), OnClose::Quit);
            assert_eq!(on_close(true, false, icon, true), OnClose::Quit);
        }
    }

    #[test]
    fn menu_names_are_shortened_and_keep_their_ampersands() {
        assert_eq!(short("a & b.zip"), "a && b.zip");
        let s = short(&"x".repeat(60));
        assert_eq!(s.chars().count(), MENU_NAME_MAX + 1);
        assert!(s.ends_with('…'));
    }

    /// #529: the tray's picture is a real one on every system, macOS's menu bar included (where
    /// the window icon, `app_icon`, is empty on purpose).
    #[test]
    fn the_tray_picture_is_never_empty() {
        let p = crate::app_picture();
        assert_eq!((p.width, p.height), (256, 256));
        assert_eq!(p.rgba.len(), 256 * 256 * 4);
        assert!(p.rgba.chunks(4).any(|px| px[3] > 0), "a blank picture");
        assert!(picture().is_ok());
        #[cfg(target_os = "macos")]
        assert_eq!(
            crate::app_icon().width,
            0,
            "the Dock keeps the bundle's icon"
        );
    }
}
