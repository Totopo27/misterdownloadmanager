//! Global shortcut to show or hide the main window (#558), set in Options → General.
//! Supports 2-key (one modifier + key, e.g. `Cmd+U`, `Alt+Space`) and 3-key (two modifiers + key,
//! e.g. `Cmd+Shift+M`, `Ctrl+Alt+D`) combinations.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A global key combination: one or more modifiers (`Ctrl`, `Alt`, `Shift`, `Cmd`) and a main key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
    pub key: HotKeyCode,
}

/// Supported main keys for the global shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotKeyCode {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Space,
    Tab,
    Enter,
    Backquote,
    Minus,
    Equals,
    BracketLeft,
    BracketRight,
    Backslash,
    Semicolon,
    Quote,
    Comma,
    Period,
    Slash,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

impl HotKeyCode {
    pub fn name(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
            Self::H => "H",
            Self::I => "I",
            Self::J => "J",
            Self::K => "K",
            Self::L => "L",
            Self::M => "M",
            Self::N => "N",
            Self::O => "O",
            Self::P => "P",
            Self::Q => "Q",
            Self::R => "R",
            Self::S => "S",
            Self::T => "T",
            Self::U => "U",
            Self::V => "V",
            Self::W => "W",
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
            Self::Digit0 => "0",
            Self::Digit1 => "1",
            Self::Digit2 => "2",
            Self::Digit3 => "3",
            Self::Digit4 => "4",
            Self::Digit5 => "5",
            Self::Digit6 => "6",
            Self::Digit7 => "7",
            Self::Digit8 => "8",
            Self::Digit9 => "9",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
            Self::Space => "Space",
            Self::Tab => "Tab",
            Self::Enter => "Enter",
            Self::Backquote => "`",
            Self::Minus => "-",
            Self::Equals => "=",
            Self::BracketLeft => "[",
            Self::BracketRight => "]",
            Self::Backslash => "\\",
            Self::Semicolon => ";",
            Self::Quote => "'",
            Self::Comma => ",",
            Self::Period => ".",
            Self::Slash => "/",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        match token.trim().to_ascii_uppercase().as_str() {
            "A" => Some(Self::A),
            "B" => Some(Self::B),
            "C" => Some(Self::C),
            "D" => Some(Self::D),
            "E" => Some(Self::E),
            "F" => Some(Self::F),
            "G" => Some(Self::G),
            "H" => Some(Self::H),
            "I" => Some(Self::I),
            "J" => Some(Self::J),
            "K" => Some(Self::K),
            "L" => Some(Self::L),
            "M" => Some(Self::M),
            "N" => Some(Self::N),
            "O" => Some(Self::O),
            "P" => Some(Self::P),
            "Q" => Some(Self::Q),
            "R" => Some(Self::R),
            "S" => Some(Self::S),
            "T" => Some(Self::T),
            "U" => Some(Self::U),
            "V" => Some(Self::V),
            "W" => Some(Self::W),
            "X" => Some(Self::X),
            "Y" => Some(Self::Y),
            "Z" => Some(Self::Z),
            "0" => Some(Self::Digit0),
            "1" => Some(Self::Digit1),
            "2" => Some(Self::Digit2),
            "3" => Some(Self::Digit3),
            "4" => Some(Self::Digit4),
            "5" => Some(Self::Digit5),
            "6" => Some(Self::Digit6),
            "7" => Some(Self::Digit7),
            "8" => Some(Self::Digit8),
            "9" => Some(Self::Digit9),
            "F1" => Some(Self::F1),
            "F2" => Some(Self::F2),
            "F3" => Some(Self::F3),
            "F4" => Some(Self::F4),
            "F5" => Some(Self::F5),
            "F6" => Some(Self::F6),
            "F7" => Some(Self::F7),
            "F8" => Some(Self::F8),
            "F9" => Some(Self::F9),
            "F10" => Some(Self::F10),
            "F11" => Some(Self::F11),
            "F12" => Some(Self::F12),
            "SPACE" => Some(Self::Space),
            "TAB" => Some(Self::Tab),
            "ENTER" | "RETURN" => Some(Self::Enter),
            "`" | "BACKQUOTE" | "GRAVE" => Some(Self::Backquote),
            "-" | "MINUS" => Some(Self::Minus),
            "=" | "EQUALS" | "PLUS" => Some(Self::Equals),
            "[" | "BRACKETLEFT" | "OPENBRACKET" => Some(Self::BracketLeft),
            "]" | "BRACKETRIGHT" | "CLOSEBRACKET" => Some(Self::BracketRight),
            "\\" | "BACKSLASH" => Some(Self::Backslash),
            ";" | "SEMICOLON" => Some(Self::Semicolon),
            "'" | "QUOTE" | "APOSTROPHE" => Some(Self::Quote),
            "," | "COMMA" => Some(Self::Comma),
            "." | "PERIOD" => Some(Self::Period),
            "/" | "SLASH" => Some(Self::Slash),
            "UP" | "ARROWUP" => Some(Self::Up),
            "DOWN" | "ARROWDOWN" => Some(Self::Down),
            "LEFT" | "ARROWLEFT" => Some(Self::Left),
            "RIGHT" | "ARROWRIGHT" => Some(Self::Right),
            "HOME" => Some(Self::Home),
            "END" => Some(Self::End),
            "PAGEUP" | "PGUP" => Some(Self::PageUp),
            "PAGEDOWN" | "PGDN" => Some(Self::PageDown),
            _ => None,
        }
    }

