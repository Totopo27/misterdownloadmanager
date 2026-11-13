//! The only Windows FFI in Mister Download Manager (D-008): IDM's dialog frame for our dialog windows, and the
//! shell's own small file-type icons (the ones Explorer and IDM show — Windows' artwork, not IDM's),
//! and the drop target's OLE `IDropTarget` (`ole`).
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::mem::{size_of, zeroed};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, ReleaseDC,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Shell::{
    DefSubclassProc, SHFILEINFOW, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SetWindowSubclass,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, EnumWindows, GWL_EXSTYLE, GWL_STYLE, GetIconInfo, GetWindowLongPtrW,
    GetWindowThreadProcessId, ICONINFO, InternalGetWindowText, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, WM_NCCALCSIZE, WM_NCHITTEST,
    WS_EX_DLGMODALFRAME, WS_THICKFRAME,
};

struct Find {
    pid: u32,
    title: Vec<u16>,
    /// Where each window's title is read: as long as the title looked for, one more unit and the
    /// NUL, so a longer title reads longer, never the same. A fixed 256 units cut every title to
    /// 255, and a window named after a long file was never found (#540).
    buf: Vec<u16>,
    found: HWND,
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
    // SAFETY: lparam is the &mut Find passed to EnumWindows below, alive for the call.
    let f = unsafe { &mut *(lparam as *mut Find) };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid != f.pid {
        return 1;
    }
    // The title as the window keeps it, read without a message: GetWindowTextW sends WM_GETTEXT
    // to a window of this process, and one whose thread is busy (on a stalled disk: the file
    // icons' COM window) held the window's thread until it answered (#583).
    // SAFETY: InternalGetWindowText writes at most buf.len() units, the NUL included.
    let n = unsafe { InternalGetWindowText(hwnd, f.buf.as_mut_ptr(), f.buf.len() as i32) };
    if n > 0 && f.buf[..n as usize] == f.title[..] {
        f.found = hwnd;
        return 0;
    }
    1
}

/// The window of this thread that holds the mouse capture (a button is down in it), if any.
fn own_capture() -> Option<HWND> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetCapture;
    // SAFETY: a plain query; GetCapture only sees this thread's windows.
    let h = unsafe { GetCapture() };
    (!h.is_null()).then_some(h)
}

/// The main window of this run, once it exists ([`set_main_window`]). 0: not known yet.
static MAIN_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Whether the main window is in front: no visible window of another program covers any part
/// of it from above. A tray click asks (#520): the click itself takes the focus first, and with
/// the icon under the taskbar's ^ the focus went to the flyout well before, so focus alone
/// can't tell. The taskbar and its flyout, always on top, don't count, nor do hidden, minimized
/// or cloaked windows (another virtual desktop, a suspended app) or this program's own (its
/// dialogs over it). None: the main window isn't known yet.
pub fn main_window_in_front() -> Option<bool> {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GW_HWNDPREV, GetWindow, GetWindowRect, IsIconic, IsWindowVisible, WS_EX_TOPMOST,
    };
    let main = MAIN_WINDOW.load(std::sync::atomic::Ordering::Relaxed) as HWND;
    if main.is_null() {
        return None;
    }
    let rect = |h: HWND| {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: a read into a local; a gone window fails harmlessly.
        (unsafe { GetWindowRect(h, &mut r) } != 0 && r.right > r.left && r.bottom > r.top)
            .then_some(r)
    };
    let m = rect(main)?;
    // SAFETY: plain queries about windows, walking the z-order up from ours.
    let me = unsafe { GetCurrentProcessId() };
    let mut h = unsafe { GetWindow(main, GW_HWNDPREV) };
    // A bound, in case windows come and go under the walk.
    for _ in 0..4096 {
        if h.is_null() {
            break;
        }
        let mut pid = 0;
        // SAFETY: as above.
        let (visible, topmost) = unsafe {
            GetWindowThreadProcessId(h, &mut pid);
            (
                IsWindowVisible(h) != 0 && IsIconic(h) == 0,
                GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0,
            )
        };
        if visible && !topmost && pid != me {
            let mut cloaked: u32 = 0;
            // SAFETY: a u32 read into a local of that size.
            let known = unsafe {
                DwmGetWindowAttribute(
                    h,
                    DWMWA_CLOAKED as _,
                    (&mut cloaked as *mut u32).cast(),
                    std::mem::size_of::<u32>() as u32,
                )
            } >= 0;
            if !(known && cloaked != 0)
                && let Some(r) = rect(h)
                && r.left < m.right
                && m.left < r.right
                && r.top < m.bottom
                && m.top < r.bottom
            {
                return Some(false);
            }
        }
        // SAFETY: as above.
        h = unsafe { GetWindow(h, GW_HWNDPREV) };
    }
    Some(true)
}

/// Remember the window the app was made for: the one that stays. A way of drawing that fails
/// (Direct3D with no usable adapter, renderer.rs) leaves its own window behind under the same
/// title until the event loop next runs, and a dialog owned by that window goes down with it
/// (#337).
pub fn set_main_window(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(RawWindowHandle::Win32(h)) = cc.window_handle().map(|h| h.as_raw()) {
        MAIN_WINDOW.store(h.hwnd.get(), std::sync::atomic::Ordering::Relaxed);
    }
}

/// This process's top-level window with exactly `title`; the main window by its handle.
pub(crate) fn find_own_window(title: &str) -> Option<HWND> {
    let known = MAIN_WINDOW.load(std::sync::atomic::Ordering::Relaxed);
    own_window(
        title,
        (title == crate::MAIN_TITLE && known != 0).then_some(known as HWND),
    )
}

/// `known` while that window exists, else the top-level window of this process titled `title`.
fn own_window(title: &str, known: Option<HWND>) -> Option<HWND> {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;
    // SAFETY: a plain query; any value is accepted as the handle.
    if let Some(h) = known.filter(|h| unsafe { IsWindow(*h) } != 0) {
        return Some(h);
    }
    // As the window was given it (dialogs.rs, progress.rs): without isolates (#919).
    let title: Vec<u16> = crate::i18n::os_text(title).encode_utf16().collect();
    let mut f = Find {
        pid: unsafe { GetCurrentProcessId() },
        buf: vec![0; title.len() + 2],
        title,
        found: std::ptr::null_mut(),
    };
    unsafe { EnumWindows(Some(enum_cb), &mut f as *mut Find as LPARAM) };
    (!f.found.is_null()).then_some(f.found)
}

/// The address a connection to `dst` would leave from (PAC's `myIpAddress`), asked of the
/// routing table (GetBestRoute2). No socket is opened: a UDP socket, the usual way to ask, is a
/// listening socket to Windows' firewall, which would ask the user about MDM. Installed as the
/// engine's lookup at start (`mdm_engine::net::set_source_lookup`); unix.rs has none (the
/// engine's UDP way raises no prompt there).
pub fn source_address(dst: std::net::IpAddr) -> Option<std::net::IpAddr> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetBestRoute2, MIB_IPFORWARD_ROW2};
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, SOCKADDR_INET};
    // SAFETY: plain-data structures, zero is a valid value for each; the union is written and
    // read by its family; GetBestRoute2 only reads `dest` and writes `row` and `src`.
    unsafe {
        let mut dest: SOCKADDR_INET = zeroed();
        match dst {
            IpAddr::V4(a) => {
                dest.Ipv4.sin_family = AF_INET;
                dest.Ipv4.sin_addr.S_un.S_addr = u32::from_ne_bytes(a.octets());
            }
            IpAddr::V6(a) => {
                dest.Ipv6.sin6_family = AF_INET6;
                dest.Ipv6.sin6_addr.u.Byte = a.octets();
            }
        }
        let mut row: MIB_IPFORWARD_ROW2 = zeroed();
        let mut src: SOCKADDR_INET = zeroed();
        let r = GetBestRoute2(
            std::ptr::null(),
            0,
            std::ptr::null(),
            &dest,
            0,
            &mut row,
            &mut src,
        );
        if r != 0 {
            return None;
        }
        match src.si_family {
            AF_INET => Some(IpAddr::V4(Ipv4Addr::from(
                src.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes(),
            ))),
            AF_INET6 => Some(IpAddr::V6(Ipv6Addr::from(src.Ipv6.sin6_addr.u.Byte))),
            _ => None,
        }
    }
}

/// Off-screen test runs (MDM_OFFSCREEN=1): every top-level window of ours gets WS_EX_NOACTIVATE,
/// so Windows never makes it the foreground window, whatever asks (a click, a dialog coming to
/// the front, an accessibility client): the user's keystrokes stay in their own window while
/// tests run. UI Automation still works on them (Select, Invoke, Toggle need no activation).
/// Cheap: only our own windows are touched, once each.
pub fn keep_from_foreground() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowThreadProcessId, IsWindowVisible, WS_EX_NOACTIVATE,
    };
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
        let pid = lparam as u32;
        let mut owner = 0u32;
        // SAFETY: plain queries and a style change on a window of this process.
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut owner);
            if owner == pid && IsWindowVisible(hwnd) != 0 {
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                if ex & WS_EX_NOACTIVATE == 0 {
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex | WS_EX_NOACTIVATE) as isize);
                }
            }
        }
        1
    }
    // SAFETY: the callback only reads its pid from lparam.
    unsafe {
        EnumWindows(Some(cb), GetCurrentProcessId() as LPARAM);
    }
}

/// Our window titled `title`, just shown (dialogs::keep_theme), in front of the other programs'
/// windows if Windows didn't make it the foreground window (#208). Windows gives a program the
/// foreground only when it may take it (the user's click is in it, or the browser's native host
/// passed its right on, `AllowSetForegroundWindow`, which the user's next input elsewhere ends).
/// Refused, the window is placed under the program in front, just above our own windows, and
/// stays there: `HWND_TOP` and `BringWindowToTop` don't move it either. So a window opened at the
/// browser's request (Download File Info, a progress window) could open behind the browser. A
/// round trip through topmost puts it above every window that isn't topmost, as Download
/// complete is (#77), without taking the keyboard from the program being typed in (a click on
/// it does). None while it isn't shown yet (ask again in the next frame); else whether it was
/// lifted.
pub fn come_forward(title: &str) -> Option<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, IsIconic, IsWindowVisible, WS_EX_TOPMOST,
    };
    let hwnd = find_own_window(title)?;
    // SAFETY: plain queries about our own window.
    let (visible, foreground, topmost, minimized) = unsafe {
        (
            IsWindowVisible(hwnd) != 0,
            GetForegroundWindow() == hwnd,
            GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0,
            IsIconic(hwnd) != 0,
        )
    };
    if !visible {
        return None;
    }
    let lift = lifted_when_shown(foreground, topmost, minimized);
    if lift {
        // SAFETY: our own window, on its thread.
        unsafe { above_others(hwnd) };
    }
    Some(lift)
}

/// Whether our window titled `title` exists and is not on the screen: hidden and not minimized
/// (a window whose owner is minimized counts: Windows hides it with its owner).
pub fn window_hidden(title: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
    // SAFETY: plain queries about our own window.
    find_own_window(title).is_some_and(|h| unsafe { IsWindowVisible(h) == 0 && IsIconic(h) == 0 })
}

/// Our window titled `title` shown (it was hidden behind winit's back), not activated, and above
/// the other programs' windows ([`come_forward`]) unless off-screen.
pub fn show_hidden(title: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
    if let Some(h) = find_own_window(title) {
        // SAFETY: our own window, on its thread.
        unsafe { ShowWindow(h, SW_SHOWNOACTIVATE) };
        if !crate::offscreen() {
            come_forward(title);
        }
    }
}

/// Whether a window just shown goes above the others ([`come_forward`]): it isn't the foreground
/// window, isn't minimized, and isn't topmost already (a question, a notice: lifting it would
/// end its topmost).
fn lifted_when_shown(foreground: bool, topmost: bool, minimized: bool) -> bool {
    !foreground && !topmost && !minimized
}

/// Window `hwnd` above every window that isn't topmost, not activated, nothing else changed (its
/// size, place, frame and styles; its owner stays where it is).
///
/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn above_others(hwnd: HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOOWNERZORDER, SWP_NOSIZE,
    };
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
    // SAFETY: (the caller's) our own window; z-order changes only.
    unsafe {
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
        SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, flags);
    }
}

/// Give our window titled `title` IDM's modal-dialog frame (WS_EX_DLGMODALFRAME, no sizing
/// frame: 3 px borders, 26 px caption) and resize it so the client stays `client` points, at
/// `scale` pixels per point (the display's scaling times any zoom), with the frame measured for
/// the window's own DPI. `light` dialogs get a light caption.
/// Returns true once applied (call again each frame until it is).
pub fn dialog_frame(title: &str, client: [f32; 2], light: bool, scale: f32) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
    let Some(hwnd) = find_own_window(title) else {
        return false;
    };
    unsafe {
        // IDM's light dialogs keep a light caption even in dark mode.
        let dark: i32 = (!light).into();
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &dark as *const i32 as *const c_void,
            size_of::<i32>() as u32,
        );
        // Owned by the main window, as IDM's dialogs are: no taskbar button, kept above it.
        if let Some(main) = find_own_window(crate::MAIN_TITLE) {
            use windows_sys::Win32::UI::WindowsAndMessaging::{GWLP_HWNDPARENT, WS_EX_APPWINDOW};
            SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, main as isize);
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & !WS_EX_APPWINDOW;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & !WS_THICKFRAME;
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 | WS_EX_DLGMODALFRAME;
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: (client[0] * scale).round() as i32,
            bottom: (client[1] * scale).round() as i32,
        };
        AdjustWindowRectExForDpi(&mut rc, style, 0, ex, GetDpiForWindow(hwnd));
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            rc.right - rc.left,
            rc.bottom - rc.top,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    true
}

/// Hit-test results (winuser.h).
const HTCLIENT: LRESULT = 1;
const HTCAPTION: LRESULT = 2;
const HTSYSMENU: LRESULT = 3;

/// The system menu's "Hide to the notification area" (#63): a command id of ours (below
/// 0xF000, its low 4 bits clear, as WM_SYSCOMMAND wants).
const SC_HIDE_TO_TRAY: usize = 0x0E10;

/// Where a drawn title bar's parts are, in client pixels (egui publishes them each frame,
/// `set_caption_hits`): the caption strip's height, the icon (the system menu's place) and the
/// caption buttons, each [left, top, right, bottom].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptionHits {
    pub strip_h: i32,
    pub icon: [i32; 4],
    pub buttons: Vec<[i32; 4]>,
    /// Which of `buttons` is Maximize, answered as the system's (a resizable window, #45).
    pub max_button: Option<usize>,
}

thread_local! {
    /// Each framed window's caption parts, by HWND. Read in its subclass proc with try_borrow:
    /// WM_NCHITTEST can arrive while egui's frame holds the map (re-entered messages).
    static CAPTION_HITS: std::cell::RefCell<std::collections::HashMap<isize, CaptionHits>> =
        Default::default();
    /// Windows whose system menu's "Hide to the notification area" was chosen.
    static HIDE_ASKED: std::cell::RefCell<std::collections::HashSet<isize>> = Default::default();
}

/// Publish where window `hwnd`'s drawn title bar's parts are (client pixels), for its hit test.
pub fn set_caption_hits(hwnd: isize, hits: CaptionHits) {
    CAPTION_HITS.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut() {
            m.insert(hwnd, hits);
        }
    });
}

/// The window is gone: forget its caption parts.
pub fn forget_caption(hwnd: isize) {
    CAPTION_HITS.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut() {
            m.remove(&hwnd);
        }
    });
    HIDE_ASKED.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            s.remove(&hwnd);
        }
    });
    for set in [&ANIMATING, &FADING] {
        set.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                s.remove(&hwnd);
            }
        });
    }
}

/// Was "Hide to the notification area" chosen in window `hwnd`'s system menu (once)?
pub fn take_hide_asked(hwnd: isize) -> bool {
    HIDE_ASKED.with(|s| s.try_borrow_mut().is_ok_and(|mut s| s.remove(&hwnd)))
}

fn inside(r: &[i32; 4], x: i32, y: i32) -> bool {
    x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
}

/// What client point `(x, y)` is: our caption buttons are the client's (egui hovers and clicks
/// them), the icon is the system menu, the rest of the caption strip is the caption (the system
/// moves, snaps and shakes the window from it, double-clicks and right-clicks it), below it the
/// client's.
fn frame_hit(x: i32, y: i32, hits: &CaptionHits) -> LRESULT {
    if hits.buttons.iter().any(|b| inside(b, x, y)) {
        HTCLIENT
    } else if inside(&hits.icon, x, y) {
        HTSYSMENU
    } else if y >= 0 && y < hits.strip_h {
        HTCAPTION
    } else {
        HTCLIENT
    }
}

/// The Maximize button of a resizable window whose title bar is ours: under the pointer, or
/// pressed. Windows hit-tests it as the system's (HTMAXBUTTON), so Windows 11 shows its Snap
/// Layouts over it; the clicks are then the frame's, and so is its hover, which egui can't see:
/// the title bar reads it from here ([`max_button_state`]).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct MaxButton {
    pub hot: bool,
    pub pressed: bool,
}

thread_local! {
    static MAX_BUTTON: std::cell::RefCell<std::collections::HashMap<isize, MaxButton>> =
        Default::default();
}

/// Window `hwnd`'s Maximize button's state (see [`MaxButton`]).
pub fn max_button_state(hwnd: isize) -> MaxButton {
    MAX_BUTTON.with(|m| {
        m.try_borrow()
            .ok()
            .and_then(|m| m.get(&hwnd).copied())
            .unwrap_or_default()
    })
}

fn set_max_button(hwnd: HWND, state: MaxButton) {
    use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
    let changed = MAX_BUTTON.with(|m| {
        m.try_borrow_mut()
            .is_ok_and(|mut m| m.insert(hwnd as isize, state) != Some(state))
    });
    if changed {
        // SAFETY: our own window; a repaint is asked for (winit draws a frame).
        unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
    }
}

/// Give the resizable window titled `title` its title bar drawn by us (`on`), or the system's
/// back. Returns the window when done.
pub fn drawn_frame(title: &str, on: bool) -> Option<isize> {
    let hwnd = find_own_window(title)?;
    // Without a caption style on every version, as the progress window and the dialogs (#802):
    // on Windows 10 the caption style hidden by our WM_NCCALCSIZE put the window's picture a
    // title bar lower than its input there too, a black band above the bar and every click
    // landing above what it showed (#620).
    // SAFETY: our own window, on its thread.
    unsafe {
        if on {
            caption_less(hwnd, Drawn::Sizable);
        } else {
            caption_back(hwnd);
        }
    }
    if !on {
        forget_caption(hwnd as isize);
    }
    Some(hwnd as isize)
}

/// Give our window titled `title`, whose title bar is drawn by us (winui::caption, #63), its fixed
/// frame without a caption style ([`caption_less`], #208, #620), the client staying `client`
/// points (the drawn title bar included) at `scale` pixels per point. With `hide_item` (the
/// translated "Hide to the notification area"), the system menu gets it, Alt+Space reaching it
/// from the keyboard; None takes it out. Call while the window is still hidden, and again when
/// the size, the scale or the item changes. Returns the window, once applied.
pub fn thin_frame(
    title: &str,
    client: [f32; 2],
    scale: f32,
    hide_item: Option<&str>,
) -> Option<isize> {
    use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, DeleteMenu, GetSystemMenu, MF_BYCOMMAND, MF_STRING,
    };
    let hwnd = find_own_window(title)?;
    // Windows 11 draws its 1 px border round a window without a caption style; Windows 10 draws
    // none, so there it has the system's own (WS_BORDER).
    let kind = if windows_11() {
        Drawn::Fixed
    } else {
        Drawn::Edged
    };
    // SAFETY: our own window, on its thread; the menu text and the rectangle are locals read
    // during the calls.
    unsafe {
        caption_less(hwnd, kind);
        let menu = GetSystemMenu(hwnd, 0);
        if !menu.is_null() {
            DeleteMenu(menu, SC_HIDE_TO_TRAY as u32, MF_BYCOMMAND);
            if let Some(text) = hide_item {
                let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
                AppendMenuW(menu, MF_STRING, SC_HIDE_TO_TRAY, w.as_ptr());
            }
        }
        // The window is the client and the frame its styles describe round it, for its own DPI:
        // none on Windows 11, the 1 px border on Windows 10 (without the caption style a
        // minimized window has for its animation, [`animation_caption`]).
        let style = drawn_style(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, kind);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: (client[0] * scale).round() as i32,
            bottom: (client[1] * scale).round() as i32,
        };
        AdjustWindowRectExForDpi(&mut rc, style, 0, ex, GetDpiForWindow(hwnd).max(96));
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            rc.right - rc.left,
            rc.bottom - rc.top,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    Some(hwnd as isize)
}

/// Windows 11 (build 22000 on). Every window whose title bar is ours and that keeps its size
/// (the progress window, a style theme's dialogs) is framed without a caption style on every
/// version ([`thin_frame`]): with the caption style hidden by our WM_NCCALCSIZE, a Windows 11
/// Insider build (#208) and Windows 10 22H2 (#620) showed the window's picture where the style's
/// client would have been, a black band a title bar tall above it, every click landing above what
/// it showed. The resizable main window with a style theme is framed so too ([`drawn_frame`],
/// #802). Windows 11 colours the sizing frame's strip above its title bar ([`caption_colour`]);
/// Windows 10 can't (DWMWA_CAPTION_COLOR is Windows 11's), so there the strip, 7 px at 100 %, keeps
/// the system's title bar colour (white, the accent colour, or black in dark mode).
fn windows_11() -> bool {
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        reg_string_in(
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuild",
        )
        .and_then(|b| b.parse::<u32>().ok())
        .is_some_and(|b| b >= 22000)
    })
}

/// How a window whose title bar is ours is framed without a caption style ([`caption_less`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drawn {
    /// A fixed size (the progress window, a dialog with a style theme's title bar): a plain
    /// popup, all client, as the drop target and the menus are (#210, #231). Windows 11 draws
    /// its border, over the client's edge.
    Fixed,
    /// The same with the system's 1 px border round the client (WS_BORDER): Windows 10, which
    /// draws no border round a window without a caption style (#620).
    Edged,
    /// Resizable (the main window with a style theme): a popup with the system's sizing frame,
    /// for resizing on every edge, Aero Snap and Snap Layouts, the shadow, and maximizing.
    Sizable,
}

/// The subclass of a window framed by [`caption_less`]: its id, and its data for each kind.
const DRAWN_SUBCLASS: usize = 0x1d8;

impl Drawn {
    fn data(self) -> usize {
        match self {
            Drawn::Fixed => 0,
            Drawn::Sizable => 1,
            Drawn::Edged => 2,
        }
    }

    fn of(data: usize) -> Drawn {
        match data {
            1 => Drawn::Sizable,
            2 => Drawn::Edged,
            _ => Drawn::Fixed,
        }
    }
}

/// The style of a window whose title bar is ours, framed without a caption style (#208): a
/// popup with no caption, border or dialog frame, with the system's sizing frame when it is
/// resizable; its system menu and its Minimize and Maximize kept (the taskbar's and Alt+Space's
/// commands). With a caption style hidden by our WM_NCCALCSIZE, a Windows 11 Insider build
/// (22635) placed the window's picture where the style's client would have been: a black band a
/// title bar tall across the top of the progress window, as it did with the menus (#210) and the
/// drop target (#231). Here what the styles describe is what the window is: the system's own
/// WM_NCCALCSIZE frames it, no other reading can differ.
pub(crate) fn drawn_style(style: u32, kind: Drawn) -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{WS_BORDER, WS_CAPTION, WS_POPUP};
    // WS_CAPTION is WS_BORDER | WS_DLGFRAME: both go, and the border comes back where wanted.
    let plain = (style & !(WS_CAPTION | WS_THICKFRAME)) | WS_POPUP;
    match kind {
        Drawn::Fixed => plain,
        Drawn::Edged => plain | WS_BORDER,
        Drawn::Sizable => plain | WS_THICKFRAME,
    }
}

/// Its extended styles: without the edges that add to a frame (IDM's dialog frame among them).
/// The rest stays: a taskbar button where it had one, never-activate in an off-screen test copy.
pub(crate) fn drawn_ex_style(ex: u32) -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{WS_EX_CLIENTEDGE, WS_EX_STATICEDGE};
    ex & !(WS_EX_DLGMODALFRAME | WS_EX_CLIENTEDGE | WS_EX_STATICEDGE)
}

/// Frame our window `hwnd`, whose title bar is ours, without a caption style ([`drawn_style`]).
/// Its subclass keeps the styles when winit sets its own again (on every change of its window
/// flags) and hit-tests the drawn title bar. Again on the same window: the styles applied again.
///
/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn caption_less(hwnd: HWND, kind: Drawn) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE;
    // SAFETY: (the caller's) our own window; the proc lives as long as the program; the DWM
    // attribute is read from a local during the call.
    unsafe {
        // The subclass first: it keeps the styles below when winit sets its own again.
        SetWindowSubclass(hwnd, Some(caption_less_proc), DRAWN_SUBCLASS, kind.data());
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, drawn_ex_style(ex) as isize);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_STYLE, drawn_style(style, kind) as isize);
        // Windows 11 rounds a window with a caption style by itself, not a popup: asked for.
        let corner = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &corner as *const _ as *const c_void,
            size_of::<i32>() as u32,
        );
        // The frame measured again (WM_NCCALCSIZE), by the system.
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// The system's title bar back on a window framed by [`caption_less`] (the main window, back to
/// a theme without a title bar of its own).
///
/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn caption_back(hwnd: HWND) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_CAPTION_COLOR, DWMWA_COLOR_DEFAULT, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT,
        DwmSetWindowAttribute,
    };
    use windows_sys::Win32::UI::Shell::RemoveWindowSubclass;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SWP_NOSIZE, WS_CAPTION, WS_POPUP};
    // SAFETY: (the caller's) our own window; DWM attributes read from locals during the calls.
    unsafe {
        RemoveWindowSubclass(hwnd, Some(caption_less_proc), DRAWN_SUBCLASS);
        dwm_frame_off(hwnd, false);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_STYLE, ((style & !WS_POPUP) | WS_CAPTION) as isize);
        let corner = DWMWCP_DEFAULT;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &corner as *const _ as *const c_void,
            size_of::<i32>() as u32,
        );
        let colour = DWMWA_COLOR_DEFAULT;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR as u32,
            &colour as *const u32 as *const c_void,
            size_of::<u32>() as u32,
        );
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    CAPTION_COLOUR.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut() {
            m.remove(&(hwnd as isize));
        }
    });
    for set in [&ANIMATING, &FADING] {
        set.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                s.remove(&(hwnd as isize));
            }
        });
    }
}

thread_local! {
    /// The windows framed by [`caption_less`] that have the caption style for the length of a
    /// minimize, maximize or restore ([`animation_caption`]), by HWND.
    static ANIMATING: std::cell::RefCell<std::collections::HashSet<isize>> = Default::default();
    /// Those of them that have it for the moment they are hidden ([`fade_caption`]).
    static FADING: std::cell::RefCell<std::collections::HashSet<isize>> = Default::default();
}

fn animating(hwnd: HWND) -> bool {
    ANIMATING.with(|s| s.try_borrow().is_ok_and(|s| s.contains(&(hwnd as isize))))
}

/// The message a window framed by [`caption_less`] posts itself when it changed size or place
/// with the caption style of its animation still on: it is taken off once the window is out of
/// the taskbar ([`animation_caption`]).
fn settled_message() -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;
    static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        let name: Vec<u16> = "MDM.caption-less.settled\0".encode_utf16().collect();
        // SAFETY: a NUL-terminated string read during the call.
        unsafe { RegisterWindowMessageW(name.as_ptr()) }
    })
}

/// The caption style on (`on`) or off a window framed by [`caption_less`], for the length of the
/// system's minimize, maximize or restore (#874). Windows plays those animations only for a
/// window with a caption style; one without just pops in and out. The style is given as the
/// system's command starts and taken back as soon as the window is in its new state (minimized:
/// once it is back), and WM_NCCALCSIZE keeps the frame of the window without it the whole time
/// (the client filling the window inside its frame), so its picture never moves: a caption style
/// hidden for good is what put the black band above the title bar on Windows 10 (#620, #802).
///
/// # Safety
/// `hwnd` is a window of this thread framed by [`caption_less`] as `kind`.
unsafe fn animation_caption(hwnd: HWND, kind: Drawn, on: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsZoomed, SWP_NOSIZE, WS_CAPTION};
    if animating(hwnd) == on {
        return;
    }
    // First: the subclass's WM_STYLECHANGING keeps the caption style only while it is listed.
    ANIMATING.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            if on {
                s.insert(hwnd as isize);
            } else {
                s.remove(&(hwnd as isize));
            }
        }
    });
    // SAFETY: (the caller's) our own window, on its thread.
    unsafe {
        if on {
            dwm_frame_off(hwnd, true);
        }
        let style = drawn_style(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, kind);
        let style = if on { style | WS_CAPTION } else { style };
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        // The frame measured again: the same, by our WM_NCCALCSIZE with the caption style on, by
        // the system's without it. Maximized with the caption style, the window was placed by
        // the system for a caption's frame, a pixel wider than its own on every side: put back
        // where it is maximized without it, its client the work area.
        let maximized = (!on && kind == Drawn::Sizable && IsZoomed(hwnd) != 0)
            .then(|| maximized_to_work_area(hwnd))
            .flatten();
        let (r, place) = match maximized {
            Some((r, _)) => (r, 0),
            None => (zeroed(), SWP_NOMOVE | SWP_NOSIZE),
        };
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE | place,
        );
        if !on {
            dwm_frame_off(hwnd, false);
        }
    }
}

/// A set of windows by HWND, kept by the window thread ([`ANIMATING`], [`FADING`]).
type WindowSet = std::thread::LocalKey<std::cell::RefCell<std::collections::HashSet<isize>>>;

fn fading(hwnd: HWND) -> bool {
    FADING.with(|s| s.try_borrow().is_ok_and(|s| s.contains(&(hwnd as isize))))
}

