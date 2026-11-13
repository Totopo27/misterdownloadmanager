//! The text of a report, in the manner of Minecraft's: what happened, the stack of the thread that
//! crashed or froze, what MDM was doing, the system, then every other thread and the modules.
//! English on purpose: it is read by whoever fixes the bug, whatever language MDM speaks.
//!
//! Nothing personal on purpose: the home folder shows as `~`, and MDM's state is counts only (no
//! addresses or file names). A panic's message and the log can hold text the user typed; the
//! report window shows all of it before anything is sent.

use std::fmt::Write as _;
use std::path::Path;

use crate::analyze::{Analysis, Frame, Thread};
use crate::protocol::{Kind, Notes};

/// Frames of a thread shown in full; other threads show this many.
const OTHER_THREAD_FRAMES: usize = 12;

/// Everything a report is made from.
#[derive(Clone, Debug, Default)]
pub struct Input<'a> {
    pub kind: Option<Kind>,
    /// When it happened (RFC 3339, UTC).
    pub time: String,
    pub notes: Notes,
    pub analysis: Option<&'a Analysis>,
    pub log_tail: Vec<String>,
    /// The dump's file name, when there is one.
    pub dump: Option<String>,
}

/// One line that changes with each report (Minecraft's has its jokes; ours stay kind).
const SORRY: &[&str] = &[
    "Sorry about that.",
    "That wasn't supposed to happen.",
    "Even download managers have bad days.",
    "This one is on us.",
    "Thank you for sending this; it helps.",
];