    pub fn from_egui(key: egui::Key) -> Option<Self> {
        use egui::Key;
        match key {
            Key::A => Some(Self::A),
            Key::B => Some(Self::B),
            Key::C => Some(Self::C),
            Key::D => Some(Self::D),
            Key::E => Some(Self::E),
            Key::F => Some(Self::F),
            Key::G => Some(Self::G),
            Key::H => Some(Self::H),
            Key::I => Some(Self::I),
            Key::J => Some(Self::J),
            Key::K => Some(Self::K),
            Key::L => Some(Self::L),
            Key::M => Some(Self::M),
            Key::N => Some(Self::N),
            Key::O => Some(Self::O),
            Key::P => Some(Self::P),
            Key::Q => Some(Self::Q),
            Key::R => Some(Self::R),
            Key::S => Some(Self::S),
            Key::T => Some(Self::T),
            Key::U => Some(Self::U),
            Key::V => Some(Self::V),
            Key::W => Some(Self::W),
            Key::X => Some(Self::X),
            Key::Y => Some(Self::Y),
            Key::Z => Some(Self::Z),
            Key::Num0 => Some(Self::Digit0),
            Key::Num1 => Some(Self::Digit1),
            Key::Num2 => Some(Self::Digit2),
            Key::Num3 => Some(Self::Digit3),
            Key::Num4 => Some(Self::Digit4),
            Key::Num5 => Some(Self::Digit5),
            Key::Num6 => Some(Self::Digit6),
            Key::Num7 => Some(Self::Digit7),
            Key::Num8 => Some(Self::Digit8),
            Key::Num9 => Some(Self::Digit9),
            Key::F1 => Some(Self::F1),
            Key::F2 => Some(Self::F2),
            Key::F3 => Some(Self::F3),
            Key::F4 => Some(Self::F4),
            Key::F5 => Some(Self::F5),
            Key::F6 => Some(Self::F6),
            Key::F7 => Some(Self::F7),
            Key::F8 => Some(Self::F8),
            Key::F9 => Some(Self::F9),
            Key::F10 => Some(Self::F10),
            Key::F11 => Some(Self::F11),
            Key::F12 => Some(Self::F12),
            Key::Space => Some(Self::Space),
            Key::Tab => Some(Self::Tab),
            Key::Enter => Some(Self::Enter),
            Key::Backtick => Some(Self::Backquote),
            Key::Minus => Some(Self::Minus),
            Key::Equals | Key::Plus => Some(Self::Equals),
            Key::OpenBracket => Some(Self::BracketLeft),
            Key::CloseBracket => Some(Self::BracketRight),
            Key::Backslash => Some(Self::Backslash),
            Key::Semicolon => Some(Self::Semicolon),
            Key::Quote => Some(Self::Quote),
            Key::Comma => Some(Self::Comma),
            Key::Period => Some(Self::Period),
            Key::Slash => Some(Self::Slash),
            Key::ArrowUp => Some(Self::Up),
            Key::ArrowDown => Some(Self::Down),
            Key::ArrowLeft => Some(Self::Left),
            Key::ArrowRight => Some(Self::Right),
            Key::Home => Some(Self::Home),
            Key::End => Some(Self::End),
            Key::PageUp => Some(Self::PageUp),
            Key::PageDown => Some(Self::PageDown),
            _ => None,
        }
    }