/// The caption style on (`on`) or off a window framed by [`caption_less`] for the moment it is
/// hidden (the main window or a progress window into the tray): Windows fades a window out only
/// when it has a caption style as it goes (#874). Unlike [`animation_caption`], the style alone
/// changes: nothing is measured again, so the window's frame and picture stay as they are. Not
/// while it has the caption of a minimize, nor taken off one minimized meanwhile (its restore
/// takes it off). (Shown again, a window appears at once, with Classic too: Windows fades in
/// none of MDM's windows.)
///
/// # Safety
/// `hwnd` is a window of this thread framed by [`caption_less`] as `kind`.
unsafe fn fade_caption(hwnd: HWND, kind: Drawn, on: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, WS_CAPTION};
    if on == fading(hwnd) || (on && animating(hwnd)) {
        return;
    }
    // First: the subclass's WM_STYLECHANGING keeps the caption style only while it is listed.
    let list = |set: &'static WindowSet| {
        set.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                if on {
                    s.insert(hwnd as isize);
                } else {
                    s.remove(&(hwnd as isize));
                }
            }
        })
    };
    list(&FADING);
    // SAFETY: (the caller's) our own window, on its thread.
    unsafe {
        if !on && IsIconic(hwnd) != 0 {
            return;
        }
        list(&ANIMATING);
        if on {
            dwm_frame_off(hwnd, true);
        }
        let style = drawn_style(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, kind);
        let style = if on { style | WS_CAPTION } else { style };
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        if !on {
            dwm_frame_off(hwnd, false);
        }
    }
}

/// DWM's own frame off (`caption`) or back on a window framed by [`caption_less`] for as long as it
/// has the caption style of an animation or a fade, on Windows 10: there DWM draws the caption
/// style's title bar over the window's for those frames, a white (or accent-coloured) band above
/// MDM's own title bar (#874). With DWM's frame off, a thin edge shows instead; the window is the
/// same before and after. Windows 11 draws no such band.
///
/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn dwm_frame_off(hwnd: HWND, caption: bool) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMNCRP_USEWINDOWSTYLE, DWMWA_NCRENDERING_POLICY, DwmSetWindowAttribute,
    };
    if windows_11() {
        return;
    }
    let policy = if caption {
        DWMNCRP_DISABLED
    } else {
        DWMNCRP_USEWINDOWSTYLE
    };
    // SAFETY: (the caller's) our own window; the policy is read from a local during the call.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY as u32,
            &policy as *const _ as *const c_void,
            size_of::<i32>() as u32,
        );
    }
}

/// The frame (left, top, right, bottom) of a window framed by [`caption_less`] as `kind`, in its
/// own DPI's pixels, without the caption style of its animation ([`animation_caption`]).
///
/// # Safety
/// `hwnd` is a window of this thread.
unsafe fn drawn_frame_edges(hwnd: HWND, kind: Drawn) -> [i32; 4] {
    use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
    // SAFETY: (the caller's) our own window; the rectangle is a local.
    unsafe {
        let style = drawn_style(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, kind);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let mut f: RECT = zeroed();
        AdjustWindowRectExForDpi(&mut f, style, 0, ex, GetDpiForWindow(hwnd).max(96));
        [-f.left, -f.top, f.right, f.bottom]
    }
}

/// Minimize our window `hwnd` as its title bar's own Minimize does: by the system's command
/// (WM_SYSCOMMAND), which a window framed by [`caption_less`] animates (#874), where winit
/// minimizes it at once.
pub fn minimize(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, SC_MINIMIZE, WM_SYSCOMMAND};
    // SAFETY: a message posted to our own window, a system command it handles.
    unsafe { PostMessageW(hwnd as HWND, WM_SYSCOMMAND, SC_MINIMIZE as WPARAM, 0) };
}

/// Maximize our window `hwnd`, or restore it when it is maximized, as its title bar's own
/// Maximize does (see [`minimize`]).
pub fn toggle_maximized(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsZoomed, PostMessageW, SC_MAXIMIZE, SC_RESTORE, WM_SYSCOMMAND,
    };
    // SAFETY: a query about our own window, and a system command posted to it.
    unsafe {
        let cmd = if IsZoomed(hwnd as HWND) != 0 {
            SC_RESTORE
        } else {
            SC_MAXIMIZE
        };
        PostMessageW(hwnd as HWND, WM_SYSCOMMAND, cmd as WPARAM, 0);
    }
}

thread_local! {
    /// The colour last given to each resizable window's frame above its drawn title bar.
    static CAPTION_COLOUR: std::cell::RefCell<std::collections::HashMap<isize, u32>> =
        Default::default();
}

/// The colour of the sizing frame's strip above the main window's drawn title bar (`rgb`, the
/// top of the bar), so the two read as one bar (Windows 11 paints the frame of a window without
/// a caption style in its caption colour; Windows 10 in its own, which no app can set). Cheap:
/// DWM is asked only when it changes.
pub fn caption_colour(hwnd: isize, rgb: [u8; 3]) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CAPTION_COLOR, DwmSetWindowAttribute};
    if !windows_11() {
        return;
    }
    let colour = u32::from_le_bytes([rgb[0], rgb[1], rgb[2], 0]);
    let changed = CAPTION_COLOUR.with(|m| {
        m.try_borrow_mut()
            .is_ok_and(|mut m| m.insert(hwnd, colour) != Some(colour))
    });
    if changed {
        // SAFETY: our own window; the colour is read from a local during the call.
        unsafe {
            DwmSetWindowAttribute(
                hwnd as HWND,
                DWMWA_CAPTION_COLOR as u32,
                &colour as *const u32 as *const c_void,
                size_of::<u32>() as u32,
            );
        }
    }
}

/// The window menu (the system menu) of a window framed by [`caption_less`], opened at screen
/// point `(x, y)` as the system opens it from a caption: right-click on the title bar, a click on
/// its icon. Its items are enabled as the window's state allows; the command chosen is carried
/// out as the system's (WM_SYSCOMMAND). Alt+Space opens it through the system already.
fn window_menu(hwnd: HWND, x: i32, y: i32, sizable: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnableMenuItem, GetSystemMenu, IsIconic, IsZoomed, MF_BYCOMMAND, MF_ENABLED, MF_GRAYED,
        PostMessageW, SC_CLOSE, SC_MAXIMIZE, SC_MINIMIZE, SC_MOVE, SC_RESTORE, SC_SIZE,
        SetMenuDefaultItem, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_SYSCOMMAND,
        WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
    };
    // SAFETY: our own window and its system menu, on its thread.
    unsafe {
        let menu = GetSystemMenu(hwnd, 0);
        if menu.is_null() {
            return;
        }
        let (zoomed, iconic) = (IsZoomed(hwnd) != 0, IsIconic(hwnd) != 0);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        for (cmd, on) in [
            (SC_RESTORE, zoomed || iconic),
            (SC_MOVE, !zoomed),
            (SC_SIZE, sizable && !zoomed && !iconic),
            (SC_MINIMIZE, style & WS_MINIMIZEBOX != 0 && !iconic),
            (
                SC_MAXIMIZE,
                sizable && style & WS_MAXIMIZEBOX != 0 && !zoomed,
            ),
            (SC_CLOSE, true),
        ] {
            let flag = if on { MF_ENABLED } else { MF_GRAYED };
            EnableMenuItem(menu, cmd, MF_BYCOMMAND | flag);
        }
        SetMenuDefaultItem(menu, SC_CLOSE, 0);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            x,
            y,
            0,
            hwnd,
            std::ptr::null(),
        );
        if cmd != 0 {
            PostMessageW(hwnd, WM_SYSCOMMAND, cmd as WPARAM, 0);
        }
    }
}

/// A window framed by [`caption_less`]: it keeps its styles, the system frames it
/// (WM_NCCALCSIZE as its styles say), and its drawn title bar is hit-tested as a caption: the
/// buttons are the client's (egui), the icon the system menu's, the rest the caption (moved,
/// snapped, shaken, double-clicked by the system). A resizable one's Maximize answers as the
/// system's (Snap Layouts) and it maximizes to the work area, as a window with a caption does.
unsafe extern "system" fn caption_less_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, HTMAXBUTTON, IsIconic, IsZoomed, NCCALCSIZE_PARAMS, PostMessageW, SC_CLOSE,
        SC_MAXIMIZE, SC_MINIMIZE, SC_RESTORE, STYLESTRUCT, SWP_HIDEWINDOW, SendMessageW, WINDOWPOS,
        WM_GETMINMAXINFO, WM_NCACTIVATE, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP,
        WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_NCRBUTTONUP, WM_STYLECHANGING, WM_SYSCOMMAND,
        WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING, WS_CAPTION, WS_MINIMIZE,
    };
    const HT_MAX: LRESULT = HTMAXBUTTON as LRESULT;
    let kind = Drawn::of(data);
    let sizable = kind == Drawn::Sizable;
    let at = |lp: LPARAM| {
        (
            (lp & 0xffff) as i16 as i32,
            ((lp >> 16) & 0xffff) as i16 as i32,
        )
    };
    match msg {
        WM_STYLECHANGING if lparam != 0 => {
            // SAFETY: for WM_STYLECHANGING, lParam points to the STYLESTRUCT of the change,
            // whose styleNew the window may alter (Win32 contract).
            let s = unsafe { &mut *(lparam as *mut STYLESTRUCT) };
            match wparam as i32 {
                GWL_STYLE => {
                    // The caption style stays only for the length of an animation.
                    let caption = if animating(hwnd) { WS_CAPTION } else { 0 };
                    s.styleNew = drawn_style(s.styleNew, kind) | caption;
                }
                GWL_EXSTYLE => s.styleNew = drawn_ex_style(s.styleNew),
                _ => {}
            }
            return 0;
        }
        // With the caption style of an animation (#874): the frame of the window without it,
        // the client filling the window inside its sizing frame as before, so its picture
        // doesn't move. Minimized, the system's.
        // SAFETY: our own window.
        WM_NCCALCSIZE
            if lparam != 0
                && animating(hwnd)
                && unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32 & WS_MINIMIZE == 0 =>
        {
            // SAFETY: with wParam TRUE, lParam points to NCCALCSIZE_PARAMS, whose first
            // rectangle is the window's and becomes its client's; with FALSE, to that rectangle
            // alone (Win32 contract).
            let r = unsafe {
                if wparam != 0 {
                    &mut (*(lparam as *mut NCCALCSIZE_PARAMS)).rgrc[0]
                } else {
                    &mut *(lparam as *mut RECT)
                }
            };
            let [l, t, rt, b] = unsafe { drawn_frame_edges(hwnd, kind) };
            r.left += l;
            r.top += t;
            r.right -= rt;
            r.bottom -= b;
            return 0;
        }
        // The system's frame for these styles, nothing of ours or winit's: what any reading of
        // the styles finds.
        WM_NCCALCSIZE => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        // Minimize, maximize and restore (the title bar's buttons, the taskbar, Win+Up and
        // Win+Down, the window menu all send these) animated: the caption style for their
        // length (#874), taken back once the window is in its new state, or once it is back
        // when it went to the taskbar.
        WM_SYSCOMMAND
            if matches!(
                (wparam & 0xFFF0) as u32,
                SC_MINIMIZE | SC_MAXIMIZE | SC_RESTORE
            ) =>
        {
            // SAFETY: our own window, framed by caption_less as `kind`, on its thread.
            unsafe {
                animation_caption(hwnd, kind, true);
                let res = DefSubclassProc(hwnd, msg, wparam, lparam);
                if IsIconic(hwnd) == 0 {
                    animation_caption(hwnd, kind, false);
                }
                return res;
            }
        }
        // Hidden (into the tray): the caption style for that moment, so it fades out as a window
        // with a title bar does (#874), taken back once it is hidden.
        WM_WINDOWPOSCHANGING | WM_WINDOWPOSCHANGED if lparam != 0 => {
            // SAFETY: for both, lParam points to the WINDOWPOS of the change (Win32 contract),
            // read here.
            let flags = unsafe { (*(lparam as *const WINDOWPOS)).flags };
            if flags & SWP_HIDEWINDOW != 0 {
                // SAFETY: our own window, framed by caption_less as `kind`, on its thread.
                unsafe { fade_caption(hwnd, kind, msg == WM_WINDOWPOSCHANGING) };
            }
            if msg == WM_WINDOWPOSCHANGED && animating(hwnd) {
                // As below.
                // SAFETY: a message posted to our own window.
                unsafe { PostMessageW(hwnd, settled_message(), 0, 0) };
            }
        }
        // Back from the taskbar some other way (winit restoring it, a program's ShowWindow):
        // the caption style goes as soon as the change is over.
        WM_WINDOWPOSCHANGED if animating(hwnd) => {
            // SAFETY: a message posted to our own window.
            unsafe { PostMessageW(hwnd, settled_message(), 0, 0) };
        }
        m if m == settled_message() => {
            // SAFETY: our own window, framed by caption_less as `kind`, on its thread.
            unsafe {
                if IsIconic(hwnd) == 0 {
                    animation_caption(hwnd, kind, false);
                }
            }
            return 0;
        }
        // Activation changes repaint the non-client area: with lParam -1 the system skips that.
        WM_NCACTIVATE => return unsafe { DefSubclassProc(hwnd, msg, wparam, -1) },
        WM_SYSCOMMAND if (wparam & 0xFFF0) == SC_HIDE_TO_TRAY => {
            HIDE_ASKED.with(|s| {
                if let Ok(mut s) = s.try_borrow_mut() {
                    s.insert(hwnd as isize);
                }
            });
            return 0;
        }
        // The window menu from the title bar, which the system opens only from a caption.
        WM_NCRBUTTONUP if wparam as LRESULT == HTCAPTION || wparam as LRESULT == HTSYSMENU => {
            let (x, y) = at(lparam);
            window_menu(hwnd, x, y, sizable);
            return 0;
        }
        WM_NCLBUTTONDOWN if wparam as LRESULT == HTSYSMENU => {
            // Under the icon, as the system opens it from its own.
            let hits = CAPTION_HITS.with(|m| {
                m.try_borrow()
                    .ok()
                    .and_then(|m| m.get(&(hwnd as isize)).map(|h| h.icon))
            });
            let mut pt = match hits {
                Some(icon) => POINT {
                    x: icon[0],
                    y: icon[3],
                },
                None => POINT { x: 0, y: 0 },
            };
            // SAFETY: our own window; the point is a local.
            unsafe { windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt) };
            window_menu(hwnd, pt.x, pt.y, sizable);
            return 0;
        }
        WM_NCLBUTTONDBLCLK if wparam as LRESULT == HTSYSMENU => {
            // SAFETY: our own window, a system command it handles.
            unsafe { PostMessageW(hwnd, WM_SYSCOMMAND, SC_CLOSE as WPARAM, 0) };
            return 0;
        }
        // Maximize: pressed and released on it toggles the window (the system's own button would
        // draw itself over ours).
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if sizable && wparam as LRESULT == HT_MAX => {
            set_max_button(
                hwnd,
                MaxButton {
                    hot: true,
                    pressed: true,
                },
            );
            return 0;
        }
        WM_NCLBUTTONUP if sizable && wparam as LRESULT == HT_MAX => {
            let was = max_button_state(hwnd as isize);
            set_max_button(
                hwnd,
                MaxButton {
                    hot: true,
                    pressed: false,
                },
            );
            if was.pressed {
                // SAFETY: our own window, a system command it handles.
                let zoomed = unsafe { IsZoomed(hwnd) } != 0;
                let cmd = if zoomed { SC_RESTORE } else { SC_MAXIMIZE };
                unsafe { SendMessageW(hwnd, WM_SYSCOMMAND, cmd as WPARAM, 0) };
            }
            return 0;
        }
        WM_NCMOUSEMOVE if sizable => {
            let on = wparam as LRESULT == HT_MAX;
            let was = max_button_state(hwnd as isize);
            set_max_button(
                hwnd,
                MaxButton {
                    hot: on,
                    pressed: was.pressed && on,
                },
            );
            if on {
                let mut t = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: a plain call with a filled struct, our own window.
                unsafe { TrackMouseEvent(&mut t) };
            }
        }
        WM_NCMOUSELEAVE if sizable => set_max_button(hwnd, MaxButton::default()),
        WM_GETMINMAXINFO if sizable && lparam != 0 => {
            let res = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            // SAFETY: for WM_GETMINMAXINFO, lParam points to the MINMAXINFO to fill.
            let info = unsafe {
                &mut *(lparam as *mut windows_sys::Win32::UI::WindowsAndMessaging::MINMAXINFO)
            };
            if let Some((r, m)) = maximized_to_work_area(hwnd) {
                info.ptMaxPosition = POINT {
                    x: r.left - m.x,
                    y: r.top - m.y,
                };
                info.ptMaxSize = POINT {
                    x: r.right - r.left,
                    y: r.bottom - r.top,
                };
            }
            return res;
        }
        _ => {}
    }
    let res = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    if msg == WM_NCHITTEST && res == HTCLIENT {
        let (x, y) = at(lparam);
        let mut pt = POINT { x, y };
        // SAFETY: our own window; the point is a local.
        unsafe { ScreenToClient(hwnd, &mut pt) };
        let hit = CAPTION_HITS.with(|m| {
            m.try_borrow().ok().and_then(|m| {
                m.get(&(hwnd as isize)).map(|h| {
                    if sizable
                        && h.max_button
                            .and_then(|i| h.buttons.get(i))
                            .is_some_and(|b| inside(b, pt.x, pt.y))
                    {
                        HT_MAX
                    } else {
                        frame_hit(pt.x, pt.y, h)
                    }
                })
            })
        });
        return hit.unwrap_or(HTCLIENT);
    }
    res
}

/// Where window `hwnd` is when maximized, on the screen: its monitor's work area (not over the
/// taskbar), with its sizing frame (without the caption style it has while it maximizes, #874)
/// beyond the work area's edges, as a window with a caption style is maximized (Windows maximizes
/// one without a caption over the whole monitor). And its monitor's top left, from which
/// WM_GETMINMAXINFO counts.
fn maximized_to_work_area(hwnd: HWND) -> Option<(RECT, windows_sys::Win32::Foundation::POINT)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    // SAFETY: plain queries about our own window and its monitor, into locals.
    unsafe {
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if mon.is_null() || GetMonitorInfoW(mon, &mut mi) == 0 {
            return None;
        }
        let (w, m) = (mi.rcWork, mi.rcMonitor);
        let [l, t, r, b] = drawn_frame_edges(hwnd, Drawn::Sizable);
        Some((
            RECT {
                left: w.left - l,
                top: w.top - t,
                right: w.right + r,
                bottom: w.bottom + b,
            },
            POINT {
                x: m.left,
                y: m.top,
            },
        ))
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    #[test]
    fn the_drawn_title_bar_hits_as_caption_icon_and_buttons() {
        // 548 × 32 points at 1.5 px per point, 4 buttons of 46 at the end, the icon at the start.
        let h = CaptionHits {
            strip_h: 48,
            icon: [0, 0, 48, 48],
            buttons: (0..4)
                .map(|i| {
                    let x1 = 822 - i * 69;
                    [x1 - 69, 0, x1, 48]
                })
                .collect(),
            max_button: None,
        };
        assert_eq!(frame_hit(10, 10, &h), HTSYSMENU);
        assert_eq!(frame_hit(300, 10, &h), HTCAPTION);
        assert_eq!(frame_hit(821, 10, &h), HTCLIENT);
        assert_eq!(frame_hit(822 - 4 * 69, 10, &h), HTCLIENT);
        assert_eq!(frame_hit(822 - 4 * 69 - 1, 10, &h), HTCAPTION);
        // Below the bar: the client's.
        assert_eq!(frame_hit(300, 48, &h), HTCLIENT);
        // Nothing published yet: everything the client's (frame_hit isn't asked then).
        assert_eq!(frame_hit(300, 10, &CaptionHits::default()), HTCLIENT);
    }
}

#[cfg(test)]
mod caption_less_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, CreateWindowExW, DestroyWindow, DispatchMessageW, GetClientRect,
        GetWindowRect, MINMAXINFO, MSG, PM_REMOVE, PeekMessageW, SWP_NOSIZE, SendMessageW,
        WM_GETMINMAXINFO, WS_BORDER, WS_CAPTION, WS_EX_APPWINDOW, WS_POPUP,
    };

    /// The styles winit gives the progress window (a caption, a sizing frame trimmed by us, a
    /// system menu, Minimize; 0x14CE0000 less WS_VISIBLE) and the main window (Maximize too), and
    /// their extended styles (a window edge, an app window: a taskbar button).
    const WINIT_PROGRESS: u32 = 0x04CE_0000;
    const WINIT_MAIN: u32 = 0x04CF_0000;
    const WINIT_EX: u32 = 0x0004_0100;

    /// The frame (left, top, right, bottom) the styles alone put round a client: what a reader
    /// of the styles, rather than of the window's WM_NCCALCSIZE, takes for it.
    fn styles_frame(style: u32, ex: u32) -> [i32; 4] {
        let mut rc: RECT = unsafe { zeroed() };
        // SAFETY: plain computation on a local rectangle.
        unsafe { AdjustWindowRectEx(&mut rc, style, 0, ex) };
        [-rc.left, -rc.top, rc.right, rc.bottom]
    }

    #[test]
    fn a_drawn_title_bar_s_window_has_no_caption_style() {
        // The caption style the band came from (#208): 31 px above the client, by the styles.
        assert!(styles_frame(WINIT_PROGRESS, WINIT_EX)[1] > 20);
        let fixed = drawn_style(WINIT_PROGRESS, Drawn::Fixed);
        assert_eq!(fixed & WS_CAPTION, 0);
        assert_eq!(fixed & WS_THICKFRAME, 0);
        assert_ne!(fixed & WS_POPUP, 0);
        // The system menu and Minimize stay (Alt+Space, the taskbar's commands).
        assert_eq!(fixed & 0x000A_0000, 0x000A_0000);
        assert_eq!(styles_frame(fixed, drawn_ex_style(WINIT_EX)), [0, 0, 0, 0]);
        // Windows 10's (#620): the same with the system's 1 px border, the same on every side.
        let edged = drawn_style(WINIT_PROGRESS, Drawn::Edged);
        assert_eq!(edged & WS_CAPTION, WS_BORDER);
        assert_eq!(edged & WS_THICKFRAME, 0);
        assert_ne!(edged & WS_POPUP, 0);
        assert_eq!(edged & 0x000A_0000, 0x000A_0000);
        assert_eq!(styles_frame(edged, drawn_ex_style(WINIT_EX)), [1, 1, 1, 1]);
        // Resizable: the sizing frame, the same on every side (none taller at the top).
        let sizable = drawn_style(WINIT_MAIN, Drawn::Sizable);
        assert_eq!(sizable & WS_CAPTION, 0);
        assert_ne!(sizable & WS_THICKFRAME, 0);
        assert_eq!(sizable & 0x000B_0000, 0x000B_0000);
        let [l, t, r, b] = styles_frame(sizable, WINIT_EX);
        assert!(l > 0 && l == t && t == r && r == b, "{l} {t} {r} {b}");
        // Applied again (winit setting its own styles once more), nothing changes.
        assert_eq!(drawn_style(fixed, Drawn::Fixed), fixed);
        assert_eq!(drawn_style(edged, Drawn::Edged), edged);
        for kind in [Drawn::Fixed, Drawn::Edged, Drawn::Sizable] {
            assert_eq!(Drawn::of(kind.data()), kind);
        }
        assert_eq!(drawn_style(sizable, Drawn::Sizable), sizable);
        // IDM's dialog frame goes; the taskbar button stays.
        let ex = drawn_ex_style(WINIT_EX | WS_EX_DLGMODALFRAME);
        assert_eq!(ex & WS_EX_DLGMODALFRAME, 0);
        assert_ne!(ex & WS_EX_APPWINDOW, 0);
    }

    /// A real (hidden, off-screen) window with winit's styles, framed by caption_less: its client
    /// is where its styles say, it keeps its styles when they are set again (winit does on every
    /// change of its flags), its edges size it (resizable) or don't (fixed), and maximized it
    /// fills its monitor's work area, not the whole monitor.
    #[test]
    fn caption_less_frames_a_real_window_as_its_styles_say() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{HTBOTTOMRIGHT, HTLEFT, HTTOP};
        for (n, (style, kind)) in [
            (WINIT_PROGRESS, Drawn::Fixed),
            (WINIT_PROGRESS, Drawn::Edged),
            (WINIT_MAIN, Drawn::Sizable),
        ]
        .into_iter()
        .enumerate()
        {
            let title = format!("mdm caption-less test {n} {}", std::process::id());
            let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
            // A class of its own whose procedure is the system's (a STATIC control answers
            // every hit test as transparent).
            let class: Vec<u16> = "mdm-caption-less-test\0".encode_utf16().collect();
            // SAFETY: a class registered with the system's own procedure, and a hidden window
            // of it on this thread, destroyed below; the strings are NUL-terminated; plain
            // queries and messages to it.
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    DefWindowProcW, RegisterClassW, WNDCLASSW,
                };
                let mut wc: WNDCLASSW = zeroed();
                wc.lpfnWndProc = Some(DefWindowProcW);
                wc.lpszClassName = class.as_ptr();
                // Registered once for both windows (the second call fails: already there).
                RegisterClassW(&wc);
                let hwnd = CreateWindowExW(
                    WINIT_EX,
                    class.as_ptr(),
                    wide.as_ptr(),
                    style,
                    -20000,
                    -20000,
                    548,
                    444,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                assert!(!hwnd.is_null());
                caption_less(hwnd, kind);
                let st = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                assert_eq!(st, drawn_style(style, kind));
                // winit setting its caption style again: the subclass keeps ours.
                SetWindowLongPtrW(hwnd, GWL_STYLE, (st | WS_CAPTION) as isize);
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, st);
                // The client is where the styles alone put it: no reading of them can differ.
                let (mut w, mut c): (RECT, RECT) = (zeroed(), zeroed());
                GetWindowRect(hwnd, &mut w);
                GetClientRect(hwnd, &mut c);
                let mut at = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
                windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut at);
                let [l, t, r, b] = styles_frame(st, ex);
                assert_eq!([at.x - w.left, at.y - w.top], [l, t]);
                assert_eq!(
                    [c.right, c.bottom],
                    [w.right - w.left - l - r, w.bottom - w.top - t - b]
                );
                // Its edges: the system's sizing edges where it is resizable, else the client's
                // (the progress window keeps its size).
                let hit = |x: i32, y: i32| {
                    SendMessageW(
                        hwnd,
                        WM_NCHITTEST,
                        0,
                        ((y as u16 as u32) << 16 | x as u16 as u32) as LPARAM,
                    )
                };
                let mid = (w.top + w.bottom) / 2;
                let sizable = kind == Drawn::Sizable;
                assert_eq!(
                    hit(w.left + 1, mid),
                    if sizable { HTLEFT as LRESULT } else { HTCLIENT }
                );
                assert_eq!(
                    hit((w.left + w.right) / 2, w.top + 1),
                    if sizable { HTTOP as LRESULT } else { HTCLIENT }
                );
                assert_eq!(
                    hit(w.right - 2, w.bottom - 2),
                    if sizable {
                        HTBOTTOMRIGHT as LRESULT
                    } else {
                        HTCLIENT
                    }
                );
                // Resized, the client follows as the styles say.
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    700,
                    500,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                GetClientRect(hwnd, &mut c);
                assert_eq!([c.right, c.bottom], [700 - l - r, 500 - t - b]);
                // Minimizing, maximizing or restoring (#874): the caption style for the length of
                // the animation, and the window framed as without it, its client in place.
                animation_caption(hwnd, kind, true);
                let on = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
                assert_eq!(on, st | WS_CAPTION);
                // winit setting its own styles again: it stays while the animation lasts.
                SetWindowLongPtrW(hwnd, GWL_STYLE, st as isize);
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, on);
                let framed = |hwnd| {
                    let (mut w, mut c): (RECT, RECT) = (zeroed(), zeroed());
                    GetWindowRect(hwnd, &mut w);
                    GetClientRect(hwnd, &mut c);
                    let mut at = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
                    windows_sys::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut at);
                    [
                        at.x - w.left,
                        at.y - w.top,
                        w.right - w.left - c.right,
                        w.bottom - w.top - c.bottom,
                    ]
                };
                assert_eq!(framed(hwnd), [l, t, l + r, t + b]);
                // Maximized (asked, not done: that would show it): its monitor's work area, with
                // the caption style of the animation on as without it.
                if sizable {
                    let mut info: MINMAXINFO = zeroed();
                    SendMessageW(
                        hwnd,
                        WM_GETMINMAXINFO,
                        0,
                        &mut info as *mut MINMAXINFO as LPARAM,
                    );
                    // The work area with the sizing frame beyond its edges, from the monitor's
                    // top left.
                    use windows_sys::Win32::Graphics::Gdi::{
                        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
                    };
                    let mut mi: MONITORINFO = zeroed();
                    mi.cbSize = size_of::<MONITORINFO>() as u32;
                    GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi);
                    let (wa, m) = (mi.rcWork, mi.rcMonitor);
                    assert_eq!(
                        [info.ptMaxPosition.x, info.ptMaxPosition.y],
                        [wa.left - m.left - l, wa.top - m.top - t]
                    );
                    assert_eq!(
                        [info.ptMaxSize.x, info.ptMaxSize.y],
                        [wa.right - wa.left + l + r, wa.bottom - wa.top + t + b]
                    );
                }
                // Moved or sized with it on (a window back from the taskbar): it goes once the
                // window has settled, by the message the window posts itself; nothing else moves.
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    -19990,
                    -19990,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, on);
                assert_eq!(framed(hwnd), [l, t, l + r, t + b]);
                let mut msg: MSG = zeroed();
                while PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE) != 0 {
                    DispatchMessageW(&msg);
                }
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, st);
                assert!(!animating(hwnd));
                assert_eq!(framed(hwnd), [l, t, l + r, t + b]);
                // Hidden (#874: into the tray, so Windows fades it out): the caption style from
                // the change's start, kept if winit sets its styles meanwhile, the frame
                // untouched, gone once it is hidden. Shown, or moved, nothing changes. (The
                // messages of the change, sent as the system sends them: the test window itself
                // never shows.)
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    SWP_HIDEWINDOW, SWP_SHOWWINDOW, WINDOWPOS, WM_WINDOWPOSCHANGED,
                    WM_WINDOWPOSCHANGING,
                };
                for change in [SWP_HIDEWINDOW, SWP_SHOWWINDOW, 0] {
                    let mut pos: WINDOWPOS = zeroed();
                    pos.hwnd = hwnd;
                    pos.flags = change | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;
                    let lp = &mut pos as *mut WINDOWPOS as LPARAM;
                    SendMessageW(hwnd, WM_WINDOWPOSCHANGING, 0, lp);
                    let during = if change == SWP_HIDEWINDOW {
                        st | WS_CAPTION
                    } else {
                        st
                    };
                    assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, during);
                    SetWindowLongPtrW(hwnd, GWL_STYLE, st as isize);
                    assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, during);
                    assert_eq!(framed(hwnd), [l, t, l + r, t + b]);
                    SendMessageW(hwnd, WM_WINDOWPOSCHANGED, 0, lp);
                    assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, st);
                    assert!(!animating(hwnd) && !fading(hwnd));
                    assert_eq!(framed(hwnd), [l, t, l + r, t + b]);
                }
                // Hidden while it sits minimized with the caption of its animation: it keeps
                // it (it goes once the window is back, as above).
                animation_caption(hwnd, kind, true);
                let mut pos: WINDOWPOS = zeroed();
                pos.hwnd = hwnd;
                pos.flags = SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER;
                let lp = &mut pos as *mut WINDOWPOS as LPARAM;
                SendMessageW(hwnd, WM_WINDOWPOSCHANGING, 0, lp);
                SendMessageW(hwnd, WM_WINDOWPOSCHANGED, 0, lp);
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, st | WS_CAPTION);
                assert!(animating(hwnd));
                animation_caption(hwnd, kind, false);
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, st);
                DestroyWindow(hwnd);
            }
        }
    }
}

