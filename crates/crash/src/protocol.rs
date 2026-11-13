//! What the app tells its monitor (minidumper messages: a kind and a JSON body), and the monitor's
//! own record of a session.

use serde::{Deserialize, Serialize};

/// Message kinds (minidumper's `send_message(kind, …)`).
pub mod kind {
    /// [`super::Session`]: once, after connecting.
    pub const SESSION: u32 = 1;
    /// [`super::Panic`]: Rust code panicked; a dump follows.
    pub const PANIC: u32 = 2;
    /// [`super::Freeze`]: the window stopped answering; a dump follows.
    pub const FREEZE: u32 = 3;
    /// The window answers again after a freeze (empty body).
    pub const RECOVERED: u32 = 4;
    /// The app is closing normally (empty body): its disconnecting is no crash.
    pub const CLEAN: u32 = 5;
    /// [`super::AppState`]: what is open and running, sent when it changes.
    pub const STATE: u32 = 6;
}

/// The app and the system it runs on, sent when the session starts.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub version: String,
    pub commit: String,
    /// How it was installed, as the issue forms word it ("The installer (setup.exe)").
    pub install: String,
    /// "Windows", "macOS" or "Linux".
    pub os: String,
    /// The system's own version text ("Windows 11 Pro 24H2 (26100)").
    pub os_version: String,
    pub arch: String,
    pub language: String,
    pub dark: bool,
    pub pid: u32,
    /// The program's path (shown with the home folder as ~).
    pub exe: String,
    /// Seconds since the Unix epoch.
    pub started: i64,
    /// What draws the windows ("Direct3D 12 on …", "OpenGL"), once known (#89).
    #[serde(default)]
    pub renderer: String,
}

/// A panic: its message, where, on which thread, and Rust's own backtrace of it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Panic {
    pub message: String,
    pub location: String,
    pub thread: String,
    pub backtrace: String,
}

/// The window stopped answering: the thread that draws it and how long one frame has run.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Freeze {
    /// The window thread's id as the system knows it (Windows' thread id, Linux's tid: the dump's
    /// thread list uses the same); 0 where the dump's ids can't be known from inside (macOS:
    /// Mach port names of the monitor's own).
    pub ui_thread: u32,
    /// The window thread is the process's main thread (always so on macOS): the dump's first.
    #[serde(default)]
    pub main: bool,
    pub seconds: u64,
    /// How much the window thread ran while the frame went on (None: not measured, by an older
    /// MDM or on macOS). It tells a loop from a wait from a computer too busy to run MDM.
    #[serde(default)]
    pub busy: Option<Busy>,
    /// What the frame was doing when the watchdog saw it stop ("the tray icon"; empty: not
    /// known). It names the wait even when the dump comes too late to show it, as on a computer
    /// slow to wake (#574).
    #[serde(default)]
    pub step: String,
}

/// The window thread's use of the processor over a stretch of a frozen frame.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Busy {
    /// The stretch measured, in ms of real time.
    pub wall_ms: u64,
    /// The processor time the window thread got in it (user + kernel), in ms.
    pub cpu_ms: u64,
    /// Page faults in it (Windows: the whole process's, soft ones included; Linux: the window
    /// thread's from disk). None where unknown.
    #[serde(default)]
    pub faults: Option<u64>,
}

/// What MDM had open and running (counts only: no addresses or file names).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AppState {
    pub downloads: usize,
    pub running: usize,
    pub queues: usize,
    /// The ids of the windows open ("main", "add-url", "options"…).
    pub windows: Vec<String>,
    pub scale: f32,
}

/// The kind of report.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A native crash (access violation, signal, Mach exception).
    Crash,
    /// A Rust panic.
    Panic,
    /// The window stopped answering.
    Freeze,
    /// The app ended without saying so and without a crash being caught.
    Exit,
}

impl Kind {
    /// The word used in file names and the issue form.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Crash => "crash",
            Kind::Panic => "panic",
            Kind::Freeze => "freeze",
            Kind::Exit => "exit",
        }
    }
}

/// Everything the monitor learnt about one session, for the report.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Notes {
    pub session: Option<Session>,
    pub panic: Option<Panic>,
    pub freeze: Option<Freeze>,
    pub state: Option<AppState>,
    pub recovered: bool,
    pub clean: bool,
    /// A freeze was announced and its dump hasn't arrived yet: every freeze is followed by its
    /// own dump, however long the window took to answer meanwhile (#420).
    pub freeze_dump_due: bool,
}

impl Notes {
    /// Take in one message from the app; unknown kinds and bad bodies are ignored (an older or
    /// newer app may send others).
    pub fn take(&mut self, kind: u32, body: &[u8]) {
        match kind {
            kind::SESSION => self.session = serde_json::from_slice(body).ok(),
            kind::PANIC => self.panic = serde_json::from_slice(body).ok(),
            kind::FREEZE => {
                self.freeze = serde_json::from_slice(body).ok();
                self.recovered = false;
                self.freeze_dump_due = true;
            }
            kind::RECOVERED => self.recovered = true,
            kind::CLEAN => self.clean = true,
            kind::STATE => self.state = serde_json::from_slice(body).ok(),
            _ => {}
        }
    }

    /// The kind of a dump that just arrived: a panic or a freeze said so first (a freeze's dump
    /// is the freeze's even when the window recovered before the dump was taken, #420: it was
    /// reported as a crash in the watchdog then); else a native crash.
    pub fn dump_kind(&self) -> Kind {
        if self.panic.is_some() {
            Kind::Panic
        } else if self.freeze_dump_due || (self.freeze.is_some() && !self.recovered) {
            Kind::Freeze
        } else {
            Kind::Crash
        }
    }

    /// A dump was written, or failed: a freeze's dump isn't owed any more.
    pub fn dump_done(&mut self) {
        self.freeze_dump_due = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #420: a freeze's dump is the freeze's even if the window recovered before the dump was
    /// taken; only a later dump counts as a crash.
    #[test]
    fn a_freeze_dump_taken_after_recovery_is_still_the_freeze() {
        let mut n = Notes::default();
        n.take(kind::FREEZE, br#"{"ui_thread":7,"seconds":8}"#);
        n.take(kind::RECOVERED, b"");
        assert_eq!(n.dump_kind(), Kind::Freeze);
        n.dump_done();
        assert_eq!(n.dump_kind(), Kind::Crash, "a later dump is a real crash");
    }

    #[test]
    fn notes_follow_the_messages() {
        let mut n = Notes::default();
        n.take(
            kind::STATE,
            br#"{"downloads":3,"running":1,"queues":1,"windows":["main"],"scale":1.5}"#,
        );
        assert_eq!(n.state.as_ref().unwrap().downloads, 3);
        assert_eq!(n.dump_kind(), Kind::Crash);
        n.take(kind::FREEZE, br#"{"ui_thread":7,"seconds":9}"#);
        assert_eq!(n.dump_kind(), Kind::Freeze);
        n.take(kind::RECOVERED, b"");
        // Its dump is still due: it's the freeze's (see the test below).
        assert_eq!(n.dump_kind(), Kind::Freeze);
        n.dump_done();
        assert_eq!(n.dump_kind(), Kind::Crash);
        n.take(
            kind::PANIC,
            br#"{"message":"boom","location":"a.rs:1:2","thread":"main","backtrace":""}"#,
        );
        assert_eq!(n.dump_kind(), Kind::Panic);
        // Unknown kinds and broken bodies change nothing.
        n.take(99, b"{}");
        n.take(kind::SESSION, b"not json");
        assert!(n.session.is_none());
    }
}