pub fn text(input: &Input, home: Option<&Path>) -> String {
    let kind = input.kind.unwrap_or(Kind::Crash);
    let mut s = String::new();
    let _ = writeln!(s, "---- Mister Download Manager crash report ----");
    let n = input.time.bytes().map(usize::from).sum::<usize>();
    let _ = writeln!(s, "// {}", SORRY[n % SORRY.len()]);
    let _ = writeln!(s);
    let _ = writeln!(s, "Time: {}", input.time);
    let _ = writeln!(s, "Description: {}", description(kind, &input.notes));
    let _ = writeln!(s, "Signature: {}", signature(kind, input));
    if let Some(d) = &input.dump {
        let _ = writeln!(s, "Dump: {d}");
    }
    // What draws, and why, at the top: a freeze or a crash inside a graphics driver is read from
    // it, and the end of a long report is what a size limit cuts (#280).
    if let Some(r) = input.notes.session.as_ref().map(|ss| &ss.renderer)
        && !r.is_empty()
    {
        let _ = writeln!(s, "Drawing with: {r}");
    }
    let _ = writeln!(s);

    // What went wrong.
    match kind {
        Kind::Panic => {
            if let Some(p) = &input.notes.panic {
                let _ = writeln!(s, "{}", p.message);
                let _ = writeln!(s, "  at {}", p.location);
                let _ = writeln!(s, "  thread: {}", p.thread);
            }
        }
        Kind::Freeze => {
            match &input.notes.freeze {
                Some(f) if f.seconds > 0 && over_before_snapshot(kind, input) => {
                    let ran = f
                        .busy
                        .map(|b| {
                            format!(
                                " Its thread ran {:.1} of {:.1} s while it did not answer.",
                                b.cpu_ms as f64 / 1000.0,
                                b.wall_ms as f64 / 1000.0
                            )
                        })
                        .unwrap_or_default();
                    let back = if key_thread(kind, input)
                        .is_some_and(|t| in_sleep_after_a_frame(&t.frames))
                    {
                        "done with that frame"
                    } else {
                        "back to waiting for events"
                    };
                    let _ = writeln!(
                        s,
                        "The window did not answer for {} s, but it was {back} when this snapshot was taken, so the stacks below don't show what held it.{ran}",
                        f.seconds
                    );
                }
                Some(f) if f.seconds > 0 => {
                    let top = key_thread(kind, input).and_then(|t| t.frames.first());
                    let secs = |ms: u64| format!("{:.1}", ms as f64 / 1000.0);
                    let why = match (stuck(f, top, main_module(input)), f.busy) {
                        (Some(Stuck::Running), Some(b)) => format!(
                            ". Its thread ran all along ({} of {} s): an endless or very long loop.",
                            secs(b.cpu_ms),
                            secs(b.wall_ms)
                        ),
                        (Some(Stuck::Waiting), Some(b)) => format!(
                            ". Its thread was waiting (it ran {} of {} s): a deadlock, or a wait on the disk or the network.",
                            secs(b.cpu_ms),
                            secs(b.wall_ms)
                        ),
                        (Some(Stuck::Starved), Some(b)) => format!(
                            ". Its thread ran only {} of {} s, and not in a wait: the computer was too busy to run MDM, or slow to load it from disk{}. Most likely not a bug in MDM.",
                            secs(b.cpu_ms),
                            secs(b.wall_ms),
                            b.faults
                                .map(|n| format!(" ({n} page faults meanwhile)"))
                                .unwrap_or_default()
                        ),
                        (_, Some(b)) => format!(
                            ". Its thread ran {} of {} s.",
                            secs(b.cpu_ms),
                            secs(b.wall_ms)
                        ),
                        (_, None) => " (a deadlock or an endless loop).".to_owned(),
                    };
                    let _ = writeln!(
                        s,
                        "The window did not answer for {} s: one frame never finished{why}",
                        f.seconds
                    );
                }
                _ => {
                    let _ = writeln!(
                        s,
                        "A dump asked for while MDM was running (a freeze or a panic): the window thread first."
                    );
                }
            }
            // Where the frame was when the watchdog saw the freeze: what held it, when the
            // snapshot came too late to show it (#574).
            if let Some(f) = input.notes.freeze.as_ref().filter(|f| !f.step.is_empty()) {
                let _ = writeln!(
                    s,
                    "When it stopped answering, the window was at: {}.",
                    f.step
                );
            }
            if input.notes.recovered {
                let _ = writeln!(s, "It answered again later.");
            }
        }
        Kind::Crash => {
            if let Some(a) = input.analysis {
                let _ = writeln!(
                    s,
                    "{} at {:#x}",
                    a.reason.as_deref().unwrap_or("Crash"),
                    a.address.unwrap_or(0)
                );
            }
        }
        Kind::Exit => {
            let _ = writeln!(
                s,
                "MDM ended without closing normally, and no crash was caught (it may have been ended from outside, or the system stopped it)."
            );
        }
    }
    let _ = writeln!(s);

    // The thread that matters.
    let key = key_thread(kind, input);
    if let Some(t) = key {
        let what = if kind == Kind::Freeze {
            "Window thread (frozen)"
        } else {
            "Thread that crashed"
        };
        let _ = writeln!(s, "-- {what}: {} --", thread_title(t));
        // A panic's from where it began (the panic hook and the dump request above it are ours).
        let from = if kind == Kind::Panic {
            panic_entry(&t.frames)
        } else {
            0
        };
        write_frames(&mut s, &t.frames[from..], usize::MAX);
        let _ = writeln!(s);
    } else if let Some(p) = &input.notes.panic
        && !p.backtrace.trim().is_empty()
    {
        let _ = writeln!(s, "-- Rust backtrace --");
        let _ = writeln!(s, "{}", p.backtrace.trim_end());
        let _ = writeln!(s);
    }

    // What MDM was doing.
    if !input.log_tail.is_empty() {
        let _ = writeln!(s, "-- What MDM was doing (its log) --");
        for l in &input.log_tail {
            let _ = writeln!(s, "{l}");
        }
        let _ = writeln!(s);
    }

    // The system.
    let _ = writeln!(s, "-- System details --");
    if let Some(ss) = &input.notes.session {
        let _ = writeln!(s, "MDM: {} ({})", ss.version, ss.commit);
        let _ = writeln!(s, "Installed with: {}", ss.install);
        let _ = writeln!(s, "System: {} ({})", ss.os_version, ss.arch);
        let _ = writeln!(s, "Language: {}", ss.language);
        let _ = writeln!(s, "Theme: {}", if ss.dark { "dark" } else { "light" });
        let _ = writeln!(s, "Program: {}", ss.exe);
        if let Ok(now) = chrono::DateTime::parse_from_rfc3339(&input.time) {
            let up = now.timestamp() - ss.started;
            if up >= 0 {
                let _ = writeln!(s, "Running for: {}", duration(up as u64));
            }
        }
    }
    if let Some(a) = input.analysis {
        let _ = writeln!(
            s,
            "CPU: {} {} ({} cores)",
            a.cpu,
            a.cpu_info.as_deref().unwrap_or(""),
            a.cpu_count
        );
        let _ = writeln!(
            s,
            "OS (dump): {} {}",
            a.os,
            a.os_version.as_deref().unwrap_or("")
        );
    }
    if let Some(st) = &input.notes.state {
        let _ = writeln!(
            s,
            "Downloads: {} ({} running), queues: {}",
            st.downloads, st.running, st.queues
        );
        let _ = writeln!(s, "Windows open: {}", st.windows.join(", "));
        let _ = writeln!(s, "Display scale: {}", st.scale);
    }
    if let Some(a) = input.analysis
        && !a.missing_symbols.is_empty()
    {
        let _ = writeln!(s, "No symbols for: {}", a.missing_symbols.join(", "));
    }
    let _ = writeln!(s);

    // Every other thread (threads waiting in the same place, as a pool's idle workers do, once
    // with all their names), then the modules.
    if let Some(a) = input.analysis {
        let mut groups: Vec<(Vec<&Thread>, String)> = Vec::new();
        for t in &a.threads {
            if key.is_some_and(|k| k.id == t.id) {
                continue;
            }
            let shape: String = t
                .frames
                .iter()
                .take(OTHER_THREAD_FRAMES)
                .map(|f| f.function.as_deref().unwrap_or("?"))
                .collect::<Vec<_>>()
                .join("|");
            match groups.iter_mut().find(|g| g.1 == shape) {
                Some(g) => g.0.push(t),
                None => groups.push((vec![t], shape)),
            }
        }
        for (threads, _) in &groups {
            let names: Vec<String> = threads.iter().map(|t| thread_title(t)).collect();
            if names.len() == 1 {
                let _ = writeln!(s, "-- Thread {} --", names[0]);
            } else {
                let _ = writeln!(s, "-- {} threads: {} --", names.len(), names.join(", "));
            }
            write_frames(&mut s, &threads[0].frames, OTHER_THREAD_FRAMES);
            let _ = writeln!(s);
        }
        let _ = writeln!(s, "-- Modules --");
        for m in &a.modules {
            let _ = writeln!(
                s,
                "{:#014x}  {:<32} {:<16} {}",
                m.base,
                m.name,
                m.version.as_deref().unwrap_or(""),
                m.debug_id.as_deref().unwrap_or("")
            );
        }
    }
    scrub(&s, home)
}