#[cfg(test)]
mod find_own_window_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WNDCLASSW,
    };

    /// Our windows are found by their whole title, however long (a progress window is named
    /// after its file, #540), and never by a title that only starts the same.
    #[test]
    fn a_window_is_found_by_its_whole_title_however_long() {
        let class: Vec<u16> = "mdm-find-own-window-test\0".encode_utf16().collect();
        // SAFETY: a class registered with the system's own procedure, and hidden windows of it
        // on this thread, destroyed below; the strings are NUL-terminated.
        unsafe {
            let mut wc: WNDCLASSW = zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let make = |title: &str| {
                let t: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
                let h = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    t.as_ptr(),
                    0,
                    -20000,
                    -20000,
                    100,
                    100,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                assert!(!h.is_null());
                h
            };
            for len in [40, 255, 256, 300, 1000] {
                let head = format!("mdm find test {} {len} ", std::process::id());
                let title = format!("{head}{}", "x".repeat(len - head.len()));
                assert_eq!(title.encode_utf16().count(), len);
                // One title longer by a unit, made first: found first, were it taken for ours.
                let longer = make(&format!("{title}x"));
                let window = make(&title);
                assert_eq!(find_own_window(&title), Some(window), "{len} units");
                DestroyWindow(window);
                assert_eq!(find_own_window(&title), None, "{len} units, gone");
                assert_eq!(find_own_window(&format!("{title}x")), Some(longer));
                DestroyWindow(longer);
            }
        }
    }

    /// #919: a window is given its title without isolates, and found by the title MDM knows it
    /// by, isolates and all.
    #[test]
    fn a_window_titled_without_isolates_is_found() {
        let title = format!(
            "71% {} {}",
            crate::i18n::isolate("mdm isolate test.zip"),
            std::process::id()
        );
        let given = crate::i18n::os_text(&title);
        assert!(
            !given
                .chars()
                .any(|c| ('\u{2066}'..='\u{2069}').contains(&c))
        );
        let window = test_window(&given);
        assert_eq!(find_own_window(&title), Some(window));
        // SAFETY: the test's own window, made on this thread.
        unsafe { DestroyWindow(window) };
    }

    /// A hidden test window of this thread titled `title`, destroyed by the caller.
    fn test_window(title: &str) -> HWND {
        let class: Vec<u16> = "mdm-find-own-window-test-2\0".encode_utf16().collect();
        let t: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        // SAFETY: a class registered with the system's own procedure (again: a no-op), and a
        // hidden window of it on this thread; the strings are NUL-terminated.
        unsafe {
            let mut wc: WNDCLASSW = zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let h = CreateWindowExW(
                0,
                class.as_ptr(),
                t.as_ptr(),
                0,
                -20000,
                -20000,
                100,
                100,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!h.is_null());
            h
        }
    }

    /// #583: looking for our window doesn't wait for another thread of this process whose window
    /// isn't answering (busy, on a stalled disk): the titles are read without a message.
    #[test]
    fn a_busy_threads_window_does_not_hold_the_search() {
        let pid = std::process::id();
        let (made, window_made) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let busy = std::thread::spawn(move || {
            let h = test_window(&format!("mdm busy window {pid}"));
            made.send(()).unwrap();
            // Busy: no message is taken until the test is done.
            let _ = released.recv();
            // SAFETY: the window made above, on this thread.
            unsafe { DestroyWindow(h) };
        });
        window_made.recv().unwrap();
        let (answer, answered) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = answer.send(find_own_window(&format!("mdm no such window {pid}")).is_some());
        });
        let got = answered.recv_timeout(std::time::Duration::from_secs(3));
        release.send(()).unwrap();
        busy.join().unwrap();
        assert_eq!(
            got,
            Ok(false),
            "the search waited for the busy thread's window"
        );
    }

    /// #633: DWM is asked for a window's caption once per change, not each time it's re-asserted.
    #[test]
    fn the_caption_goes_to_dwm_only_when_it_changes() {
        use std::sync::atomic::Ordering::Relaxed;
        let title = format!("mdm caption test {}", std::process::id());
        let h = test_window(&title);
        let calls = || CAPTION_DWM_CALLS.load(Relaxed);
        let before = calls();
        for _ in 0..50 {
            assert!(caption_dark(&title, true));
        }
        assert_eq!(calls() - before, 1, "dark, asked once");
        for _ in 0..50 {
            assert!(caption_dark(&title, false));
        }
        assert_eq!(calls() - before, 2, "light, once more");
        assert!(caption_dark(&title, true));
        assert_eq!(calls() - before, 3, "dark again");
        // SAFETY: the window made above, on this thread.
        unsafe { DestroyWindow(h) };
        assert!(!caption_dark(&title, true), "gone: not found");
        assert_eq!(calls() - before, 3);
    }
}

#[cfg(test)]
mod come_forward_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GW_HWNDNEXT, GetClientRect, GetTopWindow,
        GetWindow, GetWindowRect, RegisterClassW, WNDCLASSW, WS_EX_TOPMOST,
    };

    /// Lifted only when it didn't get the foreground, and never a topmost one (a question, a
    /// notice) or a minimized one.
    #[test]
    fn only_a_window_left_behind_is_lifted() {
        assert!(lifted_when_shown(false, false, false));
        assert!(!lifted_when_shown(true, false, false));
        assert!(!lifted_when_shown(false, true, false));
        assert!(!lifted_when_shown(false, false, true));
    }

    /// Where `hwnd` is in the z-order of the top-level windows (0: the top).
    fn z(hwnd: HWND) -> usize {
        // SAFETY: plain queries walking the desktop's top-level windows.
        unsafe {
            let mut w = GetTopWindow(std::ptr::null_mut());
            let mut i = 0;
            while !w.is_null() && w != hwnd {
                w = GetWindow(w, GW_HWNDNEXT);
                i += 1;
            }
            assert!(!w.is_null(), "not a top-level window");
            i
        }
    }

    /// Two real (hidden, off-screen) windows with winit's styles for the progress window, one
    /// framed by caption_less as on Windows 11: each in turn goes above the other, neither stays
    /// topmost, and nothing else of it changes (styles, place, size, client: the frame #208's
    /// first part made stays as it was).
    #[test]
    fn above_others_lifts_a_real_window_and_changes_nothing_else() {
        const WINIT_PROGRESS: u32 = 0x04CE_0000;
        const WINIT_EX: u32 = 0x0004_0100;
        let class: Vec<u16> = "mdm-come-forward-test\0".encode_utf16().collect();
        // SAFETY: a class registered with the system's own procedure, and hidden windows of it on
        // this thread, destroyed below; the strings are NUL-terminated; plain queries, and
        // z-order changes of our own windows.
        unsafe {
            let mut wc: WNDCLASSW = zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let make = |n: u32| {
                let title: Vec<u16> = format!("mdm come-forward test {n} {}", std::process::id())
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                let h = CreateWindowExW(
                    WINIT_EX,
                    class.as_ptr(),
                    title.as_ptr(),
                    WINIT_PROGRESS,
                    -20000,
                    -20000,
                    548,
                    444,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                assert!(!h.is_null());
                h
            };
            let (a, b) = (make(0), make(1));
            caption_less(a, Drawn::Fixed);
            let state = |h: HWND| {
                let (mut w, mut c): (RECT, RECT) = (zeroed(), zeroed());
                GetWindowRect(h, &mut w);
                GetClientRect(h, &mut c);
                (
                    GetWindowLongPtrW(h, GWL_STYLE) as u32,
                    GetWindowLongPtrW(h, GWL_EXSTYLE) as u32,
                    [w.left, w.top, w.right, w.bottom],
                    [c.right, c.bottom],
                )
            };
            for (up, other) in [(a, b), (b, a), (a, b)] {
                let before = state(up);
                above_others(up);
                assert!(z(up) < z(other), "lifted above the other window");
                let after = state(up);
                assert_eq!(after.1 & WS_EX_TOPMOST, 0, "left topmost");
                assert_eq!(before, after, "only its place in the z-order changes");
            }
            DestroyWindow(a);
            DestroyWindow(b);
        }
    }
}

#[cfg(test)]
mod main_window_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WNDCLASSW,
    };

    /// #337: two windows under one title, the way a failed Direct3D start leaves them. The
    /// window the app was made for is the one found, whichever is on top; and once it is gone
    /// the search by title still answers.
    #[test]
    fn the_known_window_wins_over_its_namesake() {
        let class: Vec<u16> = "mdm-main-window-test ".encode_utf16().collect();
        let name = format!("mdm main window test {}", std::process::id());
        let title: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: a class registered with the system's own procedure, and hidden windows of it
        // on this thread, destroyed below; the strings are NUL-terminated.
        unsafe {
            let mut wc: WNDCLASSW = zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let make = || {
                let h = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    0,
                    -20000,
                    -20000,
                    100,
                    100,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                );
                assert!(!h.is_null());
                h
            };
            let (a, b) = (make(), make());
            assert_eq!(own_window(&name, Some(a)), Some(a));
            assert_eq!(own_window(&name, Some(b)), Some(b));
            assert!(own_window(&name, None).is_some());
            DestroyWindow(b);
            assert_eq!(own_window(&name, Some(b)), Some(a), "gone: found by title");
            DestroyWindow(a);
            assert_eq!(own_window(&name, Some(a)), None);
        }
    }
}

/// The main window's "Minimize to the notification area" (#63): its minimize command (title bar,
/// system menu, taskbar) asks the app to hide it instead. The app can't see the window go
/// minimized: an idle app runs no frame while minimized, so it would never know.
struct MainMinimize {
    hwnd: isize,
    ctx: Option<egui::Context>,
    enabled: bool,
    asked: bool,
}

thread_local! {
    static MAIN_MINIMIZE: std::cell::RefCell<MainMinimize> = const {
        std::cell::RefCell::new(MainMinimize { hwnd: 0, ctx: None, enabled: false, asked: false })
    };
}

unsafe extern "system" fn main_minimize_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_SYSCOMMAND;
    const SC_MINIMIZE: usize = 0xF020;
    if msg == WM_SYSCOMMAND && (wparam & 0xFFF0) == SC_MINIMIZE {
        let caught = MAIN_MINIMIZE.with(|m| {
            let Ok(mut m) = m.try_borrow_mut() else {
                return false;
            };
            if !m.enabled {
                return false;
            }
            m.asked = true;
            if let Some(ctx) = &m.ctx {
                ctx.request_repaint();
            }
            true
        });
        if caught {
            return 0;
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Keep the main window's minimize command in step with "Minimize to the notification area"
/// (`enabled`: the option is on and a tray shows MDM's icon). Call every frame (cheap once the
/// window is found). True once after the command came: hide the window now.
pub fn main_minimize_to_tray(ctx: &egui::Context, enabled: bool) -> bool {
    MAIN_MINIMIZE.with(|m| {
        let Ok(mut m) = m.try_borrow_mut() else {
            return false;
        };
        if m.hwnd == 0
            && let Some(hwnd) = find_own_window(crate::MAIN_TITLE)
        {
            // SAFETY: our own window; the proc lives as long as the program.
            unsafe { SetWindowSubclass(hwnd, Some(main_minimize_proc), 0x7a1, 0) };
            m.hwnd = hwnd as isize;
            m.ctx = Some(ctx.clone());
        }
        m.enabled = enabled;
        std::mem::take(&mut m.asked)
    })
}

/// The highest icon id [`tray_icon_ask`] tries. tray-icon gives each icon an id of its own
/// from a counter it shares with its icon names, and doesn't say which (a program's first icon
/// has 2).
const TRAY_ICON_IDS: u32 = 16;

/// Longer than the shell takes to answer about an icon: a "no" that took this long is
/// Shell_NotifyIcon giving up on a busy Explorer, not the shell's answer.
const TRAY_SLOW: std::time::Duration = std::time::Duration::from_millis(1500);

/// What the shell said of MDM's notification-area icon ([`tray_icon_ask`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAnswer {
    /// No taskbar (no `Shell_TrayWnd` window): Explorer isn't running, or a shell without a
    /// notification area is.
    NoTaskbar,
    /// The taskbar is there but didn't answer: Windows says it stopped responding, or the
    /// question took as long as a timeout.
    Silent,
    /// The shell answered: the icon's id when it holds it (wherever it shows it), and whether it
    /// gives the icon a place on screen.
    Answered { held: Option<u32>, placed: bool },
}

/// Ask the shell about the notification-area icon of `hwnd` (the tray's own hidden window, which
/// has one icon); `known` is its id once found. Both questions wait for Explorer, for seconds when
/// it is busy (#747: 8 s), so this runs on the tray's watcher thread (tray.rs), never on the
/// window's.
///
/// - Held: an empty `NIM_MODIFY` changes nothing and succeeds only for an icon the shell has, in
///   the taskbar or among the hidden icons behind ^ (`known` first, else each id up to
///   [`TRAY_ICON_IDS`]).
/// - Placed: `Shell_NotifyIconGetRect`. Not enough on its own: Windows 10 and Windows 11 builds
///   before 26200 give an icon behind ^ no place (#750, #718, #754: 0.5.2 asked only this, and
///   left the window in the taskbar with the icon there); build 26200 gives it the place of ^.
///
/// `rect_withheld` answers "no place" without asking (MDM_TEST_TRAY_RECT=0: the reporters' case,
/// on a Windows that gives one).
pub fn tray_icon_ask(hwnd: isize, known: Option<u32>, rect_withheld: bool) -> TrayAnswer {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, IsHungAppWindow};
    let hwnd = hwnd as HWND;
    if hwnd.is_null() {
        return TrayAnswer::Answered {
            held: None,
            placed: false,
        };
    }
    let class: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
    // SAFETY: a NUL-ended class name and no title: no window is sent a message.
    let taskbar = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    if taskbar.is_null() {
        return TrayAnswer::NoTaskbar;
    }
    // SAFETY: reads the window's state; never waits for it.
    if unsafe { IsHungAppWindow(taskbar) } != 0 {
        return TrayAnswer::Silent;
    }
    let start = std::time::Instant::now();
    let ids = || {
        known
            .into_iter()
            .chain((1..=TRAY_ICON_IDS).filter(move |id| Some(*id) != known))
    };
    let held = ids().find(|id| tray_icon_held(hwnd, *id));
    // An icon it doesn't hold is asked for its place too, under each id: should an empty
    // NIM_MODIFY fail on some Windows for an icon it has, its place still says it is there.
    let placed = !rect_withheld
        && match held {
            Some(id) => tray_icon_placed(hwnd, id),
            None => ids().any(|id| tray_icon_placed(hwnd, id)),
        };
    if held.is_none() && !placed && start.elapsed() >= TRAY_SLOW {
        return TrayAnswer::Silent;
    }
    TrayAnswer::Answered { held, placed }
}

/// Whether the shell holds icon `id` of `hwnd`: an empty `NIM_MODIFY`.
fn tray_icon_held(hwnd: HWND, id: u32) -> bool {
    use windows_sys::Win32::UI::Shell::{NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
    let nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: id,
        uFlags: 0,
        // SAFETY: a plain C struct; all zero is its empty value.
        ..unsafe { zeroed() }
    };
    // SAFETY: nid lives for the call; the shell only reads it.
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) != 0 }
}

/// Whether the shell gives icon `id` of `hwnd` a place on screen.
fn tray_icon_placed(hwnd: HWND, id: u32) -> bool {
    use windows_sys::Win32::UI::Shell::{NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect};
    let ident = NOTIFYICONIDENTIFIER {
        cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
        hWnd: hwnd,
        uID: id,
        // SAFETY: a plain C struct; with a zero GUID the icon is named by its window and id.
        ..unsafe { zeroed() }
    };
    // SAFETY: a plain C struct, written by the call.
    let mut rect: RECT = unsafe { zeroed() };
    // SAFETY: both live for the call.
    unsafe { Shell_NotifyIconGetRect(&ident, &mut rect) >= 0 }
}

/// Set the tip of icon `id` of `hwnd` (127 characters at most, the rest cut). Waits for Explorer
/// as [`tray_icon_ask`] does, so it too runs on the tray's watcher thread. False when the shell
/// didn't take it (no such icon, or no answer).
pub fn tray_icon_tip(hwnd: isize, id: u32, tip: &str) -> bool {
    use windows_sys::Win32::UI::Shell::{NIF_TIP, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW};
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd as HWND,
        uID: id,
        uFlags: NIF_TIP,
        // SAFETY: a plain C struct; all zero is its empty value.
        ..unsafe { zeroed() }
    };
    // The last unit stays the NUL that ends it.
    let room = nid.szTip.len() - 1;
    for (slot, unit) in nid.szTip.iter_mut().zip(tip.encode_utf16().take(room)) {
        *slot = unit;
    }
    // SAFETY: nid lives for the call; the shell only reads it.
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) != 0 }
}

/// Whether the window in front (the foreground window, which takes the keyboard) is another
/// program's: a click outside MDM's windows (the desktop, the taskbar, another program) put it
/// there. False while one of ours is in front, and while none is (Windows passing the foreground
/// on). An open menu closes at once on it (chrome.rs, #859).
pub fn foreground_is_another_program() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: plain queries about a window handle, which may be gone (then pid stays 0); the
    // out pointer is a local.
    unsafe {
        let front = GetForegroundWindow();
        if front.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(front, &mut pid);
        pid != 0 && pid != GetCurrentProcessId()
    }
}

/// What a borderless window of ours is, for [`popup_window`]: every one is a plain popup
/// ([`popup_style`], #208); they differ in how they take the pointer and how they look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Popup {
    /// A menu (chrome.rs): never activated, so the main window keeps the focus and the
    /// keyboard; Windows 11's small rounded corners and a 1 px border in this colour
    /// (0x00BBGGRR); no window-open animation (menus just appear).
    Menu { border: u32 },
    /// A small floating window (the drop target, droptarget.rs): activated when clicked, as
    /// any window; Windows 11's rounded corners, border and shadow, as it had with its caption
    /// style.
    Floating,
}

/// The subclass of every borderless window of ours ([`popup_proc`]): its id, and its data's
/// flag for a window never activated (a menu).
const POPUP_SUBCLASS: usize = 0x3e1;
const POPUP_NO_ACTIVATE: usize = 1;

/// A borderless window's style: a plain popup, as Win32's own menus are, without the caption,
/// frame and caption buttons winit gives every borderless window (it keeps WS_CAPTION and hides
/// the frame in WM_NCCALCSIZE). With a caption style, whatever lays the window out by its
/// styles rather than by our WM_NCCALCSIZE puts the client a caption lower and a frame to the
/// right: on a Windows 11 Insider build (22635) each menu, and the drop target, showed a black
/// band a caption tall across its top and its picture moved down by as much (#208). Without
/// one, every reading agrees: the client is the whole window.
pub(crate) fn popup_style(style: u32) -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
    };
    (style & !(WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX))
        | WS_POPUP
}

/// The extended styles a borderless window keeps: no taskbar button or Alt+Tab entry (a tool
/// window, without the app window style winit gives every window, which asks for a button),
/// and a menu is never activated.
fn popup_ex_style(ex: u32, no_activate: bool) -> u32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    (ex & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW | if no_activate { WS_EX_NOACTIVATE } else { 0 }
}

#[cfg(test)]
mod popup_style_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
    };

    /// winit's borderless window as it creates a menu or the drop target (style 0x16CB0000: a
    /// caption, system menu, minimize and maximize boxes), and the extended styles
    /// popup_window gives a menu and the drop target (always on top, a window edge).
    const WINIT_BORDERLESS: u32 = 0x16CB_0000;
    const MENU_EX: u32 = 0x0804_0198;
    const DROP_TARGET_EX: u32 = 0x0000_0198;

    /// The frame (left, top, right, bottom) the styles alone put round a `w` × `h` client: what
    /// a reader of the styles, rather than of our WM_NCCALCSIZE, takes for it.
    fn frame(style: u32, ex: u32, w: i32, h: i32) -> [i32; 4] {
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        // SAFETY: plain computation on a local rectangle.
        unsafe { AdjustWindowRectEx(&mut rc, style, 0, ex) };
        [-rc.left, -rc.top, rc.right - w, rc.bottom - h]
    }

    #[test]
    fn a_menu_window_is_a_popup_whose_client_is_all_of_it() {
        // As created, the styles alone give it a caption and a frame: what a reader of the
        // styles (rather than of our WM_NCCALCSIZE) took for its client on build 22635 (#208).
        assert_ne!(frame(WINIT_BORDERLESS, MENU_EX, 279, 275), [0, 0, 0, 0]);
        let style = popup_style(WINIT_BORDERLESS);
        assert_eq!(
            style,
            WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN
        );
        // As a popup, no reading of its styles finds a frame (at any scale: none to scale).
        assert_eq!(frame(style, MENU_EX, 279, 275), [0, 0, 0, 0]);
        // Applied again (winit setting its styles once more), nothing changes.
        assert_eq!(popup_style(style), style);
    }

    #[test]
    fn the_drop_target_is_a_popup_whose_client_is_all_of_it() {
        // The black band left on build 22635 once menus were fixed (#208): a caption about
        // 26 px tall over the 56 px box, its picture moved down by as much.
        let [_, top, _, _] = frame(WINIT_BORDERLESS, 0x0004_0118, 56, 56);
        assert!(top > 20, "a caption's height above its client: {top}");
        let style = popup_style(WINIT_BORDERLESS);
        assert_eq!(
            style,
            WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN
        );
        // As winit makes it (always on top, a window edge, files accepted, an app window).
        let ex = popup_ex_style(0x0004_0118, false);
        assert_eq!(ex, DROP_TARGET_EX);
        assert_eq!(frame(style, ex, 56, 56), [0, 0, 0, 0]);
        assert_eq!(popup_style(style), style);
    }

    #[test]
    fn a_popup_keeps_its_extended_styles() {
        // winit's own extended styles for it (always on top, a window edge, files accepted,
        // an app window), without ours: they come back, but for the app window's.
        let winit_ex = WS_EX_TOPMOST | 0x0000_0100 | 0x0000_0010 | WS_EX_APPWINDOW;
        let menu = popup_ex_style(winit_ex, true);
        assert_eq!(menu & WS_EX_APPWINDOW, 0);
        assert_eq!(menu & WS_EX_NOACTIVATE, WS_EX_NOACTIVATE);
        assert_eq!(menu & WS_EX_TOOLWINDOW, WS_EX_TOOLWINDOW);
        assert_eq!(menu & WS_EX_TOPMOST, WS_EX_TOPMOST);
        // The drop target is activated as any window (a click on it, a drag), but has no
        // taskbar button either; an off-screen test copy's never-activate stays.
        let floating = popup_ex_style(winit_ex, false);
        assert_eq!(floating & WS_EX_NOACTIVATE, 0);
        assert_eq!(floating & WS_EX_TOOLWINDOW, WS_EX_TOOLWINDOW);
        assert_eq!(floating & WS_EX_TOPMOST, WS_EX_TOPMOST);
        assert_eq!(
            popup_ex_style(winit_ex | WS_EX_NOACTIVATE, false) & WS_EX_NOACTIVATE,
            WS_EX_NOACTIVATE
        );
    }

    /// A real (hidden, off-screen) window with winit's borderless styles, made a popup by
    /// popup_window: plain popup styles, its client the whole window, and both kept when its
    /// styles are set again (winit does on every change of its flags).
    #[test]
    fn popup_window_makes_a_real_window_a_popup_and_keeps_it_one() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetClientRect, GetWindowRect, WS_CAPTION,
        };
        for (n, kind) in [
            Popup::Floating,
            Popup::Menu {
                border: 0x00ff_0000,
            },
        ]
        .into_iter()
        .enumerate()
        {
            let title = format!("mdm popup test {n} {}", std::process::id());
            let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: a hidden window of the system's STATIC class on this thread, destroyed
            // below; the strings are NUL-terminated.
            let hwnd = unsafe {
                CreateWindowExW(
                    0x0000_0108,
                    class.as_ptr(),
                    wide.as_ptr(),
                    WINIT_BORDERLESS & !WS_VISIBLE,
                    -20000,
                    -20000,
                    56,
                    56,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null());
            assert!(popup_window(&title, kind), "applied once");
            assert!(!popup_window(&title, kind), "only once");
            // SAFETY: our own window; plain queries and style changes, as winit's.
            unsafe {
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
                assert_eq!(style & WS_CAPTION, 0);
                assert_ne!(style & WS_POPUP, 0);
                // winit setting its caption style again: the subclass keeps the popup's.
                SetWindowLongPtrW(hwnd, GWL_STYLE, (style | WS_CAPTION) as isize);
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, style);
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, 0x0000_0108);
                assert_eq!(GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32, ex);
                assert_eq!(
                    ex & WS_EX_NOACTIVATE != 0,
                    matches!(kind, Popup::Menu { .. })
                );
                // Its size as the drop target's (a window with a caption style can't be made
                // that narrow: a popup can), then its client is the whole window.
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    56,
                    56,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let (mut w, mut c): (RECT, RECT) = (zeroed(), zeroed());
                GetWindowRect(hwnd, &mut w);
                GetClientRect(hwnd, &mut c);
                assert_eq!([w.right - w.left, w.bottom - w.top], [56, 56]);
                assert_eq!([c.left, c.top, c.right, c.bottom], [0, 0, 56, 56]);
                DestroyWindow(hwnd);
            }
        }
    }
}

/// Every borderless window of ours: it keeps its popup styles when winit sets the styles again
/// (it does on every change of its window flags, showing the window included), and its client
/// is the whole window, with no frame (winit's borderless frame moves the client a pixel down,
/// for a shadow drawn round a caption style, which a popup has no need of). A menu's (`data`
/// has [`POPUP_NO_ACTIVATE`]) answers mouse-activation with "don't activate": clicking an item
/// must not take focus from the main window (which would close the menu before the click
/// completes).
unsafe extern "system" fn popup_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{STYLESTRUCT, WM_STYLECHANGING};
    const WM_MOUSEACTIVATE: u32 = 0x0021;
    const MA_NOACTIVATE: LRESULT = 3;
    let no_activate = data & POPUP_NO_ACTIVATE != 0;
    if msg == WM_MOUSEACTIVATE && no_activate {
        return MA_NOACTIVATE;
    }
    if msg == WM_STYLECHANGING && lparam != 0 {
        // SAFETY: for WM_STYLECHANGING, lParam points to the STYLESTRUCT of the change, whose
        // styleNew the window may alter (Win32 contract).
        let s = unsafe { &mut *(lparam as *mut STYLESTRUCT) };
        match wparam as i32 {
            GWL_STYLE => s.styleNew = popup_style(s.styleNew),
            GWL_EXSTYLE => s.styleNew = popup_ex_style(s.styleNew, no_activate),
            _ => {}
        }
        return 0;
    }
    if msg == WM_NCCALCSIZE {
        // No non-client area: the proposed window rectangle (rgrc[0], or the RECT itself when
        // wParam is FALSE) stays as the client's.
        return 0;
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Make our borderless window titled `title` a plain popup window whose client is all of it
/// ([`popup_style`], #208), with no taskbar entry, and the rest of what `kind` says. Every
/// borderless window of ours comes here (chrome.rs's `popup_viewport` and `style_popup`). Call
/// it in the window's own pass, before anything is drawn in it: its frame changes, and a
/// picture drawn before that is lost with the old frame (black until something makes it draw
/// again). Cheap to call every frame: returns true only on the call that applied it; the window
/// is created hidden and should be shown then.
pub fn popup_window(title: &str, kind: Popup) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_WINDOW_CORNER_PREFERENCE,
        DWMWCP_ROUND, DWMWCP_ROUNDSMALL, DwmSetWindowAttribute,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetPropW, SWP_NOSIZE, SetPropW};
    let Some(hwnd) = find_own_window(title) else {
        return false;
    };
    let no_activate = matches!(kind, Popup::Menu { .. });
    // SAFETY: our own window, on its thread; the subclass proc lives as long as the program;
    // the DWM attributes are read from locals during the calls.
    unsafe {
        // Done once (comctl32 5.8, which MDM links, has no GetWindowSubclass to ask by).
        let done: Vec<u16> = "MDM.popup\0".encode_utf16().collect();
        if !GetPropW(hwnd, done.as_ptr()).is_null() {
            return false;
        }
        SetPropW(hwnd, done.as_ptr(), 1usize as _);
        if let Popup::Menu { .. } = kind {
            let off: i32 = 1;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                &off as *const i32 as *const c_void,
                size_of::<i32>() as u32,
            );
        }
        // The subclass first: it keeps the styles below when winit sets its own again.
        let data = if no_activate { POPUP_NO_ACTIVATE } else { 0 };
        SetWindowSubclass(hwnd, Some(popup_proc), POPUP_SUBCLASS, data);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, popup_ex_style(ex, no_activate) as isize);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_STYLE, popup_style(style) as isize);
        // The frame measured again (WM_NCCALCSIZE): no caption, the client the whole window.
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        // Windows 11 rounds a window with a caption style by itself, not a popup: asked for.
        let corner = match kind {
            Popup::Menu { .. } => DWMWCP_ROUNDSMALL,
            Popup::Floating => DWMWCP_ROUND,
        };
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &corner as *const _ as *const c_void,
            size_of::<i32>() as u32,
        );
        if let Popup::Menu { border } = kind {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR as u32,
                &border as *const u32 as *const c_void,
                size_of::<u32>() as u32,
            );
        }
    }
    true
}

/// Width of `text` in Segoe UI 12 px as GDI measures it (whole-pixel hinted advances) — the
/// width Win32 menus size themselves by. Cached per string.
pub fn menu_text_width(text: &str) -> i32 {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use windows_sys::Win32::Foundation::SIZE;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateFontW, GetTextExtentPoint32W, HFONT, SelectObject,
    };
    thread_local! {
        static FONT: HFONT = {
            let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
            // SAFETY: plain GDI object creation; the font lives for the thread.
            unsafe { CreateFontW(-12, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 0, 0, face.as_ptr()) }
        };
        static CACHE: RefCell<HashMap<String, i32>> = RefCell::new(HashMap::new());
    }
    if let Some(w) = CACHE.with(|c| c.borrow().get(text).copied()) {
        return w;
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    let w = FONT.with(|font| unsafe {
        let dc = GetDC(std::ptr::null_mut());
        let old = SelectObject(dc, *font);
        let mut size = SIZE { cx: 0, cy: 0 };
        GetTextExtentPoint32W(dc, wide.as_ptr(), wide.len() as i32, &mut size);
        SelectObject(dc, old);
        ReleaseDC(std::ptr::null_mut(), dc);
        size.cx
    });
    CACHE.with(|c| c.borrow_mut().insert(text.to_owned(), w));
    w
}