    /// Carbon `kVK_*` virtual key code (`<HIToolbox/Events.h>`).
    #[cfg(any(target_os = "macos", test))]
    pub fn carbon_vk(self) -> u32 {
        match self {
            Self::A => 0x00,
            Self::S => 0x01,
            Self::D => 0x02,
            Self::F => 0x03,
            Self::H => 0x04,
            Self::G => 0x05,
            Self::Z => 0x06,
            Self::X => 0x07,
            Self::C => 0x08,
            Self::V => 0x09,
            Self::B => 0x0B,
            Self::Q => 0x0C,
            Self::W => 0x0D,
            Self::E => 0x0E,
            Self::R => 0x0F,
            Self::Y => 0x10,
            Self::T => 0x11,
            Self::Digit1 => 0x12,
            Self::Digit2 => 0x13,
            Self::Digit3 => 0x14,
            Self::Digit4 => 0x15,
            Self::Digit6 => 0x16,
            Self::Digit5 => 0x17,
            Self::Equals => 0x18,
            Self::Digit9 => 0x19,
            Self::Digit7 => 0x1A,
            Self::Minus => 0x1B,
            Self::Digit8 => 0x1C,
            Self::Digit0 => 0x1D,
            Self::BracketRight => 0x1E,
            Self::O => 0x1F,
            Self::U => 0x20,
            Self::BracketLeft => 0x21,
            Self::I => 0x22,
            Self::P => 0x23,
            Self::Enter => 0x24,
            Self::L => 0x25,
            Self::J => 0x26,
            Self::Quote => 0x27,
            Self::K => 0x28,
            Self::Semicolon => 0x29,
            Self::Backslash => 0x2A,
            Self::Comma => 0x2B,
            Self::Slash => 0x2C,
            Self::N => 0x2D,
            Self::M => 0x2E,
            Self::Period => 0x2F,
            Self::Tab => 0x30,
            Self::Space => 0x31,
            Self::Backquote => 0x32,
            Self::F1 => 0x7A,
            Self::F2 => 0x78,
            Self::F3 => 0x63,
            Self::F4 => 0x76,
            Self::F5 => 0x60,
            Self::F6 => 0x61,
            Self::F7 => 0x62,
            Self::F8 => 0x64,
            Self::F9 => 0x65,
            Self::F10 => 0x6D,
            Self::F11 => 0x67,
            Self::F12 => 0x6F,
            Self::Home => 0x73,
            Self::PageUp => 0x74,
            Self::End => 0x77,
            Self::PageDown => 0x79,
            Self::Left => 0x7B,
            Self::Right => 0x7C,
            Self::Down => 0x7D,
            Self::Up => 0x7E,
        }
    }

    /// Windows `VK_*` virtual-key code (`WinUser.h`).
    #[cfg(any(windows, test))]
    pub fn win32_vk(self) -> u32 {
        match self {
            Self::A => 0x41,
            Self::B => 0x42,
            Self::C => 0x43,
            Self::D => 0x44,
            Self::E => 0x45,
            Self::F => 0x46,
            Self::G => 0x47,
            Self::H => 0x48,
            Self::I => 0x49,
            Self::J => 0x4A,
            Self::K => 0x4B,
            Self::L => 0x4C,
            Self::M => 0x4D,
            Self::N => 0x4E,
            Self::O => 0x4F,
            Self::P => 0x50,
            Self::Q => 0x51,
            Self::R => 0x52,
            Self::S => 0x53,
            Self::T => 0x54,
            Self::U => 0x55,
            Self::V => 0x56,
            Self::W => 0x57,
            Self::X => 0x58,
            Self::Y => 0x59,
            Self::Z => 0x5A,
            Self::Digit0 => 0x30,
            Self::Digit1 => 0x31,
            Self::Digit2 => 0x32,
            Self::Digit3 => 0x33,
            Self::Digit4 => 0x34,
            Self::Digit5 => 0x35,
            Self::Digit6 => 0x36,
            Self::Digit7 => 0x37,
            Self::Digit8 => 0x38,
            Self::Digit9 => 0x39,
            Self::F1 => 0x70,
            Self::F2 => 0x71,
            Self::F3 => 0x72,
            Self::F4 => 0x73,
            Self::F5 => 0x74,
            Self::F6 => 0x75,
            Self::F7 => 0x76,
            Self::F8 => 0x77,
            Self::F9 => 0x78,
            Self::F10 => 0x79,
            Self::F11 => 0x7A,
            Self::F12 => 0x7B,
            Self::Space => 0x20,
            Self::Tab => 0x09,
            Self::Enter => 0x0D,
            Self::Backquote => 0xC0,
            Self::Minus => 0xBD,
            Self::Equals => 0xBB,
            Self::BracketLeft => 0xDB,
            Self::BracketRight => 0xDD,
            Self::Backslash => 0xDC,
            Self::Semicolon => 0xBA,
            Self::Quote => 0xDE,
            Self::Comma => 0xBC,
            Self::Period => 0xBE,
            Self::Slash => 0xBF,
            Self::Up => 0x26,
            Self::Down => 0x28,
            Self::Left => 0x25,
            Self::Right => 0x27,
            Self::Home => 0x24,
            Self::End => 0x23,
            Self::PageUp => 0x21,
            Self::PageDown => 0x22,
        }
    }
}