/// A report made and saved.
#[derive(Clone, Debug)]
pub struct Made {
    pub kind: Kind,
    pub text: String,
    pub signature: String,
    /// The `.txt` beside the dump (or alone, with no dump).
    pub path: std::path::PathBuf,
    pub dump: Option<std::path::PathBuf>,
    pub analysis: Option<Analysis>,
    pub notes: Notes,
}

impl Made {
    /// A freeze's window thread, as its dump has it (the app reads where it was stuck: #847).
    pub fn frozen_window_thread(&self) -> Option<&Thread> {
        if self.kind != Kind::Freeze {
            return None;
        }
        window_thread(self.analysis.as_ref()?, self.notes.freeze.as_ref()?)
    }
}

/// Read `dump` (none for an exit nothing caught), write the report beside it in `data`'s crashes
/// folder, and return it. `symbols`: Breakpad symbol stores to try (the release's, once fetched).
pub fn make(
    dump: Option<&Path>,
    kind: Kind,
    notes: Notes,
    data: &Path,
    symbols: &[std::path::PathBuf],
) -> Made {
    let analysis = dump.and_then(|d| match crate::analyze::analyze(d, symbols) {
        Ok(a) => Some(a),
        Err(e) => {
            tracing::warn!("crash report: the dump couldn't be read: {e}");
            None
        }
    });
    let input = Input {
        kind: Some(kind),
        time: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        notes: notes.clone(),
        analysis: analysis.as_ref(),
        log_tail: crate::logs::tail(data, 150),
        dump: dump
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().into_owned()),
    };
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(std::path::PathBuf::from);
    let text = text(&input, home.as_deref());
    let signature = signature(kind, &input);
    let path = match dump {
        Some(d) => d.with_extension("txt"),
        None => {
            crate::monitor::crashes_dir(data).join(format!("{}.txt", crate::monitor::stem(kind)))
        }
    };
    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let _ = std::fs::write(&path, &text);
    Made {
        kind,
        text,
        signature,
        path,
        dump: dump.map(Path::to_path_buf),
        analysis,
        notes,
    }
}

/// The kind in a sentence.
pub fn description(kind: Kind, notes: &Notes) -> &'static str {
    match kind {
        Kind::Crash => "MDM crashed",
        Kind::Panic => "MDM hit an internal error and closed",
        Kind::Freeze if notes.recovered => "The window stopped responding (it recovered)",
        Kind::Freeze => "The window stopped responding",
        Kind::Exit => "MDM closed unexpectedly",
    }
}

/// A short, stable name for the problem, the same for every report of it (it finds the issue
/// already filed for it): the kind and the first of MDM's own functions on the stack that
/// matters ("freeze in misterdownloadmanager::winui::combo_edit").
pub fn signature(kind: Kind, input: &Input) -> String {
    let main_module = main_module(input);
    // The freeze ended before the snapshot: its stack says nothing about it, and without this
    // every such report was filed under the start-up function that runs the event loop (#355).
    if over_before_snapshot(kind, input) {
        return format!("{} over before the snapshot", kind.word());
    }
    // A computer too busy to run MDM: one name for all of them, wherever the thread happened to be.
    if let Some(f) = input.notes.freeze.as_ref().filter(|_| kind == Kind::Freeze) {
        let top = key_thread(kind, input).and_then(|t| t.frames.first());
        if stuck(f, top, main_module) == Some(Stuck::Starved) {
            return format!("{} while the computer was busy", kind.word());
        }
    }
    let ours = key_thread(kind, input).and_then(|t| {
        // A panic's stack starts in the panic hook (the reporter asking for the dump): what
        // panicked is below the last frame of Rust's panic machinery.
        let below = if kind == Kind::Panic {
            let e = panic_entry(&t.frames);
            (e..t.frames.len())
                .find(|&i| {
                    !t.frames[i]
                        .function
                        .as_deref()
                        .map(trim_function)
                        .is_some_and(|n| n.starts_with("std::") || n.starts_with("core::"))
                })
                .unwrap_or(e)
        } else {
            0
        };
        first_own_frame(&t.frames[below.min(t.frames.len())..], main_module)
    });
    let place = match (ours, &input.notes.panic) {
        (Some(f), _) => f,
        // No symbols: a panic still says where.
        (None, Some(p)) => strip_column(&p.location),
        (None, None) => key_thread(kind, input)
            .and_then(|t| t.frames.first())
            .map(|f| format!("{}+{:#x}", f.module.as_deref().unwrap_or("?"), f.offset))
            .unwrap_or_else(|| "unknown".to_owned()),
    };
    format!("{} in {place}", kind.word())
}