thread_local! {
    /// The caption each of our windows was last given by [`caption_dark`] (dark: true).
    static CAPTION_DARK: std::cell::RefCell<std::collections::HashMap<isize, bool>> =
        Default::default();
}

/// Whether DWM is to be told `dark` for window `hwnd`: not when that is what it was last told.
/// Windows that are gone are forgotten.
fn caption_news(hwnd: isize, dark: bool, alive: impl Fn(isize) -> bool) -> bool {
    CAPTION_DARK.with(|m| {
        let Ok(mut m) = m.try_borrow_mut() else {
            return true;
        };
        if m.get(&hwnd) == Some(&dark) {
            return false;
        }
        m.retain(|h, _| alive(*h));
        m.insert(hwnd, dark);
        true
    })
}

/// How many times [`caption_dark`] has asked DWM, for the tests.
#[cfg(test)]
static CAPTION_DWM_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Give our window titled `title` a dark or light caption (DWM). Returns false if not found.
/// DWM is asked only when the window's caption changes, never again for the same one: a call
/// into DWM waits on a system that is stalled (its disk, its memory), and the window's thread
/// waited with it, every 2 s (#633). Callers call it when the theme changes and for a new window.
pub fn caption_dark(title: &str, dark: bool) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    use windows_sys::Win32::Graphics::Gdi::{RDW_FRAME, RDW_INVALIDATE, RedrawWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;
    let Some(hwnd) = find_own_window(title) else {
        return false;
    };
    // SAFETY: a plain query; any value is accepted as the handle.
    let alive = |h: isize| unsafe { IsWindow(h as HWND) } != 0;
    if !caption_news(hwnd as isize, dark, alive) {
        return true;
    }
    #[cfg(test)]
    CAPTION_DWM_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let v: i32 = dark.into();
    // SAFETY: documented attribute with a 4-byte BOOL value.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &v as *const i32 as *const c_void,
            size_of::<i32>() as u32,
        );
        // Windows repaints a caption only on the next activation change; force it now.
        RedrawWindow(
            hwnd,
            std::ptr::null(),
            std::ptr::null_mut(),
            RDW_FRAME | RDW_INVALIDATE,
        );
    }
    true
}

/// The popup menus Windows draws for us — the notification-area icon's (tray-icon's), the
/// window menu of a drawn title bar — in MDM's mode, not Windows' (#682): dark on a dark surface
/// (dark mode, a dark theme), light otherwise, whatever Windows' own app mode says. Windows has
/// no documented way: uxtheme's SetPreferredAppMode (ordinal 135, ForceDark 2 / ForceLight 3)
/// sets the process's mode and FlushMenuThemes (ordinal 136) makes menus read it again, as the
/// win32-darkmode project and the apps built on it do. They exist from Windows 10 1903 (build
/// 18362) on; 1809 has another function at 135 and older builds no dark menus, so there menus
/// stay as Windows draws them. Called when the mode changes; asks uxtheme only when it is news.
pub fn menus_dark(dark: bool) {
    use std::sync::atomic::{AtomicU8, Ordering};
    static TOLD: AtomicU8 = AtomicU8::new(u8::MAX);
    let mode: i32 = if dark { 2 } else { 3 };
    if TOLD.swap(mode as u8, Ordering::Relaxed) == mode as u8 {
        return;
    }
    let Some(f) = menu_mode_fns() else {
        return;
    };
    // SAFETY: the functions behind these ordinals on build 18362 and later (menu_mode_fns),
    // SetPreferredAppMode(int) -> int and FlushMenuThemes(void); process-wide, any thread.
    unsafe {
        (f.set_preferred_app_mode)(mode);
        (f.flush_menu_themes)();
    }
}

/// uxtheme's functions that set the mode of the menus Windows draws ([`menus_dark`]).
#[derive(Clone, Copy)]
struct MenuModeFns {
    set_preferred_app_mode: unsafe extern "system" fn(i32) -> i32,
    flush_menu_themes: unsafe extern "system" fn(),
}

/// [`MenuModeFns`], looked up once: None before Windows 10 1903 or if uxtheme lacks them.
fn menu_mode_fns() -> Option<MenuModeFns> {
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
    };
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    static FNS: std::sync::OnceLock<Option<MenuModeFns>> = std::sync::OnceLock::new();
    *FNS.get_or_init(|| {
        let build = reg_string_in(
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "CurrentBuild",
        )
        .and_then(|b| b.parse::<u32>().ok())?;
        if build < 18362 {
            return None;
        }
        let name: Vec<u16> = "uxtheme.dll\0".encode_utf16().collect();
        // SAFETY: a NUL-terminated name, loaded from System32 only; the module stays loaded for
        // the process's life (never freed), so the pointers taken from it stay valid. An ordinal
        // goes as the name pointer's low word (MAKEINTRESOURCEA).
        unsafe {
            let module = LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            );
            if module.is_null() {
                return None;
            }
            let set = GetProcAddress(module, 135usize as *const u8)?;
            let flush = GetProcAddress(module, 136usize as *const u8)?;
            Some(MenuModeFns {
                set_preferred_app_mode: std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "system" fn(i32) -> i32,
                >(set),
                flush_menu_themes: std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "system" fn(),
                >(flush),
            })
        }
    })
}

#[cfg(test)]
mod menus_dark_tests {
    /// #682: uxtheme has the menu mode functions on every Windows MDM is tested on (10 1903 and
    /// later, Server 2022's runner included), and setting the mode either way returns.
    #[test]
    fn the_menu_mode_functions_are_found_and_answer() {
        assert!(super::menu_mode_fns().is_some());
        super::menus_dark(true);
        super::menus_dark(false);
    }
}

/// A drag out of Download complete is under way ([`drag_file_out`]): one at a time, the mouse
/// holds only one.
static DRAGGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// When the last drag out ended.
static DRAG_ENDED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// A drag out is under way, or ended a moment ago. Its keys are its own: Escape cancels the
/// drag in OLE's loop, on the drag's thread, while the window (whose thread the keys go to)
/// sees them too, often after the drag already ended, and must not take Escape as Close.
pub fn dragging_out() -> bool {
    use std::sync::atomic::Ordering;
    DRAGGING.load(Ordering::Acquire)
        || DRAG_ENDED
            .lock()
            .ok()
            .and_then(|t| *t)
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(500))
}

/// Start dragging `file` out of our window (to Explorer, a mail, a chat…), like IDM's Download
/// complete grip. The drag is the shell's own for the file, as Explorer's is (its preview, its
/// formats). The window is the one holding the mouse (the button is down on its grip: winit
/// captured the mouse for it), so it is the right one when several show the same title; else
/// our window titled `title`, given in the interface language, as it shows.
///
/// The drop target decides what the drag does (#869): MDM offers a move, a copy and a link, so
/// Explorer applies its own rules, as for a file dragged in Explorer: moved on the same drive or
/// with Shift, copied to another drive or with Ctrl, a shortcut with Alt. A move is followed:
/// `followed` is told where the file went (from the shell's change notifications), or that it
/// went where MDM can't learn; a copy, a link or a cancelled drag tell nothing. It may come long
/// after the drop (a move to another drive copies first), from the drag's thread. MDM never
/// deletes the file itself, even for a target that asks the source to (a move it doesn't
/// optimize): the file stays, so nothing is lost.
///
/// The drag runs on a thread of its own (#561): OLE's drag loop lasts as long as the button is
/// held, and each step calls the program under the pointer (its drop target, in its own process),
/// so on the window thread a busy or hung target, or a target that copies the file before it
/// answers the drop, froze every window until it answered. That thread shares the window
/// thread's input ([`with_shared_input`]): OLE reads the held button and takes the mouse from
/// there, as it did from the window thread.
pub fn drag_file_out(
    title: &str,
    file: &std::path::Path,
    followed: impl FnOnce(crate::dialogs::DraggedOut) + Send + 'static,
) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let hwnd = own_capture()
        .or_else(|| find_own_window(title))
        .ok_or("window not found")? as isize;
    if DRAGGING.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let file = file.to_path_buf();
    let started = with_shared_input("drag out", move |shared| {
        if !shared {
            tracing::warn!("drag out: the window's input isn't shared");
        }
        // Looked for here, beside the window (#743): a drive slow to answer doesn't hold it.
        let result = if file.exists() {
            dragout::drag(hwnd as HWND, &file)
        } else {
            Ok(None)
        };
        if let Ok(mut t) = DRAG_ENDED.lock() {
            *t = Some(std::time::Instant::now());
        }
        DRAGGING.store(false, Ordering::Release);
        // OLE's drag loop took the button's release, so the window never saw it: without it
        // the next press on the grip only ended the drag the window still thought was on, and
        // the grip worked every other time.
        release_button(hwnd as HWND);
        let dropped = result.unwrap_or_else(|e| {
            tracing::warn!("drag out: {e}");
            None
        });
        // Once the window's input is given back: what became of the file.
        move || {
            if let Some(out) = dropped.and_then(dragout::Dropped::follow) {
                followed(out);
            }
        }
    });
    if started.is_err() {
        DRAGGING.store(false, Ordering::Release);
    }
    started
}

/// Tell `hwnd` the left button is up, at the pointer, as Windows would have done had OLE's
/// drag loop not taken the release. A move first: the pointer left the window during the drag,
/// and a release with no pointer position is dropped before it reaches the interface.
fn release_button(hwnd: HWND) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    // The left button's flag in a mouse message's wParam (MK_LBUTTON, winuser.h).
    const MK_LBUTTON: usize = 0x0001;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, PostMessageW, WM_LBUTTONUP, WM_MOUSEMOVE,
    };
    let mut pt = POINT { x: 0, y: 0 };
    // SAFETY: reads into a local; `hwnd` is our own window (a closed one fails harmlessly).
    unsafe {
        GetCursorPos(&mut pt);
        ScreenToClient(hwnd, &mut pt);
    }
    let lparam = (((pt.y as u16 as u32) << 16) | pt.x as u16 as u32) as isize;
    // SAFETY: messages posted to our own window.
    unsafe {
        PostMessageW(hwnd, WM_MOUSEMOVE, MK_LBUTTON, lparam);
        PostMessageW(hwnd, WM_LBUTTONUP, 0, lparam);
    }
}

/// Run `work` on a new thread named `name`, with OLE (a single-threaded apartment) and the
/// calling thread's input state: the keys and mouse buttons held, the mouse capture and the
/// focus (AttachThreadInput), as a modal loop like OLE's drag needs when the button went down in
/// the caller's window. `work` is told whether the input is shared. Returns at once; the input
/// is detached when `work` ends, then what `work` returned runs (still with OLE: the drag's
/// follow-up), and OLE is closed. The caller has a message queue (a window thread).
fn with_shared_input<A: FnOnce()>(
    name: &str,
    work: impl FnOnce(bool) -> A + Send + 'static,
) -> Result<(), String> {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, PM_NOREMOVE, PeekMessageW};
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn OleInitialize(reserved: *mut c_void) -> i32;
        fn OleUninitialize();
    }
    // SAFETY: a plain query.
    let caller = unsafe { GetCurrentThreadId() };
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            // SAFETY: OLE for this new thread, closed below on the same thread when it took.
            let ole = unsafe { OleInitialize(std::ptr::null_mut()) } >= 0;
            // A thread is given its message queue by its first window call, and only threads
            // with one can share input: a look at the (empty) queue makes it.
            // SAFETY: a read into a local MSG; nothing is removed.
            let mut msg: MSG = unsafe { zeroed() };
            unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
            // SAFETY: plain calls with two live threads of this process; undone below.
            let me = unsafe { GetCurrentThreadId() };
            let attached = unsafe { AttachThreadInput(me, caller, 1) } != 0;
            let after = work(attached);
            if attached {
                // SAFETY: as above.
                unsafe { AttachThreadInput(me, caller, 0) };
            }
            after();
            if ole {
                // SAFETY: matches the OleInitialize above, on this thread.
                unsafe { OleUninitialize() };
            }
        })
        .map(drop)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod drag_thread_tests {
    use super::*;

    /// The drag's thread (#561): another thread than the window's, which goes on at once, in a
    /// single-threaded apartment (OLE's drag needs one), sharing the window thread's input.
    #[test]
    fn the_drag_runs_beside_the_window_thread_with_its_input() {
        use windows_sys::Win32::System::Com::{APTTYPE, APTTYPEQUALIFIER, CoGetApartmentType};
        use windows_sys::Win32::System::Threading::GetCurrentThreadId;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, PM_NOREMOVE, PeekMessageW};
        // This test's thread stands for the window thread: it has a message queue.
        // SAFETY: a read into a local MSG; nothing is removed.
        let mut msg: MSG = unsafe { zeroed() };
        unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_NOREMOVE) };
        // SAFETY: a plain query.
        let window_thread = unsafe { GetCurrentThreadId() };
        let (tx, rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        with_shared_input("drag test", move |shared| {
            // Held until the caller has gone on: the work doesn't hold the caller up.
            go_rx.recv().unwrap();
            let mut kind: APTTYPE = 0;
            let mut qualifier: APTTYPEQUALIFIER = 0;
            // SAFETY: reads into two locals.
            let hr = unsafe { CoGetApartmentType(&mut kind, &mut qualifier) };
            // SAFETY: a plain query.
            let me = unsafe { GetCurrentThreadId() };
            tx.send((me, shared, hr, kind)).unwrap();
            || {}
        })
        .unwrap();
        go_tx.send(()).unwrap();
        let (me, shared, hr, kind) = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert_ne!(me, window_thread);
        assert!(shared, "the input is shared with the window thread");
        assert_eq!(hr, 0);
        // APTTYPE_STA (0); the first apartment of the process would be APTTYPE_MAINSTA (3).
        assert!(
            kind == 0 || kind == 3,
            "a single-threaded apartment, not {kind}"
        );
    }
}

/// Dragging a downloaded file out of Download complete (#869): the shell's own data object for
/// it (Explorer's drag image, its formats), offered for a move, a copy or a link so the drop
/// target applies Explorer's rules, and a move followed through the shell's change
/// notifications, so the download can point where its file went.
mod dragout {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::mem::zeroed;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HWND_MESSAGE, MSG,
        MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, RegisterClassW,
        TranslateMessage, WM_APP, WNDCLASSW,
    };

    use crate::dialogs::DraggedOut;

    type HResult = i32;
    const DROPEFFECT_COPY: u32 = 1;
    const DROPEFFECT_MOVE: u32 = 2;
    const DROPEFFECT_LINK: u32 = 4;
    /// SHDoDragDrop's (DoDragDrop's) answer when the file was dropped, not cancelled.
    const DRAGDROP_S_DROP: HResult = 0x0004_0100;
    /// SHChangeNotifyRegister: the shell's own notifications (what Explorer and every program
    /// using the shell's file operations do), not the disks' (which would watch every drive),
    /// delivered as a handle to lock.
    const SHCNRF_SHELL_LEVEL: i32 = 0x0002;
    const SHCNRF_NEW_DELIVERY: i32 = 0x8000;
    const SHCNE_RENAMEITEM: i32 = 0x0000_0001;
    const SHCNE_CREATE: i32 = 0x0000_0002;
    const SHCNE_DELETE: i32 = 0x0000_0004;
    const SHCNE_MKDIR: i32 = 0x0000_0008;
    const SHCNE_RMDIR: i32 = 0x0000_0010;
    const SHCNE_RENAMEFOLDER: i32 = 0x0002_0000;
    /// The notification's message to our watching window.
    const WM_SHELL_CHANGED: u32 = WM_APP + 0x69;
    /// The formats a drop target sets on the data object to say what it did (Shell Clipboard
    /// Formats): an optimized move (Explorer's, on any drive) answers "none" to the drag, so
    /// the source doesn't delete the file the target already moved, and says "move" here.
    const PERFORMED: &str = "Performed DropEffect";
    const LOGICAL_PERFORMED: &str = "Logical Performed DropEffect";
    /// How long a dropped file is watched while it is still there: a target that said it moves
    /// it may be copying it to another drive first; one that said nothing may do so on its own
    /// thread.
    const MOVING_FOR: Duration = Duration::from_secs(30 * 60);
    const DROPPED_FOR: Duration = Duration::from_secs(60);
    /// How long the shell's word on where it went is waited for once the file is gone.
    const GONE_FOR: Duration = Duration::from_secs(3);

    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
    const IID_IDATAOBJECT: Guid = Guid(0x0000_010e, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

    #[repr(C)]
    struct NotifyEntry {
        pidl: *const c_void,
        recursive: i32,
    }

    #[repr(C)]
    struct FormatEtc {
        cf: u16,
        ptd: *mut c_void,
        aspect: u32,
        lindex: i32,
        tymed: u32,
    }

    #[repr(C)]
    struct StgMedium {
        tymed: u32,
        handle: *mut c_void,
        unk_for_release: *mut c_void,
    }

    /// IDataObject's vtable up to GetData (index 3), all we call.
    #[repr(C)]
    struct DataObjectVtbl {
        _query_interface: usize,
        _add_ref: usize,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        get_data:
            unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut StgMedium) -> HResult,
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ILCreateFromPathW(path: *const u16) -> *mut c_void;
        fn ILFree(pidl: *mut c_void);
        fn ILClone(pidl: *const c_void) -> *mut c_void;
        fn ILRemoveLastID(pidl: *mut c_void) -> i32;
        fn ILFindLastID(pidl: *const c_void) -> *mut c_void;
        fn SHCreateDataObject(
            folder: *const c_void,
            count: u32,
            items: *const *const c_void,
            inner: *mut c_void,
            riid: *const Guid,
            out: *mut *mut c_void,
        ) -> HResult;
        fn SHDoDragDrop(
            hwnd: HWND,
            data: *mut c_void,
            source: *mut c_void,
            effects: u32,
            effect: *mut u32,
        ) -> HResult;
        fn SHGetSpecialFolderLocation(hwnd: HWND, csidl: i32, pidl: *mut *mut c_void) -> HResult;
        fn SHChangeNotifyRegister(
            hwnd: HWND,
            sources: i32,
            events: i32,
            msg: u32,
            count: i32,
            entries: *const NotifyEntry,
        ) -> u32;
        fn SHChangeNotifyDeregister(id: u32) -> i32;
        fn SHChangeNotification_Lock(
            change: *mut c_void,
            pid: u32,
            pidls: *mut *mut *mut c_void,
            event: *mut i32,
        ) -> *mut c_void;
        fn SHChangeNotification_Unlock(lock: *mut c_void) -> i32;
        fn SHGetPathFromIDListEx(pidl: *const c_void, path: *mut u16, len: u32, opts: i32) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn ReleaseStgMedium(medium: *mut StgMedium);
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalLock(h: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(h: *mut c_void) -> i32;
        fn GlobalSize(h: *mut c_void) -> usize;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
    }

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain(Some(0)).collect()
    }

    /// What the shell said happened to a file or folder, as paths.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) enum Seen {
        Renamed(PathBuf, PathBuf),
        Created(PathBuf),
        Deleted(PathBuf),
    }

    thread_local! {
        /// What this drag's watch saw (one drag per thread).
        static SEEN: RefCell<Vec<Seen>> = const { RefCell::new(Vec::new()) };
    }

    /// Two paths name the same file: Windows' names don't tell case apart.
    fn same(a: &Path, b: &Path) -> bool {
        let norm = |p: &Path| {
            p.to_string_lossy()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_lowercase()
        };
        norm(a) == norm(b)
    }

    /// `path` without the `\\?\` that canonicalize puts before it (the shell's paths have none).
    pub(super) fn plain(path: PathBuf) -> PathBuf {
        let s = path.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            PathBuf::from(format!(r"\\{rest}"))
        } else if let Some(rest) = s.strip_prefix(r"\\?\") {
            PathBuf::from(rest)
        } else {
            path
        }
    }

    /// In a drive's Recycle Bin (`$Recycle.Bin`): thrown away, not put somewhere to open.
    pub(super) fn in_recycle_bin(path: &Path) -> bool {
        path.components().any(|c| {
            c.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case("$Recycle.Bin")
        })
    }

    /// Where `original` went, from what the shell saw: renamed or moved on its drive (one
    /// notification with both paths, followed as often as it moved), or, gone from its place,
    /// the one file of its name made elsewhere meanwhile (a move to another drive copies then
    /// deletes). None while nothing says, or when two places could be it.
    pub(super) fn followed(original: &Path, seen: &[Seen]) -> Option<PathBuf> {
        let mut at = original.to_path_buf();
        let (mut renamed, mut deleted) = (false, false);
        for s in seen {
            match s {
                Seen::Renamed(from, to) if same(from, &at) => {
                    at = to.clone();
                    renamed = true;
                }
                Seen::Deleted(p) if same(p, original) => deleted = true,
                _ => {}
            }
        }
        if renamed {
            return Some(at);
        }
        if !deleted {
            return None;
        }
        let name = original.file_name()?.to_string_lossy().to_lowercase();
        let mut made = seen.iter().filter_map(|s| match s {
            Seen::Created(p)
                if !same(p, original)
                    && p.file_name()
                        .is_some_and(|n| n.to_string_lossy().to_lowercase() == name) =>
            {
                Some(p)
            }
            _ => None,
        });
        let first = made.next()?;
        if made.any(|p| !same(p, first)) {
            return None;
        }
        Some(first.clone())
    }

    /// The shell's path of an item, if it is one on a disk.
    fn path_of(pidl: *const c_void) -> Option<PathBuf> {
        if pidl.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 32_768];
        // SAFETY: `pidl` is the shell's, valid while its notification is locked; `buf` is as
        // long as said.
        let ok = unsafe { SHGetPathFromIDListEx(pidl, buf.as_mut_ptr(), buf.len() as u32, 0) };
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        (ok != 0 && len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    }

    /// The watching window's procedure: each notification, as paths, into [`SEEN`].
    unsafe extern "system" fn watch_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg != WM_SHELL_CHANGED {
            // SAFETY: the system's default for every other message.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
        let mut pidls: *mut *mut c_void = std::ptr::null_mut();
        let mut event = 0i32;
        // SAFETY: a new-delivery notification: wParam its handle, lParam the process to lock
        // it for; the two items read before it is unlocked.
        unsafe {
            let lock = SHChangeNotification_Lock(
                wparam as *mut c_void,
                lparam as u32,
                &mut pidls,
                &mut event,
            );
            if lock.is_null() || pidls.is_null() {
                return 0;
            }
            let item = |i: usize| path_of(*pidls.add(i));
            // The low bits: the event (the high one says it came from the disk).
            let seen = match event & 0x7FFF_FFFF {
                SHCNE_RENAMEITEM | SHCNE_RENAMEFOLDER => {
                    item(0).zip(item(1)).map(|(a, b)| Seen::Renamed(a, b))
                }
                SHCNE_CREATE | SHCNE_MKDIR => item(0).map(Seen::Created),
                SHCNE_DELETE | SHCNE_RMDIR => item(0).map(Seen::Deleted),
                _ => None,
            };
            SHChangeNotification_Unlock(lock);
            if let Some(s) = seen {
                tracing::debug!("drag out: the shell saw {s:?}");
                SEEN.with(|v| v.borrow_mut().push(s));
            }
        }
        0
    }

    /// A hidden window of the drag's thread, told by the shell of every file renamed, made or
    /// deleted through it (the whole namespace: a move goes anywhere).
    struct Watch {
        hwnd: HWND,
        id: u32,
    }

    impl Watch {
        fn start() -> Option<Watch> {
            static CLASS: std::sync::Once = std::sync::Once::new();
            let class: Vec<u16> = "mdm-drag-out-watch\0".encode_utf16().collect();
            // SAFETY: a class with our procedure, registered once; a message-only window of
            // this thread, destroyed in Drop (or below when the shell won't watch); the desktop's
            // item list freed once registered (the shell keeps its own copy).
            unsafe {
                let module = GetModuleHandleW(std::ptr::null());
                CLASS.call_once(|| {
                    let mut wc: WNDCLASSW = zeroed();
                    wc.lpfnWndProc = Some(watch_proc);
                    wc.hInstance = module;
                    wc.lpszClassName = class.as_ptr();
                    RegisterClassW(&wc);
                });
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    module,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return None;
                }
                let mut desktop = std::ptr::null_mut();
                // CSIDL_DESKTOP (0): the namespace's root.
                if SHGetSpecialFolderLocation(std::ptr::null_mut(), 0, &mut desktop) < 0 {
                    DestroyWindow(hwnd);
                    return None;
                }
                let entry = NotifyEntry {
                    pidl: desktop,
                    recursive: 1,
                };
                let id = SHChangeNotifyRegister(
                    hwnd,
                    SHCNRF_SHELL_LEVEL | SHCNRF_NEW_DELIVERY,
                    SHCNE_RENAMEITEM
                        | SHCNE_RENAMEFOLDER
                        | SHCNE_CREATE
                        | SHCNE_MKDIR
                        | SHCNE_DELETE
                        | SHCNE_RMDIR,
                    WM_SHELL_CHANGED,
                    1,
                    &entry,
                );
                ILFree(desktop);
                if id == 0 {
                    DestroyWindow(hwnd);
                    return None;
                }
                SEEN.with(|v| v.borrow_mut().clear());
                Some(Watch { hwnd, id })
            }
        }

        /// Wait up to `wait` for messages, and hand every one of this thread's to its window.
        fn pump(&self, wait: Duration) {
            // SAFETY: waits on no handle, then the thread's own message loop.
            unsafe {
                MsgWaitForMultipleObjects(
                    0,
                    std::ptr::null(),
                    0,
                    wait.as_millis() as u32,
                    QS_ALLINPUT,
                );
                let mut msg: MSG = zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            // SAFETY: what `start` registered and made, on the same thread.
            unsafe {
                SHChangeNotifyDeregister(self.id);
                DestroyWindow(self.hwnd);
            }
            SEEN.with(|v| v.borrow_mut().clear());
        }
    }

    /// The shell's data object for one file: what Explorer itself drags (the file, its item, its
    /// drag image), and where a drop target says what it did.
    struct DataObject(*mut c_void);

    impl DataObject {
        fn of(path: &Path) -> Option<DataObject> {
            let w = wide(path.as_os_str());
            // SAFETY: the item lists made from a NUL-terminated path (its folder's, and the
            // file's within it, as Explorer's own drags give them) are freed once the data object
            // has its own copies; `out` receives an IDataObject we release in Drop.
            unsafe {
                let pidl = ILCreateFromPathW(w.as_ptr());
                if pidl.is_null() {
                    return None;
                }
                let folder = ILClone(pidl);
                if folder.is_null() {
                    ILFree(pidl);
                    return None;
                }
                ILRemoveLastID(folder);
                let item = ILFindLastID(pidl) as *const c_void;
                let mut out = std::ptr::null_mut();
                let hr = SHCreateDataObject(
                    folder,
                    1,
                    &item,
                    std::ptr::null_mut(),
                    &IID_IDATAOBJECT,
                    &mut out,
                );
                ILFree(folder);
                ILFree(pidl);
                (hr >= 0 && !out.is_null()).then_some(DataObject(out))
            }
        }

        /// A drop effect the drop target set on it, under the format `name`.
        fn effect(&self, name: &str) -> Option<u32> {
            let w = wide(std::ffi::OsStr::new(name));
            // SAFETY: GetData through the object's vtable with a FORMATETC for an HGLOBAL; the
            // medium is read under its lock and released.
            unsafe {
                let cf = RegisterClipboardFormatW(w.as_ptr());
                if cf == 0 {
                    return None;
                }
                let fmt = FormatEtc {
                    cf: cf as u16,
                    ptd: std::ptr::null_mut(),
                    aspect: 1,
                    lindex: -1,
                    tymed: 1,
                };
                let mut medium: StgMedium = zeroed();
                let vtbl = *(self.0 as *const *const DataObjectVtbl);
                if ((*vtbl).get_data)(self.0, &fmt, &mut medium) < 0 {
                    return None;
                }
                let mut value = None;
                if medium.tymed == 1 && GlobalSize(medium.handle) >= 4 {
                    let p = GlobalLock(medium.handle) as *const u32;
                    if !p.is_null() {
                        value = Some(p.read_unaligned());
                        GlobalUnlock(medium.handle);
                    }
                }
                ReleaseStgMedium(&mut medium);
                value
            }
        }

        fn release(&mut self) {
            if !self.0.is_null() {
                // SAFETY: our one reference, released once.
                unsafe {
                    let vtbl = *(self.0 as *const *const DataObjectVtbl);
                    ((*vtbl).release)(self.0);
                }
                self.0 = std::ptr::null_mut();
            }
        }
    }

    impl Drop for DataObject {
        fn drop(&mut self) {
            self.release();
        }
    }

    /// A file dropped somewhere: what it took to learn what became of it.
    pub(super) struct Dropped {
        file: PathBuf,
        data: DataObject,
        watch: Option<Watch>,
        /// What the drag returned.
        effect: u32,
    }

    /// Drag `file` out of `hwnd` (on the drag's thread, OLE set up), offering a move, a copy and
    /// a link. Some when it was dropped.
    pub(super) fn drag(hwnd: HWND, file: &Path) -> Result<Option<Dropped>, String> {
        let file = plain(std::fs::canonicalize(file).map_err(|e| e.to_string())?);
        // Watched from before the drop: a move on the same drive is done before the drag ends.
        // MDM_TEST_NO_SHELL_WATCH=1 (debug builds, for checks): not watched, as when the shell
        // won't say, so a move is one MDM can't follow.
        let unwatched = cfg!(debug_assertions)
            && std::env::var_os("MDM_TEST_NO_SHELL_WATCH").is_some_and(|v| v == "1");
        let watch = if unwatched { None } else { Watch::start() };
        if watch.is_none() {
            tracing::warn!("drag out: the shell won't say where files go: a move isn't followed");
        }
        let data = DataObject::of(&file).ok_or("the shell has no data object for the file")?;
        let mut effect = 0u32;
        // SAFETY: our window and data object; the shell makes the drop source and drag image,
        // and runs OLE's drag loop on this thread until the drop.
        let hr = unsafe {
            SHDoDragDrop(
                hwnd,
                data.0,
                std::ptr::null_mut(),
                DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
                &mut effect,
            )
        };
        if hr < 0 {
            return Err(format!("SHDoDragDrop failed, {hr:#010x}"));
        }
        if hr != DRAGDROP_S_DROP {
            return Ok(None);
        }
        tracing::info!(
            "drag out: dropped, effect {effect}, performed {:?}, logical {:?}",
            data.effect(PERFORMED),
            data.effect(LOGICAL_PERFORMED)
        );
        Ok(Some(Dropped {
            file,
            data,
            watch,
            effect,
        }))
    }

    impl Dropped {
        /// What became of the file: moved (where to, or that MDM can't learn where), or None
        /// when it stayed (a copy, a link, a drop that did nothing). Waits as long as a move may
        /// take, on the drag's thread.
        pub(super) fn follow(mut self) -> Option<DraggedOut> {
            let since = Instant::now();
            let mut gone: Option<Instant> = None;
            loop {
                match &self.watch {
                    Some(w) => w.pump(Duration::from_millis(250)),
                    None => std::thread::sleep(Duration::from_millis(250)),
                }
                let said = [
                    Some(self.effect),
                    self.data.effect(PERFORMED),
                    self.data.effect(LOGICAL_PERFORMED),
                ];
                let moving = said.iter().flatten().any(|e| e & DROPEFFECT_MOVE != 0);
                // A copy or a link, said so: the file stays.
                let kept = !moving
                    && said
                        .iter()
                        .flatten()
                        .any(|e| e & (DROPEFFECT_COPY | DROPEFFECT_LINK) != 0);
                if kept {
                    return None;
                }
                let seen = SEEN.with(|v| v.borrow().clone());
                if let Some(to) = followed(&self.file, &seen) {
                    tracing::info!("drag out: the file was moved");
                    self.data.release();
                    return Some(if in_recycle_bin(&to) {
                        DraggedOut::MovedAway
                    } else {
                        DraggedOut::Moved(to)
                    });
                }
                if std::fs::symlink_metadata(&self.file).is_ok() {
                    gone = None;
                    if since.elapsed() > if moving { MOVING_FOR } else { DROPPED_FOR } {
                        return None;
                    }
                } else if gone.get_or_insert_with(Instant::now).elapsed() > GONE_FOR {
                    tracing::info!("drag out: the file was moved where the shell didn't say");
                    return Some(DraggedOut::MovedAway);
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn p(s: &str) -> PathBuf {
            PathBuf::from(s)
        }

        /// A move on the same drive is one notification with both paths, followed however
        /// often the file moved; a move to another drive is a copy made there and the file
        /// deleted here; the Recycle Bin is no place to open.
        #[test]
        fn a_moved_file_is_followed_by_what_the_shell_saw() {
            let file = p(r"C:\Downloads\video.mp4");
            let moved = Seen::Renamed(file.clone(), p(r"C:\Movies\video.mp4"));
            assert_eq!(
                followed(&file, std::slice::from_ref(&moved)),
                Some(p(r"C:\Movies\video.mp4"))
            );
            // Case and separators don't matter (Windows' names).
            let other_case = Seen::Renamed(p(r"c:\downloads\VIDEO.mp4"), p(r"C:\Movies\video.mp4"));
            assert!(followed(&file, &[other_case]).is_some());
            // Moved again: its last place; a name taken there: the numbered one.
            let again = Seen::Renamed(p(r"C:\Movies\video.mp4"), p(r"C:\Old\video (2).mp4"));
            assert_eq!(
                followed(&file, &[moved.clone(), again]),
                Some(p(r"C:\Old\video (2).mp4"))
            );
            // Another drive: made there, then deleted here.
            let made = Seen::Created(p(r"E:\Backup\video.mp4"));
            let deleted = Seen::Deleted(file.clone());
            assert_eq!(
                followed(&file, std::slice::from_ref(&made)),
                None,
                "still here"
            );
            assert_eq!(
                followed(&file, &[made.clone(), deleted.clone()]),
                Some(p(r"E:\Backup\video.mp4"))
            );
            // Two files of its name made meanwhile: can't tell which.
            let made2 = Seen::Created(p(r"F:\video.mp4"));
            assert_eq!(followed(&file, &[made, made2, deleted.clone()]), None);
            // Deleted, nothing of its name made: not known.
            assert_eq!(followed(&file, &[deleted]), None);
            // Other files' changes say nothing.
            let others = [
                Seen::Renamed(p(r"C:\Downloads\a.zip"), p(r"C:\b.zip")),
                Seen::Deleted(p(r"C:\Downloads\a.zip")),
            ];
            assert_eq!(followed(&file, &others), None);
            assert!(in_recycle_bin(&p(
                r"C:\$Recycle.Bin\S-1-5-21-1\$RAB12CD.mp4"
            )));
            assert!(!in_recycle_bin(&p(r"C:\Movies\video.mp4")));
        }

        #[test]
        fn canonical_paths_as_the_shell_writes_them() {
            assert_eq!(
                plain(p(r"\\?\C:\Downloads\a.zip")),
                p(r"C:\Downloads\a.zip")
            );
            assert_eq!(
                plain(p(r"\\?\UNC\server\share\a.zip")),
                p(r"\\server\share\a.zip")
            );
            assert_eq!(plain(p(r"C:\a.zip")), p(r"C:\a.zip"));
        }
    }
}

/// The key of Windows' "Choose your app mode" (`AppsUseLightTheme`), under HKEY_CURRENT_USER.
/// For tests, `MDM_PERSONALIZE_KEY` names another key to read it from (and to watch), so a check
/// can switch the mode MDM sees without touching the system's.
fn personalize_key() -> String {
    std::env::var("MDM_PERSONALIZE_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize".into())
}

/// Windows' "Choose your app mode": true when apps should be dark (`AppsUseLightTheme` = 0).
/// Missing value (older Windows): light. A registry read, which can wait for seconds (#719): for
/// a thread other than the window's (`lookwatch`).
pub fn apps_use_dark() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key: Vec<u16> = personalize_key().encode_utf16().chain([0]).collect();
    let name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
    let mut value: u32 = 1;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: plain registry read into a u32 of the stated size.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut value as *mut u32 as *mut c_void,
            &mut size,
        )
    };
    rc == 0 && value == 0
}