impl Shortcut {
    /// Number of modifiers in this shortcut (1 = 2-key combo, 2 = 3-key combo).
    #[cfg(test)]
    pub fn modifier_count(self) -> usize {
        usize::from(self.ctrl)
            + usize::from(self.alt)
            + usize::from(self.shift)
            + usize::from(self.cmd)
    }

    /// Total number of keys in the combination (`modifier_count + 1`).
    #[cfg(test)]
    pub fn key_count(self) -> usize {
        self.modifier_count() + 1
    }

    /// True if this combination is safe to register system-wide: requires at least one of `Ctrl`,
    /// `Alt`, or `Cmd` (never `Shift` alone, which would swallow uppercase letters or shifted keys
    /// across all applications), and refuses everyday single-modifier editing shortcuts (`Ctrl+A`,
    /// `Ctrl+C`, `Ctrl+V`, `Ctrl+X`, `Ctrl+Z`, and `Cmd+A/C/V/X/Z`).
    pub fn is_allowed(self) -> bool {
        if !(self.ctrl || self.alt || self.cmd) {
            return false;
        }
        let single_ctrl_or_cmd = !self.alt && !self.shift && (self.ctrl != self.cmd);
        if single_ctrl_or_cmd
            && matches!(
                self.key,
                HotKeyCode::A | HotKeyCode::C | HotKeyCode::V | HotKeyCode::X | HotKeyCode::Z
            )
        {
            return false;
        }
        true
    }

    /// Parse a saved shortcut string such as `"Cmd+U"`, `"Cmd+Shift+M"`, `"Ctrl+Alt+D"`, or
    /// `"Alt+Space"`. Requires at least 1 non-Shift modifier (2-key or 3-key combination).
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut cmd = false;
        let mut main_key = None;