/// The program's own module (the dump's first).
fn main_module<'a>(input: &Input<'a>) -> Option<&'a str> {
    input
        .analysis
        .and_then(|a| a.modules.first())
        .map(|m| m.name.as_str())
}

/// What a frozen window thread was doing, judged from the processor time it got and its top frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stuck {
    /// It ran nearly all along: a loop that doesn't end, or a very long one.
    Running,
    /// It hardly ran and sits in a wait: a deadlock, or a wait on the disk or the network.
    Waiting,
    /// It hardly ran, yet sits in ordinary code: the computer didn't give it time, or was loading
    /// MDM's code from disk (a page fault stops a thread wherever it is).
    Starved,
}

/// Its share of the processor at least this: running. At most `STARVED_SHARE`, outside a wait:
/// starved. In between, or with too little measured, no verdict.
const RUNNING_SHARE: f64 = 0.6;
const STARVED_SHARE: f64 = 0.3;

pub fn stuck(
    f: &crate::protocol::Freeze,
    top: Option<&Frame>,
    main: Option<&str>,
) -> Option<Stuck> {
    let b = f.busy?;
    if b.wall_ms < 1000 {
        return None;
    }
    let share = b.cpu_ms as f64 / b.wall_ms as f64;
    if share >= RUNNING_SHARE {
        return Some(Stuck::Running);
    }
    let top = top?;
    match top.function.as_deref() {
        Some(n) if is_wait(n) => Some(Stuck::Waiting),
        Some(_) => (share <= STARVED_SHARE).then_some(Stuck::Starved),
        // Unnamed (no symbols): MDM's own module is ordinary code; a system module where the
        // thread hardly ran is almost always a wait (the system's waits live there).
        None if share > STARVED_SHARE => None,
        None if main.is_some() && top.module.as_deref() == main => Some(Stuck::Starved),
        // The C runtime's copies and compares (memcmp of an icon's first bytes, #663): never a
        // wait, so a thread that hardly ran there waited for its memory, loaded from disk.
        None if top.module.as_deref().is_some_and(is_memory_routines) => Some(Stuck::Starved),
        None => top.module.is_some().then_some(Stuck::Waiting),
    }
}

/// A module of the C runtime's memory and string routines only (memcpy, memcmp, memset): no
/// wait lives there. Windows' VCRUNTIME140.dll and VCRUNTIME140_1.dll; not ucrtbase.dll, nor
/// Linux's libc, which hold the waits as well.
fn is_memory_routines(module: &str) -> bool {
    module.to_ascii_lowercase().starts_with("vcruntime")
}

/// A function a thread waits in: the system's waits and sleeps, locks, I/O.
fn is_wait(function: &str) -> bool {
    const WAITS: &[&str] = &[
        "Wait",
        "wait",
        "Sleep",
        "sleep",
        "Delay",
        "futex",
        "psynch",
        "poll",
        "select",
        "Recv",
        "recv",
        "NtRead",
        "IoCompletion",
        "accept",
        "semaphore",
        "park",
        "lock_slow",
        "syscall",
        "mach_msg",
    ];
    WAITS.iter().any(|w| function.contains(w))
}

/// Where a panic begins on its thread's stack (core::panicking::panic_fmt and the like): what is
/// above it is the panic hook and the reporter asking for the dump. Not the last such frame:
/// every stack ends in start-up's std::panicking::catch_unwind. 0 when none is found.
fn panic_entry(frames: &[Frame]) -> usize {
    frames
        .iter()
        .position(|f| {
            f.function.as_deref().map(trim_function).is_some_and(|n| {
                n.starts_with("core::panicking")
                    || n.starts_with("std::panicking::begin_panic")
                    || n.contains("rust_begin_unwind")
            })
        })
        .unwrap_or(0)
}

/// The thread a report is about: the window thread for a freeze, else the one the dump was
/// taken for.
fn key_thread<'a>(kind: Kind, input: &Input<'a>) -> Option<&'a Thread> {
    let a = input.analysis?;
    if kind == Kind::Freeze {
        return window_thread(a, input.notes.freeze.as_ref()?);
    }
    a.thread(a.requesting?)
}