/// Windows' accent colour and its shades, from the personalisation settings (`AccentPalette`:
/// 8 colours of 4 bytes, R G B and an unused byte, lightest first: Light3, Light2, Light1, the
/// accent, Dark1, Dark2, Dark3, and one more). None when Windows has none (older versions).
pub fn accent_palette() -> Option<[[u8; 3]; 7]> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RegGetValueW,
    };
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent\0"
        .encode_utf16()
        .collect();
    let name: Vec<u16> = "AccentPalette\0".encode_utf16().collect();
    let mut bytes = [0u8; 32];
    let mut size = bytes.len() as u32;
    // SAFETY: a registry read into a 32-byte buffer of the stated size.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_BINARY,
            std::ptr::null_mut(),
            bytes.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    (rc == 0 && size >= 28).then(|| crate::native::accent_palette_bytes(&bytes))
}

/// Waits for a change to what the theme follows in the registry (#719): the app mode
/// ([`personalize_key`]), the accent colour, high contrast and the system colours, each key
/// watched with `RegNotifyChangeKeyValue`. Opened, armed and waited on by `lookwatch`'s thread,
/// never the window's: those calls can wait as long as a read.
pub struct LookKeys {
    /// The keys open for watching and the event each signals (HKEYs and HANDLEs, kept as numbers
    /// so the watcher can be moved to its thread).
    keys: Vec<(isize, isize)>,
    /// Their notifications are registered (each fires once, and is registered again then).
    armed: bool,
}

impl LookKeys {
    /// The keys that exist (the accent's is missing on older Windows).
    pub fn open() -> Self {
        use windows_sys::Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, RegCloseKey, RegOpenKeyExW,
        };
        use windows_sys::Win32::System::Threading::CreateEventW;
        let paths = [
            personalize_key(),
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent".into(),
            r"Control Panel\Accessibility\HighContrast".into(),
            r"Control Panel\Colors".into(),
        ];
        let keys = paths
            .iter()
            .filter_map(|p| {
                let wide: Vec<u16> = p.encode_utf16().chain([0]).collect();
                let mut h: HKEY = std::ptr::null_mut();
                // SAFETY: opens a key into `h`, closed in Drop (or below).
                let rc = unsafe {
                    RegOpenKeyExW(HKEY_CURRENT_USER, wide.as_ptr(), 0, KEY_NOTIFY, &mut h)
                };
                if rc != 0 {
                    return None;
                }
                // SAFETY: an unnamed auto-reset event, not signalled; closed in Drop.
                let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
                if event.is_null() {
                    // SAFETY: the key just opened, closed once.
                    unsafe { RegCloseKey(h) };
                    return None;
                }
                Some((h as isize, event as isize))
            })
            .collect();
        LookKeys { keys, armed: false }
    }

    /// Wait until one of the keys changed, or `limit` passed (what the keys miss is read then).
    pub fn wait(&mut self, limit: std::time::Duration) {
        use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Registry::{
            REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_THREAD_AGNOSTIC, RegNotifyChangeKeyValue,
        };
        use windows_sys::Win32::System::Threading::WaitForMultipleObjects;
        if self.keys.is_empty() {
            std::thread::sleep(limit);
            return;
        }
        let arm = |(key, event): (isize, isize)| {
            // SAFETY: an open key and its event; the notification is asynchronous, fires once and
            // ends when the key is closed.
            unsafe {
                RegNotifyChangeKeyValue(
                    key as _,
                    0,
                    REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_THREAD_AGNOSTIC,
                    event as _,
                    1,
                )
            };
        };
        if !self.armed {
            self.keys.iter().copied().for_each(arm);
            self.armed = true;
        }
        let events: Vec<HANDLE> = self.keys.iter().map(|&(_, e)| e as HANDLE).collect();
        let ms = limit.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        // SAFETY: our own events, as many as the count says.
        let rc = unsafe { WaitForMultipleObjects(events.len() as u32, events.as_ptr(), 0, ms) };
        let fired = rc.wrapping_sub(WAIT_OBJECT_0) as usize;
        if let Some(&k) = self.keys.get(fired) {
            // Registered again before the read that follows, so a change during it is seen too.
            arm(k);
        }
    }
}

impl Drop for LookKeys {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Registry::RegCloseKey;
        for &(key, event) in &self.keys {
            // SAFETY: a key and an event made in `open`, each closed once.
            unsafe {
                RegCloseKey(key as _);
                CloseHandle(event as _);
            }
        }
    }
}

/// Windows' high-contrast colours while high contrast is on (Settings → Accessibility →
/// Contrast themes): background, text, selection, selection's text, links, greyed text. None
/// while it is off.
pub fn high_contrast_colors() -> Option<[egui::Color32; 6]> {
    use windows_sys::Win32::Graphics::Gdi::{
        COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT, COLOR_WINDOW,
        COLOR_WINDOWTEXT, GetSysColor,
    };
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};
    // SAFETY: HIGHCONTRASTW is plain data; its size is set as the call requires.
    let mut hc: HIGHCONTRASTW = unsafe { zeroed() };
    hc.cbSize = size_of::<HIGHCONTRASTW>() as u32;
    // SAFETY: a read into the struct above, of the size it states.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            &mut hc as *mut HIGHCONTRASTW as *mut c_void,
            0,
        )
    };
    if ok == 0 || hc.dwFlags & HCF_HIGHCONTRASTON == 0 {
        return None;
    }
    let color = |i| {
        // SAFETY: a plain call; COLORREF is 0x00BBGGRR.
        let c = unsafe { GetSysColor(i) };
        egui::Color32::from_rgb(c as u8, (c >> 8) as u8, (c >> 16) as u8)
    };
    Some([
        color(COLOR_WINDOW),
        color(COLOR_WINDOWTEXT),
        color(COLOR_HIGHLIGHT),
        color(COLOR_HIGHLIGHTTEXT),
        color(COLOR_HOTLIGHT),
        color(COLOR_GRAYTEXT),
    ])
}

/// Sleep now (not hibernate: rundll32's SetSuspendState read its own window handle as
/// "hibernate"). `force`: don't let programs delay it. False when Windows refused.
pub fn sleep(force: bool) -> bool {
    use windows_sys::Win32::System::Power::SetSuspendState;
    // SAFETY: a plain call; FALSE / force / FALSE (no wake events disabled).
    unsafe { SetSuspendState(false, force, false) }
}

/// Is this screen point (a saved window's corner, in points) on a monitor now? A window put
/// back where a since-removed screen was would open out of sight.
pub fn on_a_screen(x: i32, y: i32) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
    // SAFETY: a plain query.
    !unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONULL) }.is_null()
}

/// Whether an assistive technology (a screen reader, a UI Automation client) is listening to
/// this desktop's windows now (#131): the windows then keep their accessibility tree ready, so
/// a window read for the first time answers with its contents at once.
pub fn assistive_technology_listening() -> bool {
    // SAFETY: a plain query, no arguments.
    unsafe { windows_sys::Win32::UI::Accessibility::UiaClientsAreListening() != 0 }
}

/// Rows a notch of the mouse wheel scrolls (Mouse settings → "lines to scroll at a time", 3
/// unless changed); None when it is set to a screen at a time.
pub fn wheel_lines() -> Option<f32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETWHEELSCROLLLINES, SystemParametersInfoW,
    };
    let mut lines: u32 = 3;
    // SAFETY: a plain query into a u32 it writes.
    let ok =
        unsafe { SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, (&raw mut lines).cast(), 0) };
    match lines {
        _ if ok == 0 => Some(3.0),
        // WHEEL_PAGESCROLL
        u32::MAX => None,
        n => Some(n as f32),
    }
}

/// Seconds since the last keyboard or mouse input anywhere ("limit while I'm using the
/// computer").
pub fn idle_seconds() -> u64 {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` is a LASTINPUTINFO with its size set, as the call requires.
    let (ok, now) = unsafe { (GetLastInputInfo(&mut info), GetTickCount()) };
    if ok == 0 {
        return 0;
    }
    u64::from(now.wrapping_sub(info.dwTime) / 1000)
}

// ---- taskbar progress (ITaskbarList3) -----------------------------------------------------
//
// windows-sys has no COM interface tables, so the three calls used are declared here in
// ITaskbarList3's vtable order: IUnknown (3), ITaskbarList (HrInit, AddTab, DeleteTab,
// ActivateTab, SetActiveAlt), ITaskbarList2 (MarkFullscreenWindow), ITaskbarList3
// (SetProgressValue, SetProgressState, …).

#[repr(C)]
struct TaskbarVtbl {
    _query_interface: usize,
    _add_ref: usize,
    _release: usize,
    hr_init: unsafe extern "system" fn(*mut c_void) -> i32,
    _add_tab: usize,
    _delete_tab: usize,
    _activate_tab: usize,
    _set_active_alt: usize,
    _mark_fullscreen_window: usize,
    set_progress_value: unsafe extern "system" fn(*mut c_void, HWND, u64, u64) -> i32,
    set_progress_state: unsafe extern "system" fn(*mut c_void, HWND, u32) -> i32,
}

/// The taskbar object (created on first use on the UI thread) and the last value shown.
struct Taskbar {
    obj: *mut *const TaskbarVtbl,
    shown: Option<Option<u32>>,
}

thread_local! {
    static TASKBAR: std::cell::RefCell<Option<Taskbar>> = const { std::cell::RefCell::new(None) };
}

/// The window's taskbar button shows `progress` (0..=1, the green bar), or nothing (None).
/// Only changes (in 1/1000 steps) reach the shell.
pub fn taskbar_progress(title: &str, progress: Option<f64>) {
    use windows_sys::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows_sys::core::GUID;
    const CLSID_TASKBARLIST: GUID = GUID::from_u128(0x56fdf344_fd6d_11d0_958a_006097c9a090);
    const IID_ITASKBARLIST3: GUID = GUID::from_u128(0xea1afb91_9e28_4b86_90e9_9e9f8a5eefaf);
    const TBPF_NOPROGRESS: u32 = 0;
    const TBPF_NORMAL: u32 = 2;
    let value = progress.map(|p| (p.clamp(0.0, 1.0) * 1000.0).round() as u32);
    TASKBAR.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let mut obj: *mut c_void = std::ptr::null_mut();
            // SAFETY: COM on the UI thread (a second CoInitializeEx is harmless); the out
            // pointer receives an ITaskbarList3 on success, checked before use.
            unsafe {
                CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
                let hr = CoCreateInstance(
                    &CLSID_TASKBARLIST,
                    std::ptr::null_mut(),
                    CLSCTX_INPROC_SERVER,
                    &IID_ITASKBARLIST3,
                    &mut obj,
                );
                if hr < 0 || obj.is_null() {
                    return;
                }
                let tb = obj as *mut *const TaskbarVtbl;
                ((**tb).hr_init)(obj);
                *slot = Some(Taskbar {
                    obj: tb,
                    shown: None,
                });
            }
        }
        let Some(tb) = slot.as_mut() else { return };
        if tb.shown == Some(value) {
            return;
        }
        let Some(hwnd) = find_own_window(title) else {
            return;
        };
        // SAFETY: `obj` is a live ITaskbarList3 (never released: it lives as long as the UI
        // thread); the calls match the vtable above.
        unsafe {
            let this = tb.obj as *mut c_void;
            match value {
                Some(v) => {
                    ((**tb.obj).set_progress_state)(this, hwnd, TBPF_NORMAL);
                    ((**tb.obj).set_progress_value)(this, hwnd, u64::from(v), 1000);
                }
                None => {
                    ((**tb.obj).set_progress_state)(this, hwnd, TBPF_NOPROGRESS);
                }
            }
        }
        tb.shown = Some(value);
    });
}

/// A REG_SZ / REG_DWORD value under HKEY_CURRENT_USER\`key` (None when absent).
fn reg_string(key: &str, name: &str) -> Option<String> {
    reg_string_in(
        windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
        key,
        name,
    )
}

/// The language the installer's wizard ran in (tools/installer/mdm.iss writes it on every
/// install), taken once: read, then deleted. A languages.tsv code (`pt-br`); Inno's language
/// names have `_` for `-`.
pub fn take_install_language() -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
    const KEY: &str = r"Software\MisterDownloadManager";
    const NAME: &str = "InstallLanguage";
    let value = reg_string(KEY, NAME)?;
    let w = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let (key, name) = (w(KEY), w(NAME));
    // SAFETY: NUL-terminated key and value names.
    unsafe {
        RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
    }
    Some(install_language_code(&value))
}

/// An Inno language name (`lang_pt_br`, tools/installer/languages.py) as a languages.tsv code.
fn install_language_code(inno: &str) -> String {
    inno.strip_prefix("lang_").unwrap_or(inno).replace('_', "-")
}

#[cfg(test)]
mod install_language_tests {
    #[test]
    fn inno_names_become_codes() {
        assert_eq!(super::install_language_code("lang_pt_br"), "pt-br");
        assert_eq!(super::install_language_code("lang_or"), "or");
        assert_eq!(
            super::install_language_code("lang_ca_es_valencia"),
            "ca-es-valencia"
        );
    }
}

/// The user's Windows region (Control Panel → Region): its short date's order, whether its
/// clock is 24-hour, and the first day of the week (0 = Sunday). None if Windows can't say.
pub fn region_formats() -> Option<(crate::regional::DateOrder, bool, u8)> {
    use windows_sys::Win32::Globalization::{
        GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK, LOCALE_SSHORTDATE, LOCALE_STIMEFORMAT,
    };
    let info = |what: u32| -> Option<String> {
        let mut buf = [0u16; 80];
        // SAFETY: null locale = the user's; the buffer's length is passed in characters.
        let n =
            unsafe { GetLocaleInfoEx(std::ptr::null(), what, buf.as_mut_ptr(), buf.len() as i32) };
        (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
    };
    let date = info(LOCALE_SSHORTDATE)?;
    let at = |c: char| date.find(c).unwrap_or(usize::MAX);
    let (d, m, y) = (at('d'), at('M'), at('y'));
    let order = if y < m && y < d {
        crate::regional::DateOrder::Ymd
    } else if m < d {
        crate::regional::DateOrder::Mdy
    } else {
        crate::regional::DateOrder::Dmy
    };
    // "HH:mm:ss" (24-hour) or "h:mm:ss tt".
    let clock24 = info(LOCALE_STIMEFORMAT).is_some_and(|t| t.contains('H'));
    // Windows counts from Monday (0) to Sunday (6).
    let first = info(LOCALE_IFIRSTDAYOFWEEK)
        .and_then(|s| s.trim().parse::<u8>().ok())
        .map_or(0, |n| (n + 1) % 7);
    Some((order, clock24, first))
}

/// `t` (local time) as the user's Windows region writes a short date and a time with seconds
/// (Control Panel → Region): `30/10/2026 00:38:09`, `2026/10/30 0:38:09`… None if Windows can't.
pub fn local_date_time(t: chrono::NaiveDateTime) -> Option<String> {
    use chrono::{Datelike, Timelike};
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx};
    let st = SYSTEMTIME {
        wYear: t.year() as u16,
        wMonth: t.month() as u16,
        wDayOfWeek: t.weekday().num_days_from_sunday() as u16,
        wDay: t.day() as u16,
        wHour: t.hour() as u16,
        wMinute: t.minute() as u16,
        wSecond: t.second() as u16,
        wMilliseconds: 0,
    };
    let mut date = [0u16; 80];
    let mut time = [0u16; 80];
    // SAFETY: null locale = the user's; the buffers' lengths are passed in characters.
    let (d, h) = unsafe {
        (
            GetDateFormatEx(
                std::ptr::null(),
                DATE_SHORTDATE,
                &st,
                std::ptr::null(),
                date.as_mut_ptr(),
                date.len() as i32,
                std::ptr::null(),
            ),
            GetTimeFormatEx(
                std::ptr::null(),
                0,
                &st,
                std::ptr::null(),
                time.as_mut_ptr(),
                time.len() as i32,
            ),
        )
    };
    if d <= 1 || h <= 1 {
        return None;
    }
    let date = String::from_utf16_lossy(&date[..d as usize - 1]);
    let time = String::from_utf16_lossy(&time[..h as usize - 1]);
    Some(format!("{date} {time}"))
}

/// Windows' edition, release and build, e.g. "Windows 11 Pro 24H2 (build 26200)" — for problem
/// reports. ProductName still says "Windows 10" on 11; the build tells them apart.
pub fn windows_version() -> String {
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let get = |n| reg_string_in(HKEY_LOCAL_MACHINE, KEY, n);
    let build = get("CurrentBuild").unwrap_or_default();
    let mut product = get("ProductName").unwrap_or_else(|| "Windows".into());
    if build.parse::<u32>().is_ok_and(|b| b >= 22000) {
        product = product.replace("Windows 10", "Windows 11");
    }
    let release = get("DisplayVersion")
        .map(|v| format!(" {v}"))
        .unwrap_or_default();
    format!("{product}{release} (build {build})")
}

/// The default browser (#668): the program https links open with (the user's choice in Default
/// apps, `UserChoice`'s ProgId, and that ProgId's `shell\open\command`), and its file's version.
/// None when it isn't a browser MDM knows (MDM then speaks as Chrome).
pub fn default_browser() -> Option<mdm_engine::agent::Found> {
    use mdm_engine::agent::{Browser, Found};
    use windows_sys::Win32::System::Registry::HKEY_CLASSES_ROOT;
    const HTTPS: &str = r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https";
    // Windows 11 keeps the choice in UserChoiceLatest since 2025, its ProgId in a key of its
    // own (UserChoiceLatest\ProgId's ProgId); UserChoice stays, and may be an older choice.
    let prog_id = [r"UserChoiceLatest\ProgId", "UserChoiceLatest", "UserChoice"]
        .iter()
        .find_map(|k| {
            reg_string(&format!(r"{HTTPS}\{k}"), "ProgId").filter(|p| !p.trim().is_empty())
        })?;
    let command = reg_string_in(
        HKEY_CLASSES_ROOT,
        &format!(r"{prog_id}\shell\open\command"),
        "",
    )?;
    let exe = std::path::PathBuf::from(crate::defaultbrowser::windows_program(&command)?);
    let stem = exe.file_stem()?.to_string_lossy().into_owned();
    // Opera starts through its launcher.exe, in a folder named after it ("Opera", "Opera GX").
    let name = if stem.eq_ignore_ascii_case("launcher") {
        exe.parent()?.file_name()?.to_string_lossy().into_owned()
    } else {
        stem
    };
    let browser = Browser::named(&name).or_else(|| Browser::named(&prog_id))?;
    Some(Found {
        browser,
        version: file_version(&exe),
        ubuntu: false,
    })
}

/// A program's file version ("157.1.99.55"): its version resource's fixed part.
fn file_version(exe: &std::path::Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    let path: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated path; the handle argument is optional and ignored.
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut()) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: `data` holds `size` bytes, as GetFileVersionInfoSizeW asked.
    if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let root: Vec<u16> = "\\".encode_utf16().chain(Some(0)).collect();
    let (mut info, mut len) = (std::ptr::null_mut::<c_void>(), 0u32);
    // SAFETY: `data` is the version block just read; the pointer returned points into it.
    let ok = unsafe { VerQueryValueW(data.as_ptr().cast(), root.as_ptr(), &mut info, &mut len) };
    if ok == 0 || info.is_null() || (len as usize) < size_of::<VS_FIXEDFILEINFO>() {
        return None;
    }
    // SAFETY: VerQueryValueW returned a VS_FIXEDFILEINFO of `len` bytes inside `data`, which is
    // still alive; read unaligned, as a byte buffer promises no alignment.
    let fixed = unsafe { std::ptr::read_unaligned(info.cast::<VS_FIXEDFILEINFO>()) };
    let (ms, ls) = (fixed.dwFileVersionMS, fixed.dwFileVersionLS);
    Some(format!(
        "{}.{}.{}.{}",
        ms >> 16,
        ms & 0xffff,
        ls >> 16,
        ls & 0xffff
    ))
}

fn reg_string_in(
    root: windows_sys::Win32::System::Registry::HKEY,
    key: &str,
    name: &str,
) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: registry read into `buf`, whose size in bytes is passed in `size`.
    let rc = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    if rc != 0 {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn reg_dword(key: &str, name: &str) -> Option<u32> {
    reg_dword_in(
        windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
        key,
        name,
    )
}

fn reg_dword_in(
    root: windows_sys::Win32::System::Registry::HKEY,
    key: &str,
    name: &str,
) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_DWORD, RegGetValueW};
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: plain registry read into a u32 of the stated size.
    let rc = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut value as *mut u32 as *mut c_void,
            &mut size,
        )
    };
    (rc == 0).then_some(value)
}

/// Windows' manual proxy (Settings → Network → Proxy), as (host, port), when it is on.
/// `ProxyServer` is "host:port" or "http=host:port;https=…": the http (else first) entry.
pub fn system_proxy() -> Option<(String, String)> {
    const KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";
    if reg_dword(KEY, "ProxyEnable")? == 0 {
        return None;
    }
    let server = reg_string(KEY, "ProxyServer")?;
    let entries: Vec<&str> = server.split(';').map(str::trim).collect();
    let pick = entries
        .iter()
        .find_map(|e| e.strip_prefix("http="))
        .or_else(|| entries.iter().find(|e| !e.contains('=')).copied())
        .or_else(|| {
            entries
                .first()
                .and_then(|e| e.split_once('='))
                .map(|(_, v)| v)
        })?;
    let (host, port) = pick.rsplit_once(':')?;
    Some((host.to_owned(), port.to_owned()))
}

/// Play a .wav file without waiting (Options → Sounds).
pub fn play_sound(file: &str) {
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT};
    let wide: Vec<u16> = file.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated path; SND_ASYNC copies what it needs before returning.
    unsafe {
        PlaySoundW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            SND_FILENAME | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

/// Options → General → "Launch … on startup": our entry under HKCU\…\Run (set or removed).
/// Make MDM (on) or stop it being (off) what Windows opens magnet links and .torrent files
/// with, for the current user (HKCU\Software\Classes; no admin). Off removes only what points
/// at us. Windows keeps a choice the user made in "Open with" above this (it can't be set by
/// programs), so another app chosen there stays in charge until the user picks MDM.
pub fn set_torrent_handlers(on: bool) {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW,
        RegSetKeyValueW,
    };
    const PROGID: &str = "MisterDownloadManager.Torrent";
    let w = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let set = |key: &str, name: Option<&str>, value: &str| {
        let (key, value) = (w(key), w(value));
        let name = name.map(w);
        // SAFETY: NUL-terminated strings; the size covers the value's bytes.
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                REG_SZ,
                value.as_ptr() as *const c_void,
                (value.len() * 2) as u32,
            );
        }
    };
    let get = |key: &str, name: Option<&str>| -> Option<String> {
        let key = w(key);
        let name = name.map(w);
        let mut buf = vec![0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: NUL-terminated key/name; `len` is the buffer's size in bytes.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut c_void,
                &mut len,
            )
        };
        (r == 0).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
    };
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let exe = exe.display().to_string();
    let open = format!("\"{exe}\" \"%1\"");
    let icon = format!("\"{exe}\",0");
    if on {
        set("Software\\Classes\\magnet", None, "URL:Magnet link");
        set("Software\\Classes\\magnet", Some("URL Protocol"), "");
        set("Software\\Classes\\magnet\\DefaultIcon", None, &icon);
        set(
            "Software\\Classes\\magnet\\shell\\open\\command",
            None,
            &open,
        );
        let prog = format!("Software\\Classes\\{PROGID}");
        set(&prog, None, "Torrent file");
        set(&format!("{prog}\\DefaultIcon"), None, &icon);
        set(&format!("{prog}\\shell\\open\\command"), None, &open);
        set("Software\\Classes\\.torrent", None, PROGID);
    } else {
        let ours = get("Software\\Classes\\magnet\\shell\\open\\command", None)
            .is_some_and(|c| c.contains(&exe));
        let tree = |k: &str| {
            let k = w(k);
            // SAFETY: NUL-terminated key under HKCU.
            unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
        };
        if ours {
            tree("Software\\Classes\\magnet");
        }
        tree(&format!("Software\\Classes\\{PROGID}"));
        if get("Software\\Classes\\.torrent", None).as_deref() == Some(PROGID) {
            let (k, n) = (w("Software\\Classes\\.torrent"), w(""));
            // SAFETY: NUL-terminated key; an empty name is the default value.
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) };
        }
    }
    // Tell Explorer the associations changed.
    // SAFETY: no pointers are passed.
    unsafe {
        windows_sys::Win32::UI::Shell::SHChangeNotify(
            windows_sys::Win32::UI::Shell::SHCNE_ASSOCCHANGED as i32,
            windows_sys::Win32::UI::Shell::SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

/// The Run key's value MDM keeps its startup entry in (Options → "Launch on startup", the
/// installer's "Start when Windows starts"). An off-screen test copy never touches the user's:
/// it uses `MDM_TEST_RUN_VALUE`'s name when given one, else none at all (#226's checks).
fn run_value() -> Option<String> {
    if crate::offscreen() {
        return std::env::var("MDM_TEST_RUN_VALUE")
            .ok()
            .filter(|v| v.starts_with("MisterDownloadManagerTest"));
    }
    Some("MisterDownloadManager".to_owned())
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The startup entry's command for the program `exe`: started with the session, MDM waits in
/// the notification area (`/startup`, #226).
fn startup_command(exe: &std::path::Path) -> String {
    format!("\"{}\" {}", exe.display(), mdm_ipc::STARTUP)
}

/// A startup entry `value` (a command line) written before #226 for the program `exe`: the same
/// program, without the argument. Another program's entry (a copy of MDM elsewhere) is not this
/// one's to rewrite.
fn startup_outdated(value: &str, exe: &std::path::Path) -> bool {
    // The program compared as Windows compares paths: in any case, either slash.
    let norm = |s: &str| s.replace('/', "\\").to_lowercase();
    let exe = norm(&exe.display().to_string());
    let value = value.trim();
    let args = match value.strip_prefix('"') {
        Some(rest) => match rest.split_once('"') {
            Some((program, args)) if norm(program) == exe => args,
            _ => return false,
        },
        // Unquoted (written by hand): the program's path, spaces and all, then its arguments.
        None => match value.get(..exe.len()).zip(value.get(exe.len()..)) {
            Some((program, args))
                if norm(program) == exe && (args.is_empty() || args.starts_with(' ')) =>
            {
                args
            }
            _ => return false,
        },
    };
    !args
        .split_whitespace()
        .any(|a| a.eq_ignore_ascii_case(mdm_ipc::STARTUP))
}

/// Is Mister Download Manager in the user's Run key (set by Options or by the installer's
/// "Start when Windows starts")? The registry is the truth for Options → "Launch on startup".
pub fn run_at_startup() -> bool {
    run_value().is_some_and(|name| {
        reg_string_in(
            windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
            RUN_KEY,
            &name,
        )
        .is_some()
    })
}

/// The startup entry rewritten with its argument when it was written without (by versions
/// before 0.5.0, or by their installer) for this very program (#226).
pub fn update_run_at_startup() {
    let (Some(name), Ok(exe)) = (run_value(), std::env::current_exe()) else {
        return;
    };
    let value = reg_string_in(
        windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
        RUN_KEY,
        &name,
    );
    if value.is_some_and(|v| startup_outdated(&v, &exe)) {
        tracing::info!("startup entry written again, to start in the notification area");
        set_run_at_startup(true);
    }
}

pub fn set_run_at_startup(on: bool) {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
    };
    let Some(name) = run_value() else {
        return;
    };
    let key: Vec<u16> = RUN_KEY.encode_utf16().chain(Some(0)).collect();
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    if on {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let value: Vec<u16> = startup_command(&exe)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: NUL-terminated key, name and value; the size covers the value's bytes.
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr() as *const c_void,
                (value.len() * 2) as u32,
            );
        }
    } else {
        // SAFETY: NUL-terminated key and value name.
        unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
        }
    }
}