        for part in trimmed.split('+').map(str::trim).filter(|s| !s.is_empty()) {
            match part.to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" | "⌃" => ctrl = true,
                "ALT" | "OPTION" | "OPT" | "⌥" => alt = true,
                "SHIFT" | "⇧" => shift = true,
                "CMD" | "COMMAND" | "SUPER" | "WIN" | "META" | "⌘" => cmd = true,
                _ => {
                    if main_key.is_some() {
                        return None;
                    }
                    main_key = Some(HotKeyCode::parse(part)?);
                }
            }
        }

        let key = main_key?;
        let s = Self {
            ctrl,
            alt,
            shift,
            cmd,
            key,
        };
        s.is_allowed().then_some(s)
    }

    /// Canonical string stored in `Settings::show_hide_hotkey` (e.g. `"Cmd+Shift+M"`).
    pub fn canonical(self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.cmd {
            parts.push("Cmd");
        }
        parts.push(self.key.name());
        parts.join("+")
    }

    /// Display text in the Options box (`Cmd` / `Option` on macOS, `Ctrl` / `Alt` elsewhere).
    pub fn display(self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push(if cfg!(target_os = "macos") {
                "Option"
            } else {
                "Alt"
            });
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.cmd {
            parts.push(if cfg!(target_os = "macos") {
                "Cmd"
            } else {
                "Super"
            });
        }
        parts.push(self.key.name());
        parts.join("+")
    }

    /// Carbon modifier mask (`cmdKey`, `shiftKey`, `optionKey`, `controlKey`).
    #[cfg(any(target_os = "macos", test))]
    pub fn carbon_modifiers(self) -> u32 {
        const CMD_KEY: u32 = 1 << 8;
        const SHIFT_KEY: u32 = 1 << 9;
        const OPTION_KEY: u32 = 1 << 11;
        const CONTROL_KEY: u32 = 1 << 12;
        let mut m = 0;
        if self.cmd {
            m |= CMD_KEY;
        }
        if self.shift {
            m |= SHIFT_KEY;
        }
        if self.alt {
            m |= OPTION_KEY;
        }
        if self.ctrl {
            m |= CONTROL_KEY;
        }
        m
    }

    /// Win32 modifier mask (`MOD_CONTROL`, `MOD_ALT`, `MOD_SHIFT`, `MOD_WIN`, `MOD_NOREPEAT`).
    #[cfg(any(windows, test))]
    pub fn win32_modifiers(self) -> u32 {
        const MOD_ALT: u32 = 0x0001;
        const MOD_CONTROL: u32 = 0x0002;
        const MOD_SHIFT: u32 = 0x0004;
        const MOD_WIN: u32 = 0x0008;
        const MOD_NOREPEAT: u32 = 0x4000;
        let mut m = MOD_NOREPEAT;
        if self.alt {
            m |= MOD_ALT;
        }
        if self.ctrl {
            m |= MOD_CONTROL;
        }
        if self.shift {
            m |= MOD_SHIFT;
        }
        if self.cmd {
            m |= MOD_WIN;
        }
        m
    }
}

/// Result of inspecting egui input while the shortcut recorder box is focused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureEvent {
    None,
    Cancel,
    Clear,
    Recorded(Shortcut),
}

/// Inspects `egui::InputState` while the user is recording a shortcut in Options → General.
pub fn capture_from_egui(input: &egui::InputState) -> CaptureEvent {
    for ev in &input.events {
        let egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = ev
        else {
            continue;
        };
        let has_mod = modifiers.ctrl || modifiers.alt || modifiers.shift || modifiers.mac_cmd;
        if !has_mod {
            match key {
                egui::Key::Escape => return CaptureEvent::Cancel,
                egui::Key::Backspace | egui::Key::Delete => return CaptureEvent::Clear,
                _ => continue,
            }
        }
        if let Some(code) = HotKeyCode::from_egui(*key) {
            let shortcut = Shortcut {
                ctrl: modifiers.ctrl,
                alt: modifiers.alt,
                shift: modifiers.shift,
                cmd: modifiers.mac_cmd,
                key: code,
            };
            if shortcut.is_allowed() {
                return CaptureEvent::Recorded(shortcut);
            }
        }
    }
    CaptureEvent::None
}

/// Live preview of held modifiers while recording (e.g. `"Cmd+Shift+..."`).
pub fn pending_modifiers_preview(modifiers: egui::Modifiers) -> Option<String> {
    let mut parts = Vec::with_capacity(4);
    if modifiers.ctrl {
        parts.push("Ctrl");
    }
    if modifiers.alt {
        parts.push(if cfg!(target_os = "macos") {
            "Option"
        } else {
            "Alt"
        });
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    if modifiers.mac_cmd {
        parts.push(if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Super"
        });
    }
    if parts.is_empty() {
        None
    } else {
        parts.push("...");
        Some(parts.join("+"))
    }
}

// ---- Platform global hotkey registration -------------------------------------------------------

/// Active global hotkey registration; unregisters on drop or when updated.
pub struct GlobalHotkey {
    current: String,
    fired: Arc<AtomicBool>,
    taken: Arc<AtomicBool>,
    #[cfg(target_os = "macos")]
    inner: Option<crate::unix::GlobalHotKeyOs>,
    #[cfg(windows)]
    inner: Option<crate::win32::GlobalHotKeyOs>,
}

impl GlobalHotkey {
    pub fn new(ctx: &egui::Context, spec: &str) -> Self {
        let fired = Arc::new(AtomicBool::new(false));
        let taken = Arc::new(AtomicBool::new(false));
        let mut h = Self {
            current: String::new(),
            fired,
            taken,
            #[cfg(target_os = "macos")]
            inner: None,
            #[cfg(windows)]
            inner: None,
        };
        h.apply(ctx, spec);
        h
    }