/// A freeze's window thread in its dump: by its id, or the first when it is the main thread.
fn window_thread<'a>(a: &'a Analysis, f: &crate::protocol::Freeze) -> Option<&'a Thread> {
    a.thread(f.ui_thread)
        .filter(|_| f.ui_thread != 0)
        .or_else(|| f.main.then(|| a.threads.first()).flatten())
}

/// A freeze whose window thread was already back in the event loop's wait (or in eframe's pause
/// after a frame) when the dump was taken: a frame in progress never has that on its stack, so the
/// frame had ended. The dump follows the watchdog's request, and came 7 s after it on a stalled
/// computer (#355). winit's wait, on Windows; other systems' stacks keep the verdicts below.
fn over_before_snapshot(kind: Kind, input: &Input) -> bool {
    kind == Kind::Freeze
        && input.notes.freeze.is_some()
        && key_thread(kind, input).is_some_and(|t| {
            t.frames.iter().any(|f| {
                f.function
                    .as_deref()
                    .is_some_and(|n| n.contains("wait_for_messages"))
            }) || in_sleep_after_a_frame(&t.frames)
        })
}

/// The window thread in eframe's pause after a frame of a hidden or minimized window
/// (`sleep_if_invisible_or_minimized`, 10 ms, which release builds fold into eframe's
/// `run_ui_and_paint`): it comes after the app's logic and UI have returned, so the frame had
/// ended. A window hidden in the notification area passes there after each of its ticks; two
/// freezes reported just after the computer woke had their dump taken there, 8 and 15 s after
/// they were seen (#574, #613), and their stacks said nothing of what held the window.
fn in_sleep_after_a_frame(frames: &[Frame]) -> bool {
    // Rust's own frames: the system's below them (ntdll, libc's nanosleep) carry no path.
    let mut named = frames
        .iter()
        .filter_map(|f| f.function.as_deref())
        .map(trim_function)
        .filter(|n| n.contains("::"));
    let Some(sleep) = named.next() else {
        return false;
    };
    if !(sleep.starts_with("std::thread") && sleep.ends_with("sleep")) {
        return false;
    }
    named.next().is_some_and(|caller| {
        caller.starts_with("eframe::native::")
            && (caller.ends_with("sleep_if_invisible_or_minimized")
                || caller.ends_with("run_ui_and_paint"))
    })
}

/// The first frame in the program's own code, past the system, Rust's standard library, locks,
/// panics and the crash reporter itself: a function of the program's own crates first (named as
/// its main module, the dump's first: "misterdownloadmanager", or MDM's `mdm_*` crates), else
/// of any crate linked into it (egui, winit…).
fn first_own_frame(frames: &[Frame], main_module: Option<&str>) -> Option<String> {
    let own_crate = main_module
        .map(|m| m.split('.').next().unwrap_or(m).replace('-', "_"))
        .unwrap_or_else(|| "misterdownloadmanager".to_owned());
    let paths: Vec<String> = frames
        .iter()
        .filter(|f| main_module.is_none_or(|m| f.module.as_deref() == Some(m)))
        .filter_map(|f| f.function.as_deref())
        .map(trim_function)
        .filter(|p| is_code(p))
        .collect();
    let crate_of = |p: &str| p.split("::").next().unwrap_or("").to_owned();
    paths
        .iter()
        .find(|p| {
            let c = crate_of(p);
            c == own_crate || (c.starts_with("mdm_") && c != "mdm_crash")
        })
        .or_else(|| paths.first())
        .cloned()
}

/// A Rust function path outside the machinery (not a C symbol, not std, locks or panics).
fn is_code(path: &str) -> bool {
    let machinery = [
        "std::",
        "core::",
        "alloc::",
        "parking_lot",
        "lock_api",
        "mdm_crash::",
        "crash_handler",
        "minidumper",
        "backtrace",
        "__rust",
        "rust_begin",
        "rust_panic",
    ];
    path.contains("::") && !machinery.iter().any(|m| path.starts_with(m))
}

/// A function name without its generic arguments and closure numbers, so the same place reads
/// the same in every build.
fn trim_function(f: &str) -> String {
    // Its parameters too ("crashy::frozen(std::sync::Mutex<…>*)": Windows' symbols carry them).
    let f = f.split('(').next().unwrap_or(f);
    let f = f.trim_start_matches(['<', '&']);
    let mut out = String::new();
    let mut depth = 0;
    for c in f.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split("::{{closure}}")
        .next()
        .unwrap_or(&out)
        .trim_end_matches("::")
        .to_owned()
}

fn strip_column(location: &str) -> String {
    // "src/a.rs:12:5" → "src/a.rs:12"
    match location.rsplit_once(':') {
        Some((head, col)) if col.chars().all(|c| c.is_ascii_digit()) && head.contains(':') => {
            head.to_owned()
        }
        _ => location.to_owned(),
    }
}