#[cfg(test)]
mod startup_tests {
    use super::{startup_command, startup_outdated};
    use std::path::Path;

    const EXE: &str = r"C:\Program Files\Mister Download Manager\misterdownloadmanager.exe";

    /// The entry Options and the installer write (#226): the program, quoted, then `/startup`.
    #[test]
    fn the_startup_entry_starts_mdm_in_the_tray() {
        let cmd = startup_command(Path::new(EXE));
        assert_eq!(
            cmd,
            format!("\"{EXE}\" /startup"),
            "as tools/installer/mdm.iss writes it too"
        );
        let args = mdm_ipc::parse_args(&["/startup".to_owned()]).unwrap();
        assert!(args.startup);
        assert!(!startup_outdated(&cmd, Path::new(EXE)));
    }

    /// An entry of a version before 0.5.0, or of its installer, is written again; one for another
    /// copy of MDM, or one already up to date, is left alone.
    #[test]
    fn old_startup_entries_are_rewritten() {
        let exe = Path::new(EXE);
        // Options before 0.5.0, and the installer's: the program alone, quoted.
        assert!(startup_outdated(&format!("\"{EXE}\""), exe));
        // Written by hand: unquoted, in another case, with other slashes.
        assert!(startup_outdated(
            r"c:\program files\mister download manager\MISTERDOWNLOADMANAGER.EXE",
            exe
        ));
        assert!(startup_outdated(
            &format!("\"{}\"", EXE.replace('\\', "/").to_uppercase()),
            exe
        ));
        assert!(!startup_outdated(&format!("{EXE}2"), exe));
        // Already with it (any case).
        assert!(!startup_outdated(&format!("\"{EXE}\" /startup"), exe));
        assert!(!startup_outdated(&format!("\"{EXE}\" /STARTUP"), exe));
        // Another copy's (a portable one, a build elsewhere).
        assert!(!startup_outdated(
            r#""D:\mdm\misterdownloadmanager.exe""#,
            exe
        ));
        assert!(!startup_outdated("", exe));
    }

    /// The installer's "Start when Windows starts" writes the entry Options writes.
    #[test]
    fn the_installer_writes_the_same_entry() {
        let iss = include_str!("../../../tools/installer/mdm.iss");
        assert!(iss.contains(
            r#"ValueName: "MisterDownloadManager"; ValueData: """{app}\{#AppExe}"" /startup""#
        ));
    }
}

/// The work area (left, top, right, bottom) of the monitor nearest to screen point (x, y).
pub fn work_area(x: i32, y: i32) -> Option<[i32; 4]> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    };
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        (GetMonitorInfoW(mon, &mut mi) != 0).then_some([
            mi.rcWork.left,
            mi.rcWork.top,
            mi.rcWork.right,
            mi.rcWork.bottom,
        ])
    }
}

/// The shell's icon for a file type or a folder (fileicons.rs), as Explorer shows it, from the
/// system image list that fits `px` pixels: small, large, extra large (16, 32, 48 at 100 %, more
/// with the system's scale), then jumbo (256) for anything bigger. The file need not exist. Run
/// on fileicons.rs's lookup thread: the first lookup of a type loads its shell handler.
///
/// One lookup at a time in the process (the drag picture asks from the UI thread): two threads
/// in the shell at once could miss an icon. A miss is tried once more (a handler's first load).
pub fn file_icon(what: &crate::fileicons::What, px: u32) -> Option<crate::fileicons::Native> {
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    file_icon_once(what, px).or_else(|| file_icon_once(what, px))
}

fn file_icon_once(what: &crate::fileicons::What, px: u32) -> Option<crate::fileicons::Native> {
    use crate::fileicons::What;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
    use windows_sys::Win32::UI::Shell::SHGFI_SYSICONINDEX;
    com_on_this_thread();
    let (name, attrs) = match what {
        What::FolderOpen => return list_icon(open_folder_index()?, px),
        What::Folder => ("folder".to_string(), FILE_ATTRIBUTE_DIRECTORY),
        What::File(Some(ext)) => (format!("file.{ext}"), FILE_ATTRIBUTE_NORMAL),
        What::File(None) => ("file".to_string(), FILE_ATTRIBUTE_NORMAL),
    };
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    // SAFETY: `info` is a zeroed SHFILEINFOW of the stated size; the name is NUL-terminated;
    // with SHGFI_USEFILEATTRIBUTES the file need not exist. SYSICONINDEX returns no icon to free.
    let index = unsafe {
        let mut info: SHFILEINFOW = zeroed();
        let ok = SHGetFileInfoW(
            wide.as_ptr(),
            attrs,
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES,
        );
        if ok == 0 {
            return None;
        }
        info.iIcon
    };
    list_icon(index, px)
}

/// The system image lists' icon `index` at the list that fits `px` pixels (see `file_icon`).
fn list_icon(index: i32, px: u32) -> Option<crate::fileicons::Native> {
    use windows_sys::Win32::UI::Shell::{SHIL_EXTRALARGE, SHIL_JUMBO, SHIL_LARGE, SHIL_SMALL};
    let mut got: Option<(u32, Vec<u8>)> = None;
    for list in [SHIL_SMALL, SHIL_LARGE, SHIL_EXTRALARGE, SHIL_JUMBO] {
        if got.as_ref().is_some_and(|(side, _)| *side >= px) {
            break;
        }
        let Some((side, rgba)) = image_list_icon(list, index) else {
            continue;
        };
        // A type with no picture that big gets its smaller one in the jumbo list's top-left
        // corner: keep the smaller one then.
        if list == SHIL_JUMBO
            && let Some((prev, _)) = &got
            && drawn_within(&rgba, side, *prev)
        {
            break;
        }
        got = Some((side, rgba));
    }
    let (side, rgba) = got?;
    Some(crate::fileicons::Native::Rgba {
        side,
        rgba,
        premultiplied: false,
    })
}

/// Whether every visible pixel of the `side`-square `rgba` lies in its top-left `within` square.
fn drawn_within(rgba: &[u8], side: u32, within: u32) -> bool {
    rgba.as_chunks::<4>().0.iter().enumerate().all(|(i, c)| {
        let (x, y) = (i as u32 % side, i as u32 / side);
        c[3] == 0 || (x < within && y < within)
    })
}

/// COM for this thread (apartment-threaded, as the shell's icon handlers expect), once.
fn com_on_this_thread() {
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    DONE.with(|done| {
        if !done.replace(true) {
            // SAFETY: no reserved pointer; the thread keeps COM for its lifetime.
            unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        }
    });
}

/// The start of the shell's `IImageList` vtable (commoncontrols.h), as far as `GetIcon`. The list
/// is called through it rather than comctl32's `ImageList_GetIcon`, which needs the version 6
/// controls to read the shell's lists.
#[repr(C)]
struct ImageListVtbl {
    _query_interface: usize,
    _add_ref: usize,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    // Add, ReplaceIcon, SetOverlayImage, Replace, AddMasked, Draw, Remove.
    _before_get_icon: [usize; 7],
    get_icon: unsafe extern "system" fn(
        *mut c_void,
        i32,
        u32,
        *mut windows_sys::Win32::UI::WindowsAndMessaging::HICON,
    ) -> windows_sys::core::HRESULT,
}

/// IID_IImageList, {46EB5926-582E-4017-9FDF-E8998DAA0950}.
const IID_IMAGE_LIST: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x46EB_5926,
    data2: 0x582E,
    data3: 0x4017,
    data4: [0x9F, 0xDF, 0xE8, 0x99, 0x8D, 0xAA, 0x09, 0x50],
};

/// Icon `index` of the system image list `list` (SHIL_*), as (side, RGBA).
fn image_list_icon(list: u32, index: i32) -> Option<(u32, Vec<u8>)> {
    use windows_sys::Win32::UI::Controls::ILD_TRANSPARENT;
    use windows_sys::Win32::UI::Shell::SHGetImageList;
    // SAFETY: SHGetImageList hands out an IImageList with a reference for us, released below; the
    // vtable is read only as far as GetIcon, whose icon hicon_rgba takes and destroys.
    unsafe {
        let mut list_ptr: *mut c_void = std::ptr::null_mut();
        if SHGetImageList(list as i32, &IID_IMAGE_LIST, &mut list_ptr) < 0 || list_ptr.is_null() {
            return None;
        }
        let vtbl = &**(list_ptr as *const *const ImageListVtbl);
        let mut icon = std::ptr::null_mut();
        let hr = (vtbl.get_icon)(list_ptr, index, ILD_TRANSPARENT, &mut icon);
        (vtbl.release)(list_ptr);
        if hr < 0 || icon.is_null() {
            return None;
        }
        hicon_rgba(icon)
    }
}

/// Windows' type name for a file name ("BIN File", "Compressed (zipped) Folder").
pub fn type_name(file_name: &str) -> Option<String> {
    use windows_sys::Win32::UI::Shell::SHGFI_TYPENAME;
    let name: Vec<u16> = file_name.encode_utf16().chain([0]).collect();
    // SAFETY: `info` is a zeroed SHFILEINFOW of the stated size; the name is NUL-terminated;
    // with SHGFI_USEFILEATTRIBUTES the file need not exist.
    unsafe {
        let mut info: SHFILEINFOW = zeroed();
        let ok = SHGetFileInfoW(
            name.as_ptr(),
            FILE_ATTRIBUTE_NORMAL,
            &mut info,
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES,
        );
        let len = info.szTypeName.iter().position(|c| *c == 0).unwrap_or(0);
        (ok != 0 && len > 0).then(|| String::from_utf16_lossy(&info.szTypeName[..len]))
    }
}

/// `icon`'s pixels as (side, straight RGBA), at the size its bitmap has; the icon is destroyed.
/// An icon without alpha (an old one) takes its transparency from its mask.
unsafe fn hicon_rgba(
    icon: windows_sys::Win32::UI::WindowsAndMessaging::HICON,
) -> Option<(u32, Vec<u8>)> {
    use windows_sys::Win32::Graphics::Gdi::{BITMAP, GetObjectW};
    unsafe {
        let mut ii: ICONINFO = zeroed();
        let got = GetIconInfo(icon, &mut ii) != 0;
        let mut out = None;
        if got && !ii.hbmColor.is_null() {
            let mut bm: BITMAP = zeroed();
            let have = GetObjectW(
                ii.hbmColor as _,
                size_of::<BITMAP>() as i32,
                &mut bm as *mut BITMAP as *mut c_void,
            ) != 0;
            let side = bm.bmWidth;
            if have
                && side > 0
                && side == bm.bmHeight.abs()
                && side <= 1024
                && let Some(mut px) = dib_rgba(ii.hbmColor, side)
            {
                if !px.as_chunks::<4>().0.iter().any(|c| c[3] != 0) {
                    // The mask: white (set) where the icon is transparent.
                    let mask = (!ii.hbmMask.is_null())
                        .then(|| dib_rgba(ii.hbmMask, side))
                        .flatten();
                    for (i, c) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        c[3] = match &mask {
                            Some(m) if m[i * 4] != 0 => 0,
                            _ => 255,
                        };
                    }
                }
                out = Some((side as u32, px));
            }
        }
        if got {
            DeleteObject(ii.hbmColor as _);
            DeleteObject(ii.hbmMask as _);
        }
        DestroyIcon(icon);
        out
    }
}

/// The first `side` × `side` pixels of `bitmap`, top-down, as RGBA (32 bits per pixel).
unsafe fn dib_rgba(
    bitmap: windows_sys::Win32::Graphics::Gdi::HBITMAP,
    side: i32,
) -> Option<Vec<u8>> {
    unsafe {
        let mut bmi: BITMAPINFO = zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: side,
            biHeight: -side, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..zeroed()
        };
        let mut px = vec![0u8; (side * side * 4) as usize];
        let dc = GetDC(std::ptr::null_mut());
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            side as u32,
            px.as_mut_ptr() as *mut c_void,
            &mut bmi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(std::ptr::null_mut(), dc);
        (lines == side).then(|| {
            for c in px.as_chunks_mut::<4>().0 {
                c.swap(0, 2); // BGRA → RGBA
            }
            px
        })
    }
}

/// Windows' own "open folder" (Download complete's, as IDM shows), as its system image list index.
fn open_folder_index() -> Option<i32> {
    use windows_sys::Win32::UI::Shell::{
        SHGSI_SYSICONINDEX, SHGetStockIconInfo, SHSTOCKICONINFO, SIID_FOLDEROPEN,
    };
    // SAFETY: `info` is zeroed with its size set; SYSICONINDEX returns no icon to free.
    unsafe {
        let mut info: SHSTOCKICONINFO = zeroed();
        info.cbSize = size_of::<SHSTOCKICONINFO>() as u32;
        (SHGetStockIconInfo(SIID_FOLDEROPEN, SHGSI_SYSICONINDEX, &mut info) == 0)
            .then_some(info.iSysImageIndex)
    }
}

/// The drop target's OLE side (`droptarget.rs`): a minimal `IDropTarget` that accepts links
/// (UniformResourceLocatorW), text and files.
pub mod ole {

    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, mpsc::Sender};

    use windows_sys::Win32::Foundation::HWND;

    type HResult = i32;
    const S_OK: HResult = 0;
    const E_NOINTERFACE: HResult = 0x8000_4002_u32 as i32;
    const DROPEFFECT_NONE: u32 = 0;
    const DROPEFFECT_COPY: u32 = 1;
    const CF_UNICODETEXT: u16 = 13;
    const CF_HDROP: u16 = 15;
    const TYMED_HGLOBAL: u32 = 1;
    const DVASPECT_CONTENT: u32 = 1;

    #[repr(C)]
    #[derive(PartialEq)]
    struct Guid(u32, u16, u16, [u8; 8]);
    const IID_IUNKNOWN: Guid = Guid(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
    const IID_IDROPTARGET: Guid = Guid(0x0000_0122, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

    #[repr(C)]
    struct PointL {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct FormatEtc {
        cf: u16,
        ptd: *mut c_void,
        aspect: u32,
        lindex: i32,
        tymed: u32,
    }

    #[repr(C)]
    struct StgMedium {
        tymed: u32,
        handle: *mut c_void,
        unk_for_release: *mut c_void,
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn OleInitialize(reserved: *mut c_void) -> HResult;
        fn RegisterDragDrop(hwnd: HWND, target: *mut c_void) -> HResult;
        fn RevokeDragDrop(hwnd: HWND) -> HResult;
        fn ReleaseStgMedium(medium: *mut StgMedium);
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalLock(h: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(h: *mut c_void) -> i32;
        fn GlobalSize(h: *mut c_void) -> usize;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn DragQueryFileW(hdrop: *mut c_void, i: u32, buf: *mut u16, cch: u32) -> u32;
    }

    /// IDataObject's vtable up to GetData (index 3), all we call.
    #[repr(C)]
    struct DataObjectVtbl {
        _query_interface: usize,
        _add_ref: usize,
        _release: usize,
        get_data:
            unsafe extern "system" fn(*mut c_void, *const FormatEtc, *mut StgMedium) -> HResult,
    }

    #[repr(C)]
    struct Vtbl {
        query_interface:
            unsafe extern "system" fn(*mut Target, *const Guid, *mut *mut c_void) -> HResult,
        add_ref: unsafe extern "system" fn(*mut Target) -> u32,
        release: unsafe extern "system" fn(*mut Target) -> u32,
        drag_enter:
            unsafe extern "system" fn(*mut Target, *mut c_void, u32, PointL, *mut u32) -> HResult,
        drag_over: unsafe extern "system" fn(*mut Target, u32, PointL, *mut u32) -> HResult,
        drag_leave: unsafe extern "system" fn(*mut Target) -> HResult,
        drop: unsafe extern "system" fn(*mut Target, *mut c_void, u32, PointL, *mut u32) -> HResult,
    }

    #[repr(C)]
    struct Target {
        vtbl: &'static Vtbl,
        refs: AtomicU32,
        tx: Sender<String>,
        over: Arc<AtomicBool>,
        /// What the current drag carries (decided at DragEnter).
        accepts: AtomicBool,
        /// Woken at enter, leave and drop: OLE's drag loop sends the window no input, so
        /// egui wouldn't draw the highlight, or take the drop, until something else came.
        wake: egui::Context,
    }

    static VTBL: Vtbl = Vtbl {
        query_interface,
        add_ref,
        release,
        drag_enter,
        drag_over,
        drag_leave,
        drop,
    };

    unsafe extern "system" fn query_interface(
        this: *mut Target,
        iid: *const Guid,
        out: *mut *mut c_void,
    ) -> HResult {
        // SAFETY: COM calls with valid pointers.
        unsafe {
            if *iid == IID_IUNKNOWN || *iid == IID_IDROPTARGET {
                add_ref(this);
                *out = this.cast();
                S_OK
            } else {
                *out = std::ptr::null_mut();
                E_NOINTERFACE
            }
        }
    }

    unsafe extern "system" fn add_ref(this: *mut Target) -> u32 {
        // SAFETY: `this` is a live Target (COM reference counting).
        unsafe { (*this).refs.fetch_add(1, Ordering::AcqRel) + 1 }
    }

    unsafe extern "system" fn release(this: *mut Target) -> u32 {
        // SAFETY: the last release frees the Box made in `Registration::new`.
        unsafe {
            let n = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
            if n == 0 {
                std::mem::drop(Box::from_raw(this));
            }
            n
        }
    }

    unsafe extern "system" fn drag_enter(
        this: *mut Target,
        data: *mut c_void,
        _keys: u32,
        _pt: PointL,
        effect: *mut u32,
    ) -> HResult {
        // SAFETY: valid for the call.
        unsafe {
            let ok = !read(data).is_empty();
            (*this).accepts.store(ok, Ordering::Relaxed);
            (*this).over.store(ok, Ordering::Relaxed);
            (*this).wake.request_repaint();
            *effect = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
        }
        S_OK
    }

    unsafe extern "system" fn drag_over(
        this: *mut Target,
        _keys: u32,
        _pt: PointL,
        effect: *mut u32,
    ) -> HResult {
        // SAFETY: valid for the call.
        unsafe {
            *effect = if (*this).accepts.load(Ordering::Relaxed) {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
        }
        S_OK
    }

    unsafe extern "system" fn drag_leave(this: *mut Target) -> HResult {
        // SAFETY: valid for the call.
        unsafe {
            (*this).over.store(false, Ordering::Relaxed);
            (*this).wake.request_repaint();
        }
        S_OK
    }

    unsafe extern "system" fn drop(
        this: *mut Target,
        data: *mut c_void,
        _keys: u32,
        _pt: PointL,
        effect: *mut u32,
    ) -> HResult {
        // SAFETY: valid for the call.
        unsafe {
            (*this).over.store(false, Ordering::Relaxed);
            let found = read(data);
            *effect = if found.is_empty() {
                DROPEFFECT_NONE
            } else {
                DROPEFFECT_COPY
            };
            for s in found {
                let _ = (*this).tx.send(s);
            }
            (*this).wake.request_repaint();
        }
        S_OK
    }

    /// The addresses (or file paths) a drag carries: a link, else text that holds addresses,
    /// else dropped files.
    unsafe fn read(data: *mut c_void) -> Vec<String> {
        // SAFETY: `data` is the IDataObject OLE passed in.
        unsafe {
            let name: Vec<u16> = "UniformResourceLocatorW\0".encode_utf16().collect();
            let url_format = RegisterClipboardFormatW(name.as_ptr()) as u16;
            for cf in [url_format, CF_UNICODETEXT] {
                if let Some(text) = get_text(data, cf) {
                    let urls: Vec<String> = text
                        .split_whitespace()
                        .filter(|w| {
                            let w = w.to_ascii_lowercase();
                            w.starts_with("http://")
                                || w.starts_with("https://")
                                || w.starts_with("ftp://")
                                || w.starts_with("ftps://")
                                || w.starts_with("magnet:")
                        })
                        .map(str::to_owned)
                        .collect();
                    if !urls.is_empty() {
                        return urls;
                    }
                }
            }
            files(data)
        }
    }

    unsafe fn medium(data: *mut c_void, cf: u16) -> Option<StgMedium> {
        // SAFETY: IDataObject::GetData through its vtable.
        unsafe {
            let vtbl = *(data as *const *const DataObjectVtbl);
            let fmt = FormatEtc {
                cf,
                ptd: std::ptr::null_mut(),
                aspect: DVASPECT_CONTENT,
                lindex: -1,
                tymed: TYMED_HGLOBAL,
            };
            let mut m = StgMedium {
                tymed: 0,
                handle: std::ptr::null_mut(),
                unk_for_release: std::ptr::null_mut(),
            };
            (((*vtbl).get_data)(data, &fmt, &mut m) == S_OK && !m.handle.is_null()).then_some(m)
        }
    }

    unsafe fn get_text(data: *mut c_void, cf: u16) -> Option<String> {
        // SAFETY: a locked HGLOBAL of UTF-16, read within its size, then released.
        unsafe {
            let mut m = medium(data, cf)?;
            let p = GlobalLock(m.handle) as *const u16;
            let text = (!p.is_null()).then(|| {
                let n = GlobalSize(m.handle) / 2;
                let s = std::slice::from_raw_parts(p, n);
                let end = s.iter().position(|c| *c == 0).unwrap_or(n);
                String::from_utf16_lossy(&s[..end])
            });
            GlobalUnlock(m.handle);
            ReleaseStgMedium(&mut m);
            text
        }
    }

    unsafe fn files(data: *mut c_void) -> Vec<String> {
        // SAFETY: an HDROP medium, queried with DragQueryFileW, then released.
        unsafe {
            let Some(mut m) = medium(data, CF_HDROP) else {
                return Vec::new();
            };
            let n = DragQueryFileW(m.handle, u32::MAX, std::ptr::null_mut(), 0);
            let mut out = Vec::new();
            for i in 0..n {
                let len = DragQueryFileW(m.handle, i, std::ptr::null_mut(), 0);
                let mut buf = vec![0u16; len as usize + 1];
                DragQueryFileW(m.handle, i, buf.as_mut_ptr(), len + 1);
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
            ReleaseStgMedium(&mut m);
            out
        }
    }

    /// Our target registered on the drop window; revoked when dropped.
    pub struct Registration {
        hwnd: HWND,
        target: *mut Target,
    }

    impl Registration {
        /// None until the window exists (tried again next frame).
        pub fn new(
            title: &str,
            tx: Sender<String>,
            over: Arc<AtomicBool>,
            wake: &egui::Context,
        ) -> Option<Self> {
            let hwnd = crate::win32::find_own_window(title)?;
            let target = Box::into_raw(Box::new(Target {
                vtbl: &VTBL,
                refs: AtomicU32::new(1),
                tx,
                over,
                accepts: AtomicBool::new(false),
                wake: wake.clone(),
            }));
            // SAFETY: OLE on the UI thread (winit initialised it already: S_FALSE is fine);
            // any target winit registered goes first, then ours (RegisterDragDrop AddRefs it).
            unsafe {
                OleInitialize(std::ptr::null_mut());
                RevokeDragDrop(hwnd);
                if RegisterDragDrop(hwnd, target.cast()) != S_OK {
                    release(target);
                    return None;
                }
            }
            Some(Registration { hwnd, target })
        }

        /// Off-screen test copies only (MDM_TEST_DROP, droptarget.rs): `text` dropped on the
        /// window as a browser drops a link, without a mouse: a data object holding it as
        /// "UniformResourceLocatorW" goes to the drop target OLE has registered for the window
        /// (DragEnter, then Drop), as OLE's drag loop hands it. False when the window holds no
        /// registration of ours.
        pub fn test_drop(&self, text: &str) -> bool {
            use windows_sys::Win32::UI::WindowsAndMessaging::GetPropW;
            // OLE keeps the target registered for a window in this property of it (in the
            // process that registered it: the target itself).
            let prop: Vec<u16> = "OleDropTargetInterface\0".encode_utf16().collect();
            // SAFETY: a property read on our own window.
            let registered = unsafe { GetPropW(self.hwnd, prop.as_ptr()) };
            if registered != self.target.cast() {
                tracing::warn!("test drop: the window holds no drop target of ours");
                return false;
            }
            let data = Box::into_raw(Box::new(TestData {
                vtbl: &TEST_DATA_VTBL,
                text: text.encode_utf16().chain(Some(0)).collect(),
            }));
            let target = registered as *mut Target;
            let mut effect = DROPEFFECT_COPY;
            // SAFETY: our live target, called through its vtable as OLE does, with a data
            // object that lives until both calls return.
            unsafe {
                let pt = PointL { x: 0, y: 0 };
                ((*target).vtbl.drag_enter)(target, data.cast(), 0, pt, &mut effect);
                let pt = PointL { x: 0, y: 0 };
                ((*target).vtbl.drop)(target, data.cast(), 0, pt, &mut effect);
                std::mem::drop(Box::from_raw(data));
            }
            tracing::info!("test drop: {text} (effect {effect})");
            effect == DROPEFFECT_COPY
        }
    }

    /// The data object of [`Registration::test_drop`]: its text as a link (or plain text), in
    /// memory the receiver frees (ReleaseStgMedium).
    #[repr(C)]
    struct TestData {
        vtbl: &'static TestDataVtbl,
        text: Vec<u16>,
    }

    /// IDataObject's vtable up to GetData, all the receiver calls.
    #[repr(C)]
    struct TestDataVtbl {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        get_data:
            unsafe extern "system" fn(*mut TestData, *const FormatEtc, *mut StgMedium) -> HResult,
    }

    static TEST_DATA_VTBL: TestDataVtbl = TestDataVtbl {
        query_interface: test_data_query,
        add_ref: test_data_ref,
        release: test_data_ref,
        get_data: test_data_get,
    };

    unsafe extern "system" fn test_data_query(
        _this: *mut c_void,
        _iid: *const Guid,
        out: *mut *mut c_void,
    ) -> HResult {
        // SAFETY: `out` is the caller's.
        unsafe { *out = std::ptr::null_mut() };
        E_NOINTERFACE
    }

    /// Owned by test_drop for the calls: no counting.
    unsafe extern "system" fn test_data_ref(_this: *mut c_void) -> u32 {
        1
    }

    unsafe extern "system" fn test_data_get(
        this: *mut TestData,
        fmt: *const FormatEtc,
        out: *mut StgMedium,
    ) -> HResult {
        const DV_E_FORMATETC: HResult = 0x8004_0064_u32 as i32;
        const GMEM_MOVEABLE: u32 = 2;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        }
        // SAFETY: valid pointers for the call; the memory is filled within its size and handed
        // to the caller, who releases it.
        unsafe {
            let name: Vec<u16> = "UniformResourceLocatorW\0".encode_utf16().collect();
            let url_format = RegisterClipboardFormatW(name.as_ptr()) as u16;
            if (*fmt).cf != url_format && (*fmt).cf != CF_UNICODETEXT {
                return DV_E_FORMATETC;
            }
            let text = &(*this).text;
            let h = GlobalAlloc(GMEM_MOVEABLE, text.len() * 2);
            let p = GlobalLock(h) as *mut u16;
            if p.is_null() {
                return DV_E_FORMATETC;
            }
            std::ptr::copy_nonoverlapping(text.as_ptr(), p, text.len());
            GlobalUnlock(h);
            *out = StgMedium {
                tymed: TYMED_HGLOBAL,
                handle: h,
                unk_for_release: std::ptr::null_mut(),
            };
        }
        S_OK
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            // SAFETY: revoke OLE's reference, then ours.
            unsafe {
                RevokeDragDrop(self.hwnd);
                release(self.target);
            }
        }
    }
}

/// A named mutex held while the app runs, so the installer can tell (Inno Setup's `AppMutex`)
/// and wait for `/exit` to finish before replacing or removing the program. The handle closes
/// with the process.
pub fn mark_running() {
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = "MisterDownloadManager.Running\0".encode_utf16().collect();
    // SAFETY: NUL-terminated name; the handle is deliberately kept for the process's life.
    unsafe {
        CreateMutexW(std::ptr::null(), 0, name.as_ptr());
    }
}

// ---- Crash reports (crash.rs, mdm-crash) -------------------------------------------------------

static CRASH_HANDLER: std::sync::OnceLock<mdm_crash::crash_handler::CrashHandler> =
    std::sync::OnceLock::new();

/// Catch native crashes (access violations, stack overflows, … : Windows' structured exceptions)
/// and have `reporter`'s monitor dump this process when one comes. False when it can't attach.
pub fn attach_crash_handler(reporter: std::sync::Arc<mdm_crash::client::Reporter>) -> bool {
    use mdm_crash::crash_handler::{CrashEventResult, CrashHandler, make_crash_event};
    // SAFETY: the closure runs in the exception's context; it only sends the dump request over
    // the already-open socket (no allocation needed by minidumper's request).
    let event = unsafe { make_crash_event(move |cc| CrashEventResult::Handled(reporter.dump(cc))) };
    match CrashHandler::attach(event) {
        Ok(h) => CRASH_HANDLER.set(h).is_ok(),
        Err(e) => {
            tracing::warn!("crash handler not attached: {e}");
            false
        }
    }
}

/// Have the monitor dump this process now (a panic, a freeze): every thread, this one asking.
/// The process goes on.
pub fn dump_now() -> bool {
    CRASH_HANDLER.get().is_some_and(|h| {
        matches!(
            h.simulate_exception(None),
            mdm_crash::crash_handler::CrashEventResult::Handled(true)
        )
    })
}

/// This thread's id as Windows (and so the dump) knows it.
pub fn thread_id() -> u32 {
    // SAFETY: a plain query.
    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}

/// Give this process's working set back to the system while the window is hidden (#72): the
/// pages go to the standby list, still in RAM, and come back on first touch (soft faults, no
/// disk). What Task Manager's "Memory" column shows for a tray app is this working set.
pub fn trim_working_set() {
    use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: a plain call on this process's own pseudo-handle.
    let ok = unsafe { EmptyWorkingSet(GetCurrentProcess()) } != 0;
    if !ok {
        tracing::debug!("EmptyWorkingSet failed");
    }
}

/// Move `files` to the Recycle Bin, silently (no confirmation, no progress, no error box): an
/// archive and its volumes after extracting (#37), restorable from there. False when the shell
/// refused or stopped part way; the files left are kept.
pub fn to_recycle_bin(files: &[std::path::PathBuf]) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    if files.is_empty() {
        return true;
    }
    // Whole paths, each ended by a NUL, the list by another (the shell's own format).
    let mut from: Vec<u16> = Vec::new();
    for f in files {
        let whole = std::path::absolute(f).unwrap_or_else(|_| f.clone());
        from.extend(whole.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
        ..Default::default()
    };
    // SAFETY: `op` points at `from`, a double-NUL-ended list alive for the call; no window.
    let r = unsafe { SHFileOperationW(&mut op) };
    r == 0 && op.fAnyOperationsAborted == 0
}

/// Thread `tid`'s processor time so far, and this process's page faults so far (soft ones
/// included: Windows counts no hard faults per process here): crash.rs's freeze notes.
pub fn thread_usage(tid: u32) -> Option<crate::crash::Usage> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
    };
    // 100 ns units to ms.
    let ms = |t: FILETIME| (((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64) / 10_000;
    // SAFETY: the handle is checked and closed; the out-parameters are locals of the right size.
    unsafe {
        let h = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, tid);
        if h.is_null() {
            return None;
        }
        let z = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (z, z, z, z);
        let ok = GetThreadTimes(h, &mut created, &mut exited, &mut kernel, &mut user) != 0;
        CloseHandle(h);
        if !ok {
            return None;
        }
        let mut m: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        m.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let faults = (GetProcessMemoryInfo(GetCurrentProcess(), &mut m, m.cb) != 0)
            .then_some(m.PageFaultCount as u64);
        Some(crate::crash::Usage {
            cpu_ms: ms(kernel) + ms(user),
            faults,
        })
    }
}

/// Is process `pid` still running?
pub fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: the handle is checked and closed.
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return false;
        }
        let running = WaitForSingleObject(h, 0) == WAIT_TIMEOUT;
        CloseHandle(h);
        running
    }
}