    /// Re-registers the global hotkey only when `spec` changed.
    pub fn apply(&mut self, ctx: &egui::Context, spec: &str) {
        let normalized = Shortcut::parse(spec)
            .map(Shortcut::canonical)
            .unwrap_or_default();
        if self.current == normalized {
            #[cfg(target_os = "macos")]
            if normalized.is_empty() || self.inner.is_some() || self.taken.load(Ordering::SeqCst) {
                return;
            }
            #[cfg(not(target_os = "macos"))]
            return;
        }
        self.current = normalized.clone();
        self.fired.store(false, Ordering::SeqCst);
        self.taken.store(false, Ordering::SeqCst);

        #[cfg(target_os = "macos")]
        {
            self.inner = None;
            if let Some(shortcut) = Shortcut::parse(&normalized) {
                self.inner = crate::unix::GlobalHotKeyOs::register(
                    shortcut,
                    self.fired.clone(),
                    self.taken.clone(),
                    ctx.clone(),
                );
            }
        }

        #[cfg(windows)]
        {
            self.inner = None;
            if let Some(shortcut) = Shortcut::parse(&normalized) {
                self.inner = crate::win32::GlobalHotKeyOs::register(
                    shortcut,
                    self.fired.clone(),
                    self.taken.clone(),
                    ctx.clone(),
                );
            }
        }

        #[cfg(target_os = "linux")]
        let _ = ctx;
    }

    /// Returns `true` if the global hotkey was pressed since the last check.
    pub fn take_fired(&self) -> bool {
        self.fired.swap(false, Ordering::SeqCst)
    }

    /// Returns `true` if the chosen shortcut could not be registered because another program holds it.
    pub fn is_taken(&self) -> bool {
        self.taken.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_key_and_three_key_combinations() {
        let two = Shortcut::parse("Cmd+U").unwrap();
        assert_eq!(two.key_count(), 2);
        assert!(two.cmd && !two.ctrl && !two.alt && !two.shift);
        assert_eq!(two.key, HotKeyCode::U);
        assert_eq!(two.canonical(), "Cmd+U");
        assert_eq!(two.key.carbon_vk(), 0x20);
        assert_eq!(two.carbon_modifiers(), 1 << 8);
        assert_eq!(two.key.win32_vk(), 0x55);
        assert_eq!(two.win32_modifiers(), 0x4000 | 0x0008);

        let three = Shortcut::parse("Cmd+Shift+M").unwrap();
        assert_eq!(three.key_count(), 3);
        assert!(three.cmd && three.shift && !three.ctrl && !three.alt);
        assert_eq!(three.key, HotKeyCode::M);
        assert_eq!(three.canonical(), "Shift+Cmd+M");

        let opt_space = Shortcut::parse("Option+Space").unwrap();
        assert_eq!(opt_space.key_count(), 2);
        assert!(opt_space.alt);
        assert_eq!(opt_space.key, HotKeyCode::Space);
        assert_eq!(opt_space.canonical(), "Alt+Space");

        let ctrl_alt_d = Shortcut::parse("Ctrl+Alt+D").unwrap();
        assert_eq!(ctrl_alt_d.key_count(), 3);
        assert!(ctrl_alt_d.ctrl && ctrl_alt_d.alt);
        assert_eq!(ctrl_alt_d.canonical(), "Ctrl+Alt+D");

        // Bare key without modifiers is rejected.
        assert!(Shortcut::parse("U").is_none());
        assert!(Shortcut::parse("Space").is_none());
        assert!(Shortcut::parse("").is_none());
    }

    #[test]
    fn rejects_shift_alone_and_everyday_editing_shortcuts() {
        for forbidden in [
            "Shift+A",
            "Shift+1",
            "Shift+Enter",
            "Ctrl+A",
            "Ctrl+C",
            "Ctrl+V",
            "Ctrl+X",
            "Ctrl+Z",
            "Cmd+A",
            "Cmd+C",
            "Cmd+V",
            "Cmd+X",
            "Cmd+Z",
        ] {
            assert!(
                Shortcut::parse(forbidden).is_none(),
                "{forbidden} should be rejected"
            );
        }
        // Two-modifier combinations with Shift or Alt remain allowed.
        assert!(Shortcut::parse("Ctrl+Shift+A").is_some());
        assert!(Shortcut::parse("Cmd+Shift+C").is_some());
        assert!(Shortcut::parse("Ctrl+Alt+V").is_some());
    }
}