fn thread_title(t: &Thread) -> String {
    match &t.name {
        Some(n) => format!("{n} ({})", t.id),
        None => t.id.to_string(),
    }
}

fn write_frames(s: &mut String, frames: &[Frame], max: usize) {
    for (i, f) in frames.iter().take(max).enumerate() {
        let module = f.module.as_deref().unwrap_or("?");
        match &f.function {
            Some(func) => {
                let _ = write!(s, "{i:>3}: {func}");
                if let Some(file) = &f.file {
                    let _ = write!(
                        s,
                        "\n       at {}:{}",
                        short_path(file),
                        f.line.unwrap_or(0)
                    );
                }
                let _ = writeln!(s);
            }
            None => {
                let _ = writeln!(s, "{i:>3}: {module} + {:#x}", f.offset);
            }
        }
    }
    if frames.len() > max {
        let _ = writeln!(s, "     … {} more", frames.len() - max);
    }
}

/// A source path from where it means something: `crates/app/src/winui.rs` rather than the build
/// machine's checkout, `library/std/src/…` for the standard library, a crate's own path under
/// the cargo registry.
fn short_path(p: &str) -> String {
    let p = p.replace('\\', "/");
    for mark in ["/crates/", "/library/", "/registry/src/", "/git/checkouts/"] {
        if let Some(i) = p.find(mark) {
            return p[i + 1..].to_owned();
        }
    }
    p
}

fn duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min {} s", s / 60, s % 60),
        s => format!("{} h {} min", s / 3600, s % 3600 / 60),
    }
}