/// The command line in this windowed program (main.rs, `misterdownloadmanager-cli`'s
/// launcher started it): the console it was started from, for what it prints and reads and for
/// Ctrl+C. Input and output the launcher handed over stay as they are (a file, a pipe, the console
/// itself); only those it didn't have get the console's. With the launcher's process id, this
/// process ends when the launcher does first (killed): nothing else would wait for it.
pub fn cli_console(launcher: Option<u32>) {
    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };
    use windows_sys::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    let usable = |h: HANDLE| !h.is_null() && h != INVALID_HANDLE_VALUE;
    let ids = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
    // SAFETY: plain queries and settings of this process's standard handles; the names are
    // NUL-terminated, and the handles opened are kept for the process's life.
    unsafe {
        let given = ids.map(|id| GetStdHandle(id));
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            for (id, h) in ids.into_iter().zip(given) {
                if usable(h) {
                    SetStdHandle(id, h);
                } else if !usable(GetStdHandle(id)) {
                    let name = if id == STD_INPUT_HANDLE {
                        "CONIN$\0"
                    } else {
                        "CONOUT$\0"
                    };
                    let name: Vec<u16> = name.encode_utf16().collect();
                    let console = CreateFileW(
                        name.as_ptr(),
                        GENERIC_READ | GENERIC_WRITE,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        std::ptr::null(),
                        OPEN_EXISTING,
                        0,
                        std::ptr::null_mut(),
                    );
                    if usable(console) {
                        SetStdHandle(id, console);
                    }
                }
            }
        }
        let watched = launcher.map_or(std::ptr::null_mut(), |pid| {
            OpenProcess(PROCESS_SYNCHRONIZE, 0, pid)
        });
        if !watched.is_null() {
            // A handle isn't Send; its value is, and the handle stays open.
            let watched = watched as usize;
            std::thread::spawn(move || {
                WaitForSingleObject(watched as HANDLE, INFINITE);
                std::process::exit(1);
            });
        }
    }
}

/// The OpenGL driver in its own words, for the log and the crash reports (#280): its renderer
/// (the graphics card) and version, which on Windows carries the driver's build (Intel's
/// "4.6.0 - Build 31.0.101.…", NVIDIA's "4.6.0 NVIDIA 5xx.xx"). Linux and macOS: unix.rs.
pub fn gl_driver(gl: &eframe::glow::Context) -> String {
    use eframe::glow::{self, HasContext as _};
    // SAFETY: called while eframe makes the app, with its context current on this thread;
    // glGetString only reads the driver's strings.
    let (renderer, version) = unsafe {
        (
            gl.get_parameter_string(glow::RENDERER),
            gl.get_parameter_string(glow::VERSION),
        )
    };
    format!("{renderer} ({version})")
}

/// Windows' clipboard sequence number: it changes each time the clipboard does, and asking for
/// it neither opens the clipboard nor waits for its owner (clipboard.rs: the watch for copied
/// links reads the clipboard only when it changed).
pub fn clipboard_sequence() -> u32 {
    // SAFETY: takes nothing, returns a number.
    unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() }
}

/// End process `pid` (a frozen MDM the user chose to close from the crash report).
pub fn end_process(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: the handle is checked and closed.
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h.is_null() {
            return false;
        }
        let ok = TerminateProcess(h, 1) != 0;
        CloseHandle(h);
        ok
    }
}

/// The crash test's access violation (MDM_CRASH_TEST=1, `/show crash-test:crash`).
pub fn crash_now() {
    // SAFETY: none: a write through a null pointer, on purpose, to test the crash reports.
    unsafe { std::ptr::write_volatile(std::ptr::null_mut::<u32>(), 1) };
}

/// The names of the subkeys of `key` under `root`.
fn reg_subkeys(root: windows_sys::Win32::System::Registry::HKEY, key: &str) -> Vec<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
    };
    let wide: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let mut h: HKEY = std::ptr::null_mut();
    // SAFETY: opens a key for reading into `h`, closed below.
    if unsafe { RegOpenKeyExW(root, wide.as_ptr(), 0, KEY_READ, &mut h) } != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0.. {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        // SAFETY: the subkey's name into `buf`, whose length in characters is in `len`.
        let rc = unsafe {
            RegEnumKeyExW(
                h,
                i,
                buf.as_mut_ptr(),
                &mut len,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            break;
        }
        out.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    // SAFETY: `h` was opened above.
    unsafe { RegCloseKey(h) };
    out
}

/// The browsers installed, by name, with their program: those Windows lists for "default
/// browser" (StartMenuInternet, this user's then everyone's), named by their program
/// (mdm_ipc::browsers::windows_named: Helium's chrome.exe isn't Chrome) or else by their own
/// display name; then the known programs registered only under the shell's App Paths (a zip or
/// portable install). The browser extension's offer (extension.rs) opens its page in each.
pub fn browsers() -> Vec<(String, std::path::PathBuf)> {
    use mdm_ipc::browsers::{command_program, windows_can_run_extension, windows_named};
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    const CLIENTS: &str = r"Software\Clients\StartMenuInternet";
    const APP_PATHS: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths";
    let mut out: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut add = |name: String, path: std::path::PathBuf| {
        let same = |p: &std::path::PathBuf| {
            p.to_string_lossy()
                .eq_ignore_ascii_case(&path.to_string_lossy())
        };
        if path.is_file() && !out.iter().any(|(n, p)| same(p) || *n == name) {
            out.push((name, path));
        }
    };
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for sub in reg_subkeys(root, CLIENTS) {
            let key = format!(r"{CLIENTS}\{sub}");
            let Some(program) = reg_string_in(root, &format!(r"{key}\shell\open\command"), "")
                .as_deref()
                .and_then(command_program)
            else {
                continue;
            };
            if !windows_can_run_extension(&program) {
                continue;
            }
            let name = windows_named(&program)
                .map(str::to_owned)
                .or_else(|| reg_string_in(root, &key, "").filter(|n| !n.trim().is_empty()));
            add(name.unwrap_or(sub), std::path::PathBuf::from(program));
        }
    }
    for (exe, _, _) in mdm_ipc::browsers::WINDOWS_PROGRAMS {
        let key = format!(r"{APP_PATHS}\{exe}");
        let Some(path) = reg_string_in(HKEY_CURRENT_USER, &key, "")
            .or_else(|| reg_string_in(HKEY_LOCAL_MACHINE, &key, ""))
        else {
            continue;
        };
        let path = path.trim().trim_matches('"').to_owned();
        if let Some(name) = windows_named(&path) {
            add(name.to_owned(), std::path::PathBuf::from(path));
        }
    }
    out
}

/// Open `url` in the browser whose program is `program` (from `browsers`).
pub fn open_in(program: &std::path::Path, url: &str) {
    let _ = std::process::Command::new(program).arg(url).spawn();
}

/// Windows' Group Policy "Do not preserve zone information in file attachments" (#813), as
/// (the computer's, the user's) `SaveZoneInformation` values: 1 keeps no zone information, 2
/// keeps it; None where it isn't set. Browsers save their downloads through Windows'
/// attachment services, which follow it; `webmark::keeps_zones` decides the same way. Two
/// registry reads: for a thread other than the window's (the download's, as it starts).
pub fn zone_policy() -> (Option<u32>, Option<u32>) {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Attachments";
    const NAME: &str = "SaveZoneInformation";
    (
        reg_dword_in(HKEY_LOCAL_MACHINE, KEY, NAME),
        reg_dword_in(HKEY_CURRENT_USER, KEY, NAME),
    )
}

#[cfg(test)]
mod zone_policy_tests {
    use super::*;
    use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;

    /// The policy's values are read as Windows keeps them (a DWORD under either root), and one
    /// that isn't set reads as none. The policy key itself is the administrators' to write, even
    /// the user's: a DWORD every Windows 10 and 11 has stands in for it.
    #[test]
    fn a_dword_is_read_and_a_missing_value_is_none() {
        let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        assert_eq!(
            reg_dword_in(HKEY_LOCAL_MACHINE, key, "CurrentMajorVersionNumber"),
            Some(10)
        );
        assert_eq!(
            reg_dword_in(HKEY_LOCAL_MACHINE, key, "NoSuchValueOfMdm"),
            None
        );
        // A string isn't taken for a number.
        assert_eq!(reg_dword_in(HKEY_LOCAL_MACHINE, key, "ProductName"), None);
        // Whatever this PC's policy is, it is read without failing.
        let _ = zone_policy();
    }
}

/// Mark-of-the-Web on a file being downloaded from `url` (`referrer`'s page): the zone stream
/// browsers write, so SmartScreen checks a program, Office opens a document in Protected View and
/// archive tools pass the mark on, as for the browser's own download. Written on the part file
/// as it is created (the engine renames it, the stream going along). Through `webmark::mark`,
/// which asks the option and Windows' policy first.
pub fn mark_downloaded(path: &std::path::Path, url: &str, referrer: Option<&str>) {
    // Only addresses from the web: never a login's password (the engine has none in `url`).
    let clean = |u: &str| u.split(['\r', '\n']).next().unwrap_or("").to_owned();
    let mut text = String::from("[ZoneTransfer]\r\nZoneId=3\r\n");
    if let Some(r) = referrer.filter(|r| r.starts_with("http")) {
        text.push_str(&format!("ReferrerUrl={}\r\n", clean(r)));
    }
    text.push_str(&format!("HostUrl={}\r\n", clean(url)));
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    let _ = std::fs::write(stream, text);
}

/// Rename `from` to `to` without ever replacing an existing entry (`MoveFileExW` without
/// `MOVEFILE_REPLACE_EXISTING`, returning `ErrorKind::AlreadyExists` when `to` is already there).
pub fn rename_no_replace(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let encode = |p: &std::path::Path| -> std::io::Result<Vec<u16>> {
        let mut wide: Vec<u16> = p.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "interior NUL",
            ));
        }
        wide.push(0);
        Ok(wide)
    };
    let from_w = encode(from)?;
    let to_w = encode(to)?;
    // SAFETY: both UTF-16 buffers are NUL-terminated and live through the call; flags = 0 refuses
    // cross-volume copies and refuses replacing an existing destination.
    let ok = unsafe { MoveFileExW(from_w.as_ptr(), to_w.as_ptr(), 0) };
    if ok != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Where browser integration stands: a browser looking for the native host (Chrome's key,
/// which Chromium browsers read; Firefox's) finds a manifest that exists, for this user or for
/// everyone, naming a helper program that is there (Options → General → Test browser
/// integration).
pub fn host_registration() -> crate::extension::Registration {
    use crate::extension::Registration;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let mut broken = None;
    for key in [
        r"Software\Google\Chrome\NativeMessagingHosts\com.misterdownloadmanager.host",
        r"Software\Mozilla\NativeMessagingHosts\com.misterdownloadmanager.host",
    ] {
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let Some(manifest) = reg_string_in(root, key, "") else {
                continue;
            };
            let manifest = std::path::PathBuf::from(manifest.trim().trim_matches('"'));
            if !manifest.is_file() {
                continue;
            }
            match mdm_ipc::browsers::manifest_program(&manifest) {
                Some(p) if p.is_file() => return Registration::Ready,
                Some(p) => broken = broken.or(Some(p)),
                None => {}
            }
        }
    }
    broken.map_or(Registration::Missing, Registration::HelperMissing)
}

/// Options → Sites Logins' passwords, encrypted for this Windows account (DPAPI) before the list
/// is written: another account, or a copy of the file, can't read them.
#[allow(unsafe_code)]
pub fn protect(plain: &str) -> Option<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptProtectData};
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(plain.len()).ok()?,
        pbData: plain.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: `input` points at `plain` for the call (only read); `output` is filled by the
    // system with a buffer we copy and then free with LocalFree, as CryptProtectData requires.
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut output,
        )
    };
    blob_bytes(ok, output)
}

/// The password `protect` encrypted, for this account.
#[allow(unsafe_code)]
pub fn unprotect(sealed: &[u8]) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CryptUnprotectData};
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(sealed.len()).ok()?,
        pbData: sealed.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: as in `protect`.
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut output,
        )
    };
    String::from_utf8(blob_bytes(ok, output)?).ok()
}

/// A DPAPI result's bytes, its buffer freed.
#[allow(unsafe_code)]
fn blob_bytes(
    ok: i32,
    blob: windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB,
) -> Option<Vec<u8>> {
    if ok == 0 || blob.pbData.is_null() {
        return None;
    }
    // SAFETY: the system gave `cbData` bytes at `pbData`, ours until LocalFree.
    let bytes = unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();
    // SAFETY: the buffer came from DPAPI, which allocates with LocalAlloc.
    unsafe { windows_sys::Win32::Foundation::LocalFree(blob.pbData.cast()) };
    Some(bytes)
}

// Sign-in secrets in Windows Credential Manager (#44, secrets.rs): generic credentials named
// `<service>/<account>`, for this Windows user, roaming with the profile as MDM's data folder
// (%APPDATA%) does. Credential Manager (Control Panel → Credential Manager → Windows
// Credentials) shows them, the label as their user name, and can remove them.

fn cred_target(service: &str, account: &str) -> Vec<u16> {
    format!("{service}/{account}")
        .encode_utf16()
        .chain(Some(0))
        .collect()
}

/// The last error as a fault: no logon session (a service account) means no keychain.
#[allow(unsafe_code)]
fn cred_fault(what: &str) -> crate::secrets::Fault {
    use crate::secrets::Fault;
    use windows_sys::Win32::Foundation::{ERROR_NO_SUCH_LOGON_SESSION, GetLastError};
    // SAFETY: no arguments; reads this thread's last error.
    let code = unsafe { GetLastError() };
    if code == ERROR_NO_SUCH_LOGON_SESSION {
        Fault::Missing
    } else {
        Fault::Failed(format!("{what}: Windows error {code}"))
    }
}

#[allow(unsafe_code)]
fn cred_not_found() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
    // SAFETY: no arguments.
    unsafe { GetLastError() == ERROR_NOT_FOUND }
}

/// Whether Credential Manager answers (listing MDM's own entries).
pub fn keychain_probe(service: &str) -> Result<(), crate::secrets::Fault> {
    keychain_list(service).map(|_| ())
}

#[allow(unsafe_code)]
pub fn keychain_read(
    service: &str,
    account: &str,
) -> Result<Option<String>, crate::secrets::Fault> {
    use windows_sys::Win32::Security::Credentials::{
        CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW,
    };
    let target = cred_target(service, account);
    let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `target` is a NUL-terminated UTF-16 string alive for the call; `cred` receives a
    // buffer the system allocated, freed below with CredFree.
    let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) };
    if ok == 0 {
        return if cred_not_found() {
            Ok(None)
        } else {
            Err(cred_fault("CredReadW"))
        };
    }
    // SAFETY: on success `cred` points at a CREDENTIALW whose blob holds `CredentialBlobSize`
    // bytes; both are ours until CredFree.
    let bytes = unsafe {
        let c = &*cred;
        let bytes = if c.CredentialBlob.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
        };
        CredFree(cred.cast());
        bytes
    };
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| crate::secrets::Fault::Failed("not UTF-8".into()))
}

/// Stored as UTF-8. Credential Manager takes at most 2560 bytes: a longer secret stays in the
/// settings, sealed with DPAPI.
#[allow(unsafe_code)]
pub fn keychain_write(
    service: &str,
    account: &str,
    label: &str,
    secret: &str,
) -> Result<(), crate::secrets::Fault> {
    use windows_sys::Win32::Security::Credentials::{
        CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_ENTERPRISE, CRED_TYPE_GENERIC, CREDENTIALW,
        CredWriteW,
    };
    if secret.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        return Err(crate::secrets::Fault::Failed(
            "longer than Credential Manager takes".into(),
        ));
    }
    let mut target = cred_target(service, account);
    let mut user: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
    let mut blob = secret.as_bytes().to_vec();
    // SAFETY: all-zero is a valid CREDENTIALW (null pointers, zero sizes).
    let mut cred: CREDENTIALW = unsafe { std::mem::zeroed() };
    cred.Type = CRED_TYPE_GENERIC;
    cred.TargetName = target.as_mut_ptr();
    cred.UserName = user.as_mut_ptr();
    cred.CredentialBlobSize = blob.len() as u32;
    cred.CredentialBlob = blob.as_mut_ptr();
    cred.Persist = CRED_PERSIST_ENTERPRISE;
    // SAFETY: every pointer in `cred` points at a buffer alive for the call (only read).
    let ok = unsafe { CredWriteW(&cred, 0) };
    let fault = (ok == 0).then(|| cred_fault("CredWriteW"));
    // The plain copy doesn't linger in this buffer.
    blob.iter_mut().for_each(|b| *b = 0);
    fault.map_or(Ok(()), Err)
}

#[allow(unsafe_code)]
pub fn keychain_delete(service: &str, account: &str) -> Result<(), crate::secrets::Fault> {
    use windows_sys::Win32::Security::Credentials::{CRED_TYPE_GENERIC, CredDeleteW};
    let target = cred_target(service, account);
    // SAFETY: `target` is NUL-terminated and alive for the call.
    let ok = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
    if ok == 0 && !cred_not_found() {
        return Err(cred_fault("CredDeleteW"));
    }
    Ok(())
}

/// The accounts of `service`'s generic credentials (the filter `<service>/*`: only MDM's; the
/// others' blobs are never read).
#[allow(unsafe_code)]
pub fn keychain_list(service: &str) -> Result<Vec<String>, crate::secrets::Fault> {
    use windows_sys::Win32::Security::Credentials::{
        CRED_TYPE_GENERIC, CREDENTIALW, CredEnumerateW, CredFree,
    };
    let filter: Vec<u16> = format!("{service}/*")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut count = 0u32;
    let mut creds: *mut *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `filter` is NUL-terminated; `creds` receives an array of `count` pointers the
    // system allocated, freed below with CredFree.
    let ok = unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut creds) };
    if ok == 0 {
        return if cred_not_found() {
            Ok(Vec::new())
        } else {
            Err(cred_fault("CredEnumerateW"))
        };
    }
    let prefix = format!("{service}/");
    let mut accounts = Vec::new();
    // SAFETY: on success `creds` holds `count` valid CREDENTIALW pointers, ours until CredFree;
    // each TargetName is a NUL-terminated UTF-16 string.
    unsafe {
        for i in 0..count as usize {
            let c = &**creds.add(i);
            if c.Type != CRED_TYPE_GENERIC || c.TargetName.is_null() {
                continue;
            }
            let mut n = 0;
            while *c.TargetName.add(n) != 0 {
                n += 1;
            }
            let name = String::from_utf16_lossy(std::slice::from_raw_parts(c.TargetName, n));
            if let Some(account) = name.strip_prefix(&prefix) {
                accounts.push(account.to_owned());
            }
        }
        CredFree(creds.cast());
    }
    Ok(accounts)
}

// ---- waking the computer for a queue (#142) -----------------------------------------------

/// Windows' "Allow wake timers" power option for the active plan (0 off, 1 on, 2 important
/// wake timers only), on mains power (`ac`) and on battery (`dc`), and the power source now.
pub struct WakeTimers {
    pub ac: u32,
    pub dc: u32,
    pub on_battery: bool,
    pub has_battery: bool,
}

/// Power Options → Sleep → Allow wake timers, as `powercfg /q SCHEME_CURRENT SUB_SLEEP
/// RTCWAKE` shows it. Read only; a setting that can't be read counts as on (nothing to warn of).
pub fn wake_timers() -> WakeTimers {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::System::Power::{
        GetSystemPowerStatus, PowerGetActiveScheme, PowerReadACValueIndex, PowerReadDCValueIndex,
        SYSTEM_POWER_STATUS,
    };
    use windows_sys::core::GUID;
    const SUB_SLEEP: GUID = GUID::from_u128(0x238c9fa8_0aad_41ed_83f4_97be242c8f20);
    const RTCWAKE: GUID = GUID::from_u128(0xbd3b718a_0680_4d9d_8ab2_e1d2b4ac806d);
    let (mut ac, mut dc) = (1u32, 1u32);
    let mut scheme: *mut GUID = std::ptr::null_mut();
    // SAFETY: PowerGetActiveScheme allocates the GUID it returns (freed with LocalFree below);
    // the reads only write the u32 given.
    unsafe {
        if PowerGetActiveScheme(std::ptr::null_mut(), &mut scheme) == 0 && !scheme.is_null() {
            let mut v = 0u32;
            if PowerReadACValueIndex(std::ptr::null_mut(), scheme, &SUB_SLEEP, &RTCWAKE, &mut v)
                == 0
            {
                ac = v;
            }
            if PowerReadDCValueIndex(std::ptr::null_mut(), scheme, &SUB_SLEEP, &RTCWAKE, &mut v)
                == 0
            {
                dc = v;
            }
            LocalFree(scheme.cast());
        }
    }
    // SAFETY: plain data, zero is valid; a plain query into it.
    let mut st: SYSTEM_POWER_STATUS = unsafe { zeroed() };
    let ok = unsafe { GetSystemPowerStatus(&mut st) } != 0;
    WakeTimers {
        ac,
        dc,
        // ACLineStatus 0: on battery; BatteryFlag 128: no system battery, 255: unknown.
        on_battery: ok && st.ACLineStatus == 0,
        has_battery: ok && st.BatteryFlag != 128 && st.BatteryFlag != 255,
    }
}

/// Power Options' advanced settings, where "Allow wake timers" is (under Sleep). Never from an
/// off-screen test run: its windows belong to the person at the computer.
pub fn open_power_options() {
    if crate::offscreen() {
        return;
    }
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("control.exe")
        .arg("powercfg.cpl,,3")
        .creation_flags(0x0800_0000)
        .spawn();
}

/// A change to the wake timer, and where its result goes.
struct WakeCmd {
    /// When (seconds since 1970, UTC) and for which queue; None cancels.
    at: Option<(i64, String)>,
    reply: std::sync::mpsc::Sender<Result<(), String>>,
}

/// Wake the computer at `at` (seconds since 1970, UTC; for the queue named), or cancel the wake
/// (None). A waitable timer set to resume the computer (SetWaitableTimer, fResume): it lives in
/// MDM's process (Windows cancels it if the thread that set it ends, so one thread of its own
/// keeps it), lists in `powercfg /waketimers`, and goes with MDM: nothing is left behind when
/// MDM quits or is uninstalled.
pub fn wake_at(at: Option<(i64, &str)>) -> Result<(), String> {
    use std::sync::{Mutex, OnceLock, mpsc};
    static THREAD: OnceLock<Mutex<mpsc::Sender<WakeCmd>>> = OnceLock::new();
    let tx = THREAD.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new()
            .name("mdm-wake".into())
            .spawn(move || wake_thread(rx));
        Mutex::new(tx)
    });
    let (reply, answer) = mpsc::channel();
    let cmd = WakeCmd {
        at: at.map(|(t, q)| (t, q.to_owned())),
        reply,
    };
    let sent = tx.lock().map(|tx| tx.send(cmd).is_ok()).unwrap_or(false);
    let lost = || "the wake timer's thread is gone".to_owned();
    if !sent {
        return Err(lost());
    }
    answer
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap_or_else(|_| Err(lost()))
}