/// The home folder as `~` (in paths of either kind of separator).
pub fn scrub(text: &str, home: Option<&Path>) -> String {
    let Some(home) = home.map(|h| h.display().to_string()) else {
        return text.to_owned();
    };
    if home.len() < 3 {
        return text.to_owned();
    }
    let mut out = text.replace(&home, "~");
    let flipped = home.replace('\\', "/");
    if flipped != home {
        out = out.replace(&flipped, "~");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Freeze, Panic, Session};

    fn frame(f: &str) -> Frame {
        Frame {
            function: Some(f.to_owned()),
            file: Some(r"D:\build\mdm\crates\app\src\winui.rs".to_owned()),
            line: Some(2088),
            module: Some("misterdownloadmanager.exe".to_owned()),
            offset: 0x1234,
        }
    }

    fn frozen() -> Analysis {
        Analysis {
            requesting: Some(2),
            threads: vec![
                Thread {
                    id: 1,
                    name: Some("main".into()),
                    frames: vec![
                        frame("parking_lot::raw_rwlock::RawRwLock::lock_exclusive_slow"),
                        frame("egui::context::Context::write<egui::memory::Memory>"),
                        frame("misterdownloadmanager::winui::combo_edit::{{closure}}"),
                        frame("misterdownloadmanager::dialogs::AddUrl::show"),
                    ],
                },
                Thread {
                    id: 2,
                    name: Some("watchdog".into()),
                    frames: vec![frame("mdm_crash::client::dump")],
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_freeze_names_the_window_threads_own_frame() {
        let a = frozen();
        let input = Input {
            kind: Some(Kind::Freeze),
            time: "2026-11-01T04:00:00+00:00".into(),
            notes: Notes {
                freeze: Some(Freeze {
                    ui_thread: 1,
                    main: false,
                    seconds: 9,
                    busy: None,
                    step: String::new(),
                }),
                session: Some(Session {
                    version: "0.2.3".into(),
                    exe: r"C:\Users\Someone\AppData\Local\MDM\mdm.exe".into(),
                    started: 1_793_505_000,
                    ..Default::default()
                }),
                ..Default::default()
            },
            analysis: Some(&a),
            ..Default::default()
        };
        assert_eq!(
            signature(Kind::Freeze, &input),
            "freeze in misterdownloadmanager::winui::combo_edit"
        );
        let t = text(&input, Some(Path::new(r"C:\Users\Someone")));
        assert!(t.contains("-- Window thread (frozen): main (1) --"));
        assert!(t.contains("at crates/app/src/winui.rs:2088"));
        assert!(t.contains(r"Program: ~\AppData\Local\MDM\mdm.exe"));
        assert!(!t.contains("Someone"));
        // The watchdog's thread is listed among the others.
        assert!(t.contains("-- Thread watchdog (2) --"));
    }

    /// What drew, and why, is in the report's head, before the stack and the log: a report cut
    /// to fit an issue still says it (#280, a freeze inside Intel's OpenGL driver).
    #[test]
    fn the_renderer_is_named_at_the_top() {
        let a = frozen();
        let drawing = "OpenGL on Intel(R) UHD Graphics 620 (4.6.0 - Build 31.0.101.2125)                        — Direct3D 12 failed here before: Direct3D 12 drew no first frame within 10 s";
        let input = Input {
            kind: Some(Kind::Freeze),
            time: "2026-11-07T15:13:03Z".into(),
            notes: Notes {
                session: Some(Session {
                    renderer: drawing.into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            analysis: Some(&a),
            log_tail: vec!["a log line".into()],
            dump: Some("x-freeze.dmp".into()),
        };
        let t = text(&input, None);
        let line = format!("Drawing with: {drawing}");
        let at = t.find(&line).expect("named");
        assert_eq!(t.matches(&line).count(), 1, "once");
        assert!(at < t.find("a log line").unwrap());
        assert!(at < t.find("-- System details --").unwrap());
        // Nothing to say when the window never started.
        let mut none = input.clone();
        none.notes.session = Some(Session::default());
        assert!(!text(&none, None).contains("Drawing with"));
    }

    fn freeze_with(a: &Analysis, cpu_ms: u64) -> Input<'_> {
        Input {
            kind: Some(Kind::Freeze),
            notes: Notes {
                freeze: Some(Freeze {
                    ui_thread: 1,
                    main: false,
                    seconds: 8,
                    busy: Some(crate::protocol::Busy {
                        wall_ms: 7_500,
                        cpu_ms,
                        faults: Some(12_000),
                    }),
                    step: String::new(),
                }),
                ..Default::default()
            },
            analysis: Some(a),
            ..Default::default()
        }
    }

    /// The window thread stopped in ordinary code of MDM's (#57: App::dialogs, 0 downloads).
    fn stopped_in_own_code() -> Analysis {
        let mut a = frozen();
        a.threads[0].frames = vec![
            frame("misterdownloadmanager::app::App::dialogs"),
            frame("misterdownloadmanager::app::impl$2::ui"),
        ];
        a
    }

    #[test]
    fn a_freeze_that_hardly_ran_in_own_code_is_a_busy_computer() {
        let a = stopped_in_own_code();
        let input = freeze_with(&a, 400);
        assert_eq!(
            signature(Kind::Freeze, &input),
            "freeze while the computer was busy"
        );
        let t = text(&input, None);
        assert!(t.contains("Its thread ran only 0.4 of 7.5 s, and not in a wait"));
        assert!(t.contains("(12000 page faults meanwhile)"));
        assert!(t.contains("Most likely not a bug in MDM."));
        // Without symbols, MDM's own module counts as ordinary code, a system one as a wait.
        let mut bare = stopped_in_own_code();
        bare.modules = vec![crate::analyze::ModuleInfo {
            name: "misterdownloadmanager.exe".into(),
            ..Default::default()
        }];
        bare.threads[0].frames[0].function = None;
        let f = freeze_with(&bare, 400).notes.freeze.unwrap();
        let main = Some("misterdownloadmanager.exe");
        assert_eq!(
            stuck(&f, bare.threads[0].frames.first(), main),
            Some(Stuck::Starved)
        );
        bare.threads[0].frames[0].module = Some("ntdll.dll".into());
        assert_eq!(
            stuck(&f, bare.threads[0].frames.first(), main),
            Some(Stuck::Waiting)
        );
        // #663: in the C runtime's memcmp, under MDM's own code: no wait there, so starved.
        bare.threads[0].frames[0].module = Some("VCRUNTIME140.dll".into());
        assert_eq!(
            stuck(&f, bare.threads[0].frames.first(), main),
            Some(Stuck::Starved)
        );
        assert_eq!(
            signature(Kind::Freeze, &freeze_with(&bare, 100)),
            "freeze while the computer was busy"
        );
    }

    /// #355, as reported: the dump was written 7 s after it was asked for, when the window thread
    /// was back in the event loop. Not a deadlock, not "in misterdownloadmanager::app".
    #[test]
    fn a_freeze_over_before_the_snapshot_says_so() {
        let mut a = frozen();
        a.threads[0].frames = vec![
            Frame {
                function: None,
                file: None,
                line: None,
                module: Some("win32u.dll".into()),
                offset: 0xa124,
            },
            frame(
                "winit::platform_impl::windows::event_loop::EventLoop::wait_for_messages<enum2$<eframe::native::winit_integration::UserEvent> >",
            ),
            frame("eframe::run_native"),
            frame("misterdownloadmanager::app"),
            frame("misterdownloadmanager::main"),
        ];
        let input = freeze_with(&a, 0);
        assert_eq!(
            signature(Kind::Freeze, &input),
            "freeze over before the snapshot"
        );
        let t = text(&input, None);
        assert!(
            t.contains("back to waiting for events when this snapshot was taken"),
            "{t}"
        );
        assert!(t.contains("Its thread ran 0.0 of 7.5 s"), "{t}");
        assert!(!t.contains("deadlock"), "{t}");
        assert!(!t.contains("one frame never finished"), "{t}");
    }

    /// #574 and #613, as reported (0.5.0 and 0.5.2, the window hidden in the notification area,
    /// just after the computer woke): the dump came when the window answered again, and found its
    /// thread in eframe's 10 ms pause after the frame. Not "a deadlock in
    /// misterdownloadmanager::app" nor "in ntdll.dll".
    #[test]
    fn a_freeze_whose_snapshot_found_the_pause_after_a_frame_says_so() {
        let system = |module: &str, offset: u64| Frame {
            function: None,
            file: None,
            line: None,
            module: Some(module.into()),
            offset,
        };
        let mut a = frozen();
        a.threads[0].frames = vec![
            system("ntdll.dll", 0x166464),
            system("KERNELBASE.dll", 0x1427e),
            frame("std::thread::functions::sleep()"),
            frame("eframe::native::wgpu_integration::impl$3::run_ui_and_paint()"),
            frame(
                "eframe::native::run::WinitAppWrapper::check_redraw_requests<eframe::native::wgpu_integration::WgpuWinitApp>()",
            ),
            frame(
                "eframe::native::run::impl$1::new_events<eframe::native::wgpu_integration::WgpuWinitApp>()",
            ),
            frame("eframe::run_native()"),
            frame("misterdownloadmanager::app()"),
            frame("misterdownloadmanager::main()"),
        ];
        let mut input = freeze_with(&a, 0);
        input.notes.freeze.as_mut().unwrap().step = "the tray icon".into();
        assert_eq!(
            signature(Kind::Freeze, &input),
            "freeze over before the snapshot"
        );
        let t = text(&input, None);
        assert!(
            t.contains("done with that frame when this snapshot was taken"),
            "{t}"
        );
        assert!(t.contains("the window was at: the tray icon."), "{t}");
        assert!(!t.contains("deadlock"), "{t}");

        // Linux's stack has libc's names below Rust's, and debug builds keep eframe's helper.
        a.threads[0].frames = vec![
            frame("clock_nanosleep"),
            frame("std::thread::sleep"),
            frame("eframe::native::winit_integration::sleep_if_invisible_or_minimized"),
            frame("eframe::native::glow_integration::GlowWinitRunning::run_ui_and_paint"),
        ];
        assert_eq!(
            signature(Kind::Freeze, &freeze_with(&a, 0)),
            "freeze over before the snapshot"
        );

        // A sleep of MDM's own, inside a frame, is the frame's: still a freeze in MDM.
        a.threads[0].frames = vec![
            frame("std::thread::functions::sleep()"),
            frame("misterdownloadmanager::app::App::tick()"),
            frame("eframe::native::wgpu_integration::impl$3::run_ui_and_paint()"),
        ];
        assert_eq!(
            signature(Kind::Freeze, &freeze_with(&a, 0)),
            "freeze in misterdownloadmanager::app::App::tick"
        );
    }

    #[test]
    fn a_freeze_in_a_lock_is_a_wait_and_keeps_its_place() {
        let a = frozen();
        let input = freeze_with(&a, 10);
        assert_eq!(
            signature(Kind::Freeze, &input),
            "freeze in misterdownloadmanager::winui::combo_edit"
        );
        assert!(
            text(&input, None).contains("Its thread was waiting (it ran 0.0 of 7.5 s): a deadlock")
        );
    }

    #[test]
    fn a_freeze_that_ran_all_along_is_a_loop() {
        let a = stopped_in_own_code();
        let input = freeze_with(&a, 7_300);
        assert_eq!(
            signature(Kind::Freeze, &input),
            "freeze in misterdownloadmanager::app::App::dialogs"
        );
        assert!(
            text(&input, None)
                .contains("ran all along (7.3 of 7.5 s): an endless or very long loop")
        );
        // In between: no verdict, just the numbers.
        let input = freeze_with(&a, 3_500);
        let t = text(&input, None);
        assert!(
            t.contains("finished. Its thread ran 3.5 of 7.5 s.\n"),
            "{t}"
        );
        // Not measured (an older MDM, macOS): the old sentence.
        let mut input = freeze_with(&a, 0);
        input.notes.freeze.as_mut().unwrap().busy = None;
        assert!(text(&input, None).contains("never finished (a deadlock or an endless loop)."));
    }

    #[test]
    fn a_panic_without_symbols_still_says_where() {
        let input = Input {
            kind: Some(Kind::Panic),
            notes: Notes {
                panic: Some(Panic {
                    message: "index out of bounds".into(),
                    location: "crates/app/src/list.rs:120:9".into(),
                    thread: "main".into(),
                    backtrace: String::new(),
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            signature(Kind::Panic, &input),
            "panic in crates/app/src/list.rs:120"
        );
    }

    #[test]
    fn function_names_lose_generics_and_closures() {
        assert_eq!(
            trim_function("misterdownloadmanager::winui::combo_edit::{{closure}}"),
            "misterdownloadmanager::winui::combo_edit"
        );
        assert_eq!(
            trim_function("mdm_engine::job::Job<alloc::string::String>::run"),
            "mdm_engine::job::Job::run"
        );
    }
}