fn wake_thread(rx: std::sync::mpsc::Receiver<WakeCmd>) {
    use std::sync::mpsc::RecvTimeoutError;
    use windows_sys::Win32::Foundation::{ERROR_NOT_SUPPORTED, GetLastError, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        CancelWaitableTimer, CreateWaitableTimerExW, SetWaitableTimer, TIMER_ALL_ACCESS,
        WaitForSingleObject,
    };
    // A synchronization timer (auto-reset): the check below sees each firing once.
    // SAFETY: no name, default security; the handle is this thread's for the process's life.
    let timer =
        unsafe { CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), 0, TIMER_ALL_ACCESS) };
    let mut queue = String::new();
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(cmd) => {
                let result = if timer.is_null() {
                    // SAFETY: a plain query.
                    Err(format!("CreateWaitableTimerExW failed ({})", unsafe {
                        GetLastError()
                    }))
                } else if let Some((unix, q)) = cmd.at {
                    // FILETIME: 100 ns steps since 1601, UTC (positive: an absolute time).
                    let due: i64 = (unix + 11_644_473_600) * 10_000_000;
                    // SAFETY: our timer; no completion routine; fResume = TRUE. GetLastError
                    // right after, as the call's documentation asks.
                    let (ok, err) = unsafe {
                        let ok = SetWaitableTimer(timer, &due, 0, None, std::ptr::null(), 1) != 0;
                        (ok, GetLastError())
                    };
                    queue = q;
                    if !ok {
                        Err(format!("SetWaitableTimer failed ({err})"))
                    } else if err == ERROR_NOT_SUPPORTED {
                        // Set, but this computer can't be woken by it.
                        Err(crate::i18n::t(
                            // i18n: the Scheduler, under "Wake the computer to run this queue": Windows set the timer but can't wake this computer with it
                            "This computer can't be woken by a timer",
                        )
                        .to_owned())
                    } else {
                        Ok(())
                    }
                } else {
                    // SAFETY: our timer.
                    unsafe { CancelWaitableTimer(timer) };
                    Ok(())
                };
                let _ = cmd.reply.send(result);
            }
            Err(RecvTimeoutError::Timeout) => {
                // SAFETY: our timer; a zero wait only looks.
                if !timer.is_null() && unsafe { WaitForSingleObject(timer, 0) } == WAIT_OBJECT_0 {
                    tracing::info!("wake: the wake timer went off (queue {queue:?})");
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Keep the computer from sleeping while `reason` is given (a power request, listed with its
/// reason in `powercfg /requests`); None lets it sleep again as its power plan says. False when
/// Windows refused.
pub fn keep_awake(reason: Option<&str>) -> bool {
    use std::sync::Mutex;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Power::{
        PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest,
    };
    use windows_sys::Win32::System::Threading::{
        POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT,
    };
    static REQUEST: Mutex<Option<isize>> = Mutex::new(None);
    let mut held = REQUEST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = held.take() {
        // SAFETY: the request made below, cleared and closed once.
        unsafe {
            PowerClearRequest(h as _, PowerRequestSystemRequired);
            CloseHandle(h as _);
        }
    }
    let Some(reason) = reason else {
        return true;
    };
    let mut text: Vec<u16> = reason.encode_utf16().chain(Some(0)).collect();
    // SAFETY: plain data, zero is valid.
    let mut ctx: REASON_CONTEXT = unsafe { zeroed() };
    // POWER_REQUEST_CONTEXT_VERSION is 0.
    ctx.Version = 0;
    ctx.Flags = POWER_REQUEST_CONTEXT_SIMPLE_STRING;
    ctx.Reason.SimpleReasonString = text.as_mut_ptr();
    // SAFETY: `ctx` and the string it points to live through the call (Windows copies it).
    let h = unsafe { PowerCreateRequest(&ctx) };
    if h.is_null() || h as isize == -1 {
        tracing::warn!("wake: PowerCreateRequest failed");
        return false;
    }
    // SAFETY: the request just made; closed if it can't be set.
    if unsafe { PowerSetRequest(h, PowerRequestSystemRequired) } == 0 {
        tracing::warn!("wake: PowerSetRequest failed");
        unsafe { CloseHandle(h) };
        return false;
    }
    *held = Some(h as isize);
    true
}

/// Latin, Greek and Cyrillic text as Windows' classic controls draw it, in grey (#262, #323,
/// #334): each letter's shape and width come from the font's own hinting for this size, through
/// DirectWrite's GDI-classic mode. Before, egui placed letters by their unhinted widths rounded
/// to whole pixels while the letters themselves were fitted to pixels: the two disagreed by up to
/// a pixel, so letters touched in one place ("download") and stood apart in another ("Resum e").
///
/// `smooth` picks how a letter is drawn: `false`, the classic controls' look (hinted in both
/// directions at a whole pixel size, letters on whole pixels: crisp, and coarse on a large,
/// dense screen), or `true`, DirectWrite's natural look (the size as it is, hinted up and down
/// only, each letter where it falls between two pixels, with all 256 levels of grey: the text of
/// Windows' own modern apps, much easier to read at 1440p).
///
/// `None` from DirectWrite's factory or a font's file keeps egui's rasterizer. A cluster it isn't
/// for ([`classic_char`]) is left to egui too, which shapes it. The rasterizer is told the
/// interface's direction: a bracket in a right-to-left line is mirrored by the shaper, so those
/// stay with egui there.
pub fn text_rasterizer(
    font_dir: std::path::PathBuf,
    rtl: bool,
    smooth: bool,
) -> Option<egui::epaint::text::GlyphRasterizer> {
    let glyphs = classic_text::Glyphs::new(font_dir)
        .map_err(|e| tracing::warn!("text: DirectWrite unavailable, egui draws the text ({e})"))
        .ok()?;
    let glyphs = std::sync::Mutex::new(glyphs);
    Some(
        egui::epaint::text::GlyphRasterizer::new(
            "mdm::win32::classic_text",
            move |req: &egui::epaint::text::GlyphRasterizerRequest<'_>| {
                glyphs.lock().ok()?.glyph(req, rtl, smooth)
            },
        )
        .with_priority(egui::epaint::text::FontPriority::Highest),
    )
}

/// Does [`text_rasterizer`] draw this character? One code point that needs no shaping: Latin,
/// Greek and Cyrillic letters, digits and punctuation, and the CJK characters whose face comes
/// from the interface language's script font. Scripts whose letters change shape (Arabic, Indic,
/// ...) and the invisible marks are left to egui and its shaper; so are the brackets of a
/// right-to-left line, which the shaper mirrors.
fn classic_char(c: char, rtl: bool) -> bool {
    if rtl
        && matches!(
            c,
            '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '«' | '»' | '‹' | '›'
        )
    {
        return false;
    }
    matches!(c,
        ' '..='~'
        | '\u{A0}'..='\u{AC}' // Latin-1, without the soft hyphen
        | '\u{AE}'..='\u{2AF}' // Latin Extended and IPA
        | '\u{370}'..='\u{3FF}' // Greek
        | '\u{400}'..='\u{52F}' // Cyrillic
        | '\u{1E00}'..='\u{1EFF}' // Vietnamese and other precomposed Latin
        | '\u{2010}'..='\u{2027}' // dashes, quotes, bullets, ellipsis
        | '\u{2030}'..='\u{205E}' // per mille, primes, guillemets
        | '\u{20A0}'..='\u{20BF}' // currency
    ) || crate::theme::cjk_script(c).is_some()
}

/// The font file of a family `theme.rs` sets up (Segoe UI leads the two built-in ones).
fn classic_font_file(family: &egui::FontFamily) -> Option<&'static str> {
    match family {
        egui::FontFamily::Proportional | egui::FontFamily::Monospace => Some("segoeui.ttf"),
        egui::FontFamily::Name(name) => match &**name {
            "tahoma" => Some("tahoma.ttf"),
            "tahoma-bold" => Some("tahomabd.ttf"),
            "arial-bold" => Some("arialbd.ttf"),
            _ => None,
        },
    }
}

mod classic_text {
    use std::collections::HashMap;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::PathBuf;

    use egui::epaint::text::{GlyphBitmap, GlyphRasterizerRequest, RasterizedGlyph};
    use egui::epaint::{Color32, ColorImage};
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_FACE_TYPE_TRUETYPE,
        DWRITE_FONT_FACE_TYPE_TRUETYPE_COLLECTION, DWRITE_FONT_METRICS,
        DWRITE_FONT_SIMULATIONS_NONE, DWRITE_GLYPH_METRICS, DWRITE_GLYPH_OFFSET, DWRITE_GLYPH_RUN,
        DWRITE_GRID_FIT_MODE_ENABLED, DWRITE_MEASURING_MODE_GDI_CLASSIC,
        DWRITE_MEASURING_MODE_NATURAL, DWRITE_RENDERING_MODE_GDI_CLASSIC,
        DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC, DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
        DWRITE_TEXTURE_ALIASED_1x1, DWriteCreateFactory, IDWriteFactory2, IDWriteFontFace,
    };

    /// How much the smooth look thins its edges: 1.0 keeps DirectWrite's coverage as it is.
    const SMOOTH_COVERAGE_GAMMA: f32 = 1.8;

    pub struct Glyphs {
        factory: IDWriteFactory2,
        dir: PathBuf,
        /// The font files opened so far; `None` for one DirectWrite can't use.
        faces: HashMap<&'static str, Option<IDWriteFontFace>>,
    }

    // SAFETY: DirectWrite's shared factory and the objects it makes are free-threaded
    // (documented thread-safe); `Glyphs` is only used behind a Mutex.
    unsafe impl Send for Glyphs {}

    impl Glyphs {
        pub fn new(dir: PathBuf) -> windows::core::Result<Glyphs> {
            // SAFETY: a plain COM call.
            let factory: IDWriteFactory2 =
                unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
            Ok(Glyphs {
                factory,
                dir,
                faces: HashMap::new(),
            })
        }

        fn face(&mut self, file: &'static str) -> Option<IDWriteFontFace> {
            let (factory, dir) = (&self.factory, &self.dir);
            self.faces
                .entry(file)
                .or_insert_with(|| {
                    let path: Vec<u16> = dir
                        .join(file)
                        .as_os_str()
                        .encode_wide()
                        .chain([0])
                        .collect();
                    // SAFETY: `path` is NUL-terminated and outlives the calls; plain COM calls.
                    unsafe {
                        let font = factory
                            .CreateFontFileReference(windows::core::PCWSTR(path.as_ptr()), None)
                            .ok()?;
                        factory
                            .CreateFontFace(
                                // A .ttc holds a collection: face 0 is its regular face.
                                if file.to_ascii_lowercase().ends_with(".ttc") {
                                    DWRITE_FONT_FACE_TYPE_TRUETYPE_COLLECTION
                                } else {
                                    DWRITE_FONT_FACE_TYPE_TRUETYPE
                                },
                                &[Some(font)],
                                0,
                                DWRITE_FONT_SIMULATIONS_NONE,
                            )
                            .ok()
                    }
                })
                .clone()
        }

        /// The face and glyph id for `ch`: the family's own face for the scripts it draws, or the
        /// interface language's CJK face for Chinese, Japanese and Korean text. None when no face
        /// has a glyph for it: egui and its shaper draw it instead.
        fn lookup(
            &mut self,
            family: &egui::FontFamily,
            ch: char,
        ) -> Option<(IDWriteFontFace, u16, DWRITE_FONT_METRICS)> {
            let files: Vec<&'static str> = if crate::theme::cjk_script(ch).is_some() {
                crate::theme::cjk_font_files(ch)
            } else {
                super::classic_font_file(family).into_iter().collect()
            };
            for file in files {
                let Some(face) = self.face(file) else {
                    continue;
                };
                let mut id = 0u16;
                // SAFETY: the pointer is to one element, the count given.
                if unsafe { face.GetGlyphIndices(&(ch as u32), 1, &mut id) }.is_err() {
                    continue;
                }
                if id == 0 {
                    continue;
                }
                let mut font = DWRITE_FONT_METRICS::default();
                // SAFETY: the pointer is to one element.
                unsafe {
                    face.GetMetrics(&mut font);
                }
                return Some((face, id, font));
            }
            None
        }

        pub fn glyph(
            &mut self,
            req: &GlyphRasterizerRequest<'_>,
            rtl: bool,
            smooth: bool,
        ) -> Option<RasterizedGlyph> {
            let mut chars = req.cluster.chars();
            let ch = chars.next()?;
            if chars.next().is_some() || !super::classic_char(ch, rtl) {
                return None;
            }
            if req.font_size_px.is_nan() || req.font_size_px < 1.0 {
                return None;
            }
            let (face, id, font) = self.lookup(req.family, ch)?;
            // GDI's sizes are whole pixels; the natural look keeps the size it is asked for.
            let em = if smooth {
                req.font_size_px
            } else {
                req.font_size_px.round()
            };
            let mut metrics = DWRITE_GLYPH_METRICS::default();
            // SAFETY: each pointer is to one element, the count given.
            unsafe {
                if smooth {
                    face.GetDesignGlyphMetrics(&id, 1, &mut metrics, false)
                        .ok()?;
                } else {
                    face.GetGdiCompatibleGlyphMetrics(
                        em,
                        1.0,
                        None,
                        false,
                        &id,
                        1,
                        &mut metrics,
                        false,
                    )
                    .ok()?;
                }
            }
            let advance = metrics.advanceWidth as f32 * em / f32::from(font.designUnitsPerEm);
            // Smooth: the font's own width, as egui lays a line out by it (and places each letter
            // at a quarter pixel). Classic: the whole pixels GDI's hinting gives.
            let advance_px = if smooth { advance } else { advance.round() };
            let origin = if smooth { req.subpixel_offset_px } else { 0.0 };
            let bitmap = self
                .draw(face, id, em, smooth, origin)
                .unwrap_or(GlyphBitmap {
                    // A space: egui still wants a bitmap to carry the advance.
                    image: ColorImage::new([1, 1], vec![Color32::TRANSPARENT]),
                    offset_px: egui::vec2(0.0, 0.0),
                    is_color: false,
                });
            Some(RasterizedGlyph { bitmap, advance_px })
        }

        /// The glyph in grey, placed from its origin; `None` when it has no ink.
        fn draw(
            &self,
            face: IDWriteFontFace,
            id: u16,
            em: f32,
            smooth: bool,
            origin_x: f32,
        ) -> Option<GlyphBitmap> {
            let (ids, advances, offsets) = ([id], [0f32], [DWRITE_GLYPH_OFFSET::default()]);
            let run = DWRITE_GLYPH_RUN {
                fontFace: std::mem::ManuallyDrop::new(Some(face)),
                fontEmSize: em,
                glyphCount: 1,
                glyphIndices: ids.as_ptr(),
                glyphAdvances: advances.as_ptr(),
                glyphOffsets: offsets.as_ptr(),
                isSideways: false.into(),
                bidiLevel: 0,
            };
            // SAFETY: `run` and the arrays it points to live through the calls; the texture
            // buffer is exactly the bounds' size.
            let drawn = unsafe {
                self.factory
                    .CreateGlyphRunAnalysis(
                        &run,
                        None,
                        if smooth {
                            DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC
                        } else {
                            DWRITE_RENDERING_MODE_GDI_CLASSIC
                        },
                        if smooth {
                            DWRITE_MEASURING_MODE_NATURAL
                        } else {
                            DWRITE_MEASURING_MODE_GDI_CLASSIC
                        },
                        DWRITE_GRID_FIT_MODE_ENABLED,
                        DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                        origin_x,
                        0.0,
                    )
                    .and_then(|a| {
                        let r = a.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)?;
                        let w = (r.right - r.left).max(0) as usize;
                        let h = (r.bottom - r.top).max(0) as usize;
                        let mut coverage = vec![0u8; w * h];
                        if w > 0 && h > 0 {
                            a.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &r, &mut coverage)?;
                        }
                        Ok((r, w, h, coverage))
                    })
            };
            drop(std::mem::ManuallyDrop::into_inner(run.fontFace));
            let (r, w, h, coverage) = drawn.ok()?;
            if w == 0 || h == 0 {
                return None;
            }
            // White with alpha, premultiplied (`GlyphBitmap`).
            // Smooth: less ink at the edges, so the weight matches what Windows' own
            // DirectWrite text shows (WPF's grey-scale text, compared side by side): coverage
            // blended as it is reads bolder, most of all light on dark.
            let gamma = if smooth { SMOOTH_COVERAGE_GAMMA } else { 1.0 };
            let pixels = coverage
                .into_iter()
                .map(|a| {
                    let a = ((f32::from(a) / 255.0).powf(gamma) * 255.0).round() as u8;
                    Color32::from_rgba_premultiplied(a, a, a, a)
                })
                .collect();
            Some(GlyphBitmap {
                image: ColorImage::new([w, h], pixels),
                offset_px: egui::vec2(r.left as f32, r.top as f32),
                is_color: false,
            })
        }
    }
}

/// What Explorer's "Open with → Choose another app" asks of the chooser: the "Always use this
/// app" choice may be saved for the file's type (`ALLOW_REGISTRATION`, `REGISTER_EXT`), and the
/// app chosen opens the file (`EXEC`; without it the chooser only records the choice).
const OPEN_WITH_FLAGS: windows_sys::Win32::UI::Shell::OPEN_AS_INFO_FLAGS = {
    use windows_sys::Win32::UI::Shell::{OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OAIF_REGISTER_EXT};
    OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC
};

/// `path` as `SHOpenWithDialog` reads it: the path itself, wide and NUL-ended, with no quotes
/// round it (a path with spaces is one string here, not a command line).
fn open_with_file(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Windows' own "Open with" chooser for the file `path` ("How do you want to open this file?"),
/// as Explorer shows it: the app chosen there opens the file (#720). It waits until the chooser
/// is closed, so it is called on a thread of its own (with COM, apartment-threaded, as the shell
/// expects). `Err(HRESULT)` when Windows couldn't show it; closing the chooser without a choice
/// is `Ok`.
///
/// It used to run `rundll32 shell32.dll,OpenAs_RunDLL <path>`, which takes the rest of its
/// command line as the path, quotes included: the quotes a path with a space gets made it a file
/// that isn't there, and the chooser never showed.
pub fn open_with_dialog(path: &std::path::Path) -> Result<(), i32> {
    use windows_sys::Win32::Foundation::ERROR_CANCELLED;
    use windows_sys::Win32::UI::Shell::{OPENASINFO, SHOpenWithDialog};
    com_on_this_thread();
    let file = open_with_file(path);
    let info = OPENASINFO {
        pcszFile: file.as_ptr(),
        pcszClass: std::ptr::null(),
        oaifInFlags: OPEN_WITH_FLAGS,
    };
    // SAFETY: `info` and the NUL-ended `file` it points to outlive the call, which only reads
    // them. No owner window: the window's thread isn't this one, and an owner would tie the two
    // threads' input together while the chooser is open.
    let hr = unsafe { SHOpenWithDialog(std::ptr::null_mut(), &info) };
    // HRESULT_FROM_WIN32(ERROR_CANCELLED): the chooser closed without a choice.
    let cancelled = (0x8007_0000 | ERROR_CANCELLED) as i32;
    if hr >= 0 || hr == cancelled {
        Ok(())
    } else {
        Err(hr)
    }
}

#[cfg(test)]
mod open_with_tests {
    use super::*;

    #[test]
    fn the_chooser_opens_the_file_chosen() {
        use windows_sys::Win32::UI::Shell::{OAIF_EXEC, OAIF_REGISTER_EXT};
        assert_eq!(OPEN_WITH_FLAGS & OAIF_EXEC, OAIF_EXEC);
        assert_eq!(OPEN_WITH_FLAGS & OAIF_REGISTER_EXT, OAIF_REGISTER_EXT);
    }

    #[test]
    fn a_path_with_spaces_is_given_whole_and_unquoted() {
        let path = std::path::Path::new(r"C:\Users\A B\Downloads\my file.zip");
        let wide = open_with_file(path);
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&wide[..wide.len() - 1]).unwrap(),
            r"C:\Users\A B\Downloads\my file.zip"
        );
        assert!(!wide.contains(&u16::from(b'"')));
    }
}

/// Open a file with its registered default application via Win32 `ShellExecuteW` (#811).
///
/// Unlike invoking `explorer.exe <file>`, which fails with `ExitStatus(1)` on archive files
/// (.zip, .7z) when a third-party tool such as 7-Zip is the default handler, `ShellExecuteW`
/// invokes the user's registered file association directly.
pub fn open_file_shell(path: &std::path::Path) -> Result<(), std::io::Error> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    com_on_this_thread();
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: lpFile is a null-terminated UTF-16 string; lpOperation is NULL so Windows
    // executes the default action registered for the file type.
    let ret = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    let code = ret as usize;
    if code <= 32 {
        let err = std::io::Error::last_os_error();
        return Err(if err.raw_os_error().unwrap_or(0) != 0 {
            err
        } else {
            std::io::Error::from_raw_os_error(code as i32)
        });
    }
    Ok(())
}

/// The program Windows runs to open folders, when that isn't Explorer (a file manager such as
/// Directory Opus set up to replace it, #446); None for Explorer, or when Windows doesn't say.
pub fn folder_opener() -> Option<String> {
    use windows_sys::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_EXECUTABLE, AssocQueryStringW};
    other_folder_opener(|class, verb| {
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let class = wide(class);
        let verb = verb.map(wide);
        let mut out = vec![0u16; 1024];
        let mut len = out.len() as u32;
        // SAFETY: the strings end in NUL and outlive the call; `len` is `out`'s size in
        // characters, and the call writes at most that many.
        let hr = unsafe {
            AssocQueryStringW(
                ASSOCF_NONE,
                ASSOCSTR_EXECUTABLE,
                class.as_ptr(),
                verb.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
                out.as_mut_ptr(),
                &mut len,
            )
        };
        (hr == 0).then(|| {
            let end = out.iter().position(|&c| c == 0).unwrap_or(out.len());
            String::from_utf16_lossy(&out[..end])
        })
    })
}

/// Who opens a folder, asked of `executable_for(class, verb)` as the shell reads it: a folder is
/// a `Directory` first (where a replacement registers its verb, as the default or as `open`),
/// then a `Folder` (where Windows registers Explorer). An installed program's other verbs there
/// ("Open with Code") are not the default and aren't asked for.
fn other_folder_opener(
    executable_for: impl Fn(&str, Option<&str>) -> Option<String>,
) -> Option<String> {
    let opener = [
        ("Directory", None),
        ("Directory", Some("open")),
        ("Folder", None),
    ]
    .into_iter()
    .find_map(|(class, verb)| executable_for(class, verb))?;
    let name = opener.rsplit(['\\', '/']).next().unwrap_or(&opener);
    (!name.is_empty() && !name.eq_ignore_ascii_case("explorer.exe")).then_some(opener)
}

#[cfg(test)]
mod folder_opener_tests {
    use super::other_folder_opener;

    const EXPLORER: &str = r"C:\Windows\Explorer.exe";
    const OPUS: &str = r"C:\Program Files\GPSoftware\Directory Opus\dopusrt.exe";

    /// #446: as Windows is shipped (what `reg query` shows on a clean system), Explorer it is.
    #[test]
    fn explorer_as_shipped_is_no_other_opener() {
        let shipped =
            |c: &str, v: Option<&str>| (c == "Folder" && v.is_none()).then(|| EXPLORER.to_owned());
        assert_eq!(other_folder_opener(shipped), None);
    }

    /// A replacement's default verb on `Directory` wins over Explorer on `Folder`.
    #[test]
    fn a_replacement_on_directory_is_the_opener() {
        let opus = |c: &str, v: Option<&str>| match (c, v) {
            ("Directory", None) => Some(OPUS.to_owned()),
            ("Folder", None) => Some(EXPLORER.to_owned()),
            _ => None,
        };
        assert_eq!(other_folder_opener(opus).as_deref(), Some(OPUS));
    }

    /// One that replaces `Directory`'s `open` verb, or `Folder`'s own.
    #[test]
    fn a_replaced_open_verb_or_folder_is_the_opener() {
        let open = |c: &str, v: Option<&str>| match (c, v) {
            ("Directory", Some("open")) => Some(r"C:\Apps\files.exe".to_owned()),
            ("Folder", None) => Some(EXPLORER.to_owned()),
            _ => None,
        };
        assert_eq!(
            other_folder_opener(open).as_deref(),
            Some(r"C:\Apps\files.exe")
        );
        let folder = |c: &str, v: Option<&str>| {
            (c == "Folder" && v.is_none()).then(|| r"C:\Apps\tc.exe".to_owned())
        };
        assert_eq!(
            other_folder_opener(folder).as_deref(),
            Some(r"C:\Apps\tc.exe")
        );
    }

    /// Explorer written any way stays Explorer; nothing registered, nothing to say.
    #[test]
    fn explorer_any_case_or_no_answer_is_no_other_opener() {
        let upper = |_: &str, _: Option<&str>| Some(r"C:\WINDOWS\EXPLORER.EXE".to_owned());
        assert_eq!(other_folder_opener(upper), None);
        assert_eq!(other_folder_opener(|_, _| None), None);
    }
}

#[cfg(test)]
mod classic_text_tests {
    use egui::FontFamily;
    use egui::epaint::text::GlyphRasterizerRequest;

    fn glyphs() -> super::classic_text::Glyphs {
        let dir = std::env::var_os("WINDIR")
            .map(|w| std::path::PathBuf::from(w).join("Fonts"))
            .unwrap();
        super::classic_text::Glyphs::new(dir).unwrap()
    }

    fn req<'a>(cluster: &'a str, family: &'a FontFamily, size: f32) -> GlyphRasterizerRequest<'a> {
        GlyphRasterizerRequest {
            cluster,
            family,
            font_size_px: size,
            subpixel_offset_px: 0.0,
        }
    }

    /// #334: every letter gets a whole pixel width, and is drawn within it: the two can't drift
    /// apart any more. The letters of the words the report names, at the sizes of 100 % to 200 %.
    #[test]
    fn letters_have_whole_pixel_advances_and_stay_inside_them() {
        let mut g = glyphs();
        for family in [FontFamily::Proportional, FontFamily::Name("tahoma".into())] {
            for size in [11.0, 12.0, 13.2, 15.0, 16.5, 18.0, 24.0] {
                for ch in "downloadefaultakeDRsum".chars() {
                    let text = ch.to_string();
                    let glyph = g.glyph(&req(&text, &family, size), false, false).unwrap();
                    assert_eq!(glyph.advance_px.fract(), 0.0, "{ch} at {size}");
                    assert!(glyph.advance_px >= 1.0, "{ch} at {size}");
                    let right = glyph.bitmap.offset_px.x + glyph.bitmap.image.size[0] as f32;
                    assert!(
                        right <= glyph.advance_px + 1.0,
                        "{ch} at {size}: ink ends at {right}, advance {}",
                        glyph.advance_px
                    );
                }
            }
        }
    }

    /// #837: Chinese, Japanese and Korean characters are drawn by the same rasterizer as the
    /// Latin beside them, at whole-pixel widths in the classic look. Fractional interface sizes
    /// (110 %, 115 %…) round to a whole em instead of blurring the CJK fallback.
    #[test]
    fn cjk_letters_are_drawn_with_whole_pixel_advances() {
        let mut g = glyphs();
        let dir = std::env::var_os("WINDIR")
            .map(|w| std::path::PathBuf::from(w).join("Fonts"))
            .unwrap();
        for ch in ['中', '文', '語', 'か', '한', '，', '。'] {
            // A computer without that script's font (a CI image with no CJK fonts) draws it
            // through egui's provider, if at all: nothing to check here.
            if !crate::theme::cjk_font_files(ch)
                .iter()
                .any(|f| dir.join(f).exists())
            {
                eprintln!("no font for {ch}, not checked");
                continue;
            }
            for size in [11.0, 12.0, 13.2] {
                let text = ch.to_string();
                let glyph = g
                    .glyph(&req(&text, &FontFamily::Proportional, size), false, false)
                    .unwrap_or_else(|| panic!("{ch} at {size} not drawn"));
                assert_eq!(glyph.advance_px.fract(), 0.0, "{ch} at {size}");
                assert!(glyph.advance_px >= 1.0, "{ch} at {size}");
                assert!(glyph.bitmap.image.size[0] > 0, "{ch} at {size}: no bitmap");
            }
        }
    }

    /// #837: a mark that a shaper must place with its base, and every multi-codepoint cluster,
    /// stay with egui.
    #[test]
    fn cjk_marks_and_clusters_stay_with_egui() {
        let mut g = glyphs();
        let f = FontFamily::Proportional;
        for not in [
            "\u{302A}",         // combining left tone mark
            "\u{3099}",         // combining voiced sound mark
            "\u{304B}\u{3099}", // か + voiced mark
            "\u{4E2D}\u{302A}", // 中 + tone mark
        ] {
            assert!(
                g.glyph(&req(not, &f, 12.0), false, false).is_none(),
                "{not:?}"
            );
        }
    }

    /// #837: the boundaries of the ranges the rasterizer takes, and the ones a shaper must keep
    /// (conjoining Jamo, combining marks, halfwidth sound marks).
    #[test]
    fn cjk_script_boundaries() {
        use crate::theme::cjk_script;
        for not in [
            '\u{1100}',
            '\u{11FF}',
            '\u{A960}',
            '\u{A97F}',
            '\u{D7B0}',
            '\u{D7FF}',
            '\u{302A}',
            '\u{302F}',
            '\u{3099}',
            '\u{309A}',
            '\u{FF9E}',
            '\u{FF9F}',
            '\u{2000}',
            '\u{1FFFF}',
            '\u{40000}',
        ] {
            assert_eq!(cjk_script(not), None, "U+{:04X}", not as u32);
        }
        for c in ['\u{3040}', '\u{30FF}', '\u{FF9D}'] {
            assert_eq!(cjk_script(c), Some("japanese"), "U+{:04X}", c as u32);
        }
        for c in ['\u{3130}', '\u{318F}', '\u{AC00}', '\u{D7A3}'] {
            assert_eq!(cjk_script(c), Some("hangul"), "U+{:04X}", c as u32);
        }
        for c in ['\u{4E00}', '\u{9FFF}', '\u{20000}', '\u{3FFFF}'] {
            assert_eq!(cjk_script(c), Some("han"), "U+{:04X}", c as u32);
        }
    }

    /// #837: the interface language's own face leads its Han text; kana and Hangul keep their
    /// script's face whatever the interface language. A language parameter, so no global race.
    #[test]
    fn cjk_faces_follow_the_interface_language() {
        use crate::theme::cjk_font_files_for;
        assert_eq!(cjk_font_files_for('\u{4E2D}', "zh-hans")[0], "msyh.ttc");
        assert_eq!(cjk_font_files_for('\u{4E2D}', "zh-hant")[0], "msjh.ttc");
        assert_eq!(cjk_font_files_for('\u{4E2D}', "ja")[0], "YuGothM.ttc");
        assert_eq!(cjk_font_files_for('\u{4E2D}', "ko")[0], "malgun.ttf");
        assert_eq!(cjk_font_files_for('\u{4E2D}', "en")[0], "msyh.ttc");
        assert_eq!(cjk_font_files_for('\u{304B}', "ko")[0], "YuGothM.ttc");
        assert_eq!(cjk_font_files_for('\u{D55C}', "zh-hans")[0], "malgun.ttf");
    }

    /// #837: a supplementary Han character no face has returns to egui instead of drawing tofu.
    #[test]
    fn an_unassigned_supplementary_han_falls_back_to_egui() {
        let mut g = glyphs();
        assert!(
            g.glyph(
                &req("\u{3FFFF}", &FontFamily::Proportional, 12.0),
                false,
                false
            )
            .is_none()
        );
    }

    /// The smooth look keeps the font's own width and draws more levels of grey than the
    /// classic one, whose letters are fitted to whole pixels.
    #[test]
    fn smooth_letters_keep_their_width_and_their_greys() {
        let mut g = glyphs();
        let f = FontFamily::Proportional;
        let levels = |glyph: &egui::epaint::text::RasterizedGlyph| {
            let mut l: Vec<u8> = glyph.bitmap.image.pixels.iter().map(|p| p.a()).collect();
            l.sort_unstable();
            l.dedup();
            l.len()
        };
        let classic = g.glyph(&req("o", &f, 13.2), false, false).unwrap();
        let smooth = g.glyph(&req("o", &f, 13.2), false, true).unwrap();
        assert_eq!(classic.advance_px.fract(), 0.0);
        assert!(smooth.advance_px.fract() != 0.0, "{}", smooth.advance_px);
        assert!(levels(&smooth) > levels(&classic));
        // A quarter pixel to the right changes the picture, not the width.
        let mut shifted = req("o", &f, 13.2);
        shifted.subpixel_offset_px = 0.5;
        let moved = g.glyph(&shifted, false, true).unwrap();
        assert_eq!(moved.advance_px, smooth.advance_px);
        assert_ne!(moved.bitmap.image.pixels, smooth.bitmap.image.pixels);
    }

    #[test]
    fn a_space_keeps_its_advance() {
        let mut g = glyphs();
        let s = g
            .glyph(&req(" ", &FontFamily::Proportional, 12.0), false, false)
            .unwrap();
        assert!(s.advance_px > 0.0);
    }

    #[test]
    fn what_needs_shaping_stays_with_egui() {
        let mut g = glyphs();
        let f = FontFamily::Proportional;
        for not in [
            "\u{62a}",
            "\u{5d0}",
            "\u{93e}",
            "\u{e14}",
            "e\u{301}",
            "\u{1F600}",
        ] {
            assert!(
                g.glyph(&req(not, &f, 12.0), false, false).is_none(),
                "{not:?}"
            );
        }
        // A bracket is mirrored by the shaper in a right-to-left line, only.
        assert!(g.glyph(&req("(", &f, 12.0), false, false).is_some());
        assert!(g.glyph(&req("(", &f, 12.0), true, false).is_none());
        // A family it isn't told about stays with egui.
        let other = FontFamily::Name("other".into());
        assert!(g.glyph(&req("a", &other, 12.0), false, false).is_none());
    }
}
