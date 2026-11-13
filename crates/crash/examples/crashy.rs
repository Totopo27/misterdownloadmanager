//! The end-to-end test's program (tests/e2e.rs): `crashy <freeze|panic|crash> <data dir>` starts
//! its monitor (itself with `--crash-monitor`), attaches the crash handler as MDM does, and then
//! freezes its main thread, panics, or crashes. The monitor writes the dump and the report
//! (`<data>/crashes/*.txt`) and exits when this process is gone.
//!
//! Test code: it may use `unsafe` (the crash handler, a null write, the thread id), MDM's own code
//! keeps that in win32.rs / unix.rs.
#![allow(unsafe_code)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mdm_crash::client::Reporter;
use mdm_crash::crash_handler::{CrashEventResult, CrashHandler, make_crash_event};
use mdm_crash::monitor::{self, Event};
use mdm_crash::protocol::{Freeze, Panic, Session};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(m) = mdm_crash::MonitorArgs::parse(&args) {
        run_monitor(m);
        return;
    }
    let mode = args[1].clone();
    let data = PathBuf::from(&args[2]);
    let exe = std::env::current_exe().unwrap();
    let reporter = Arc::new(Reporter::start(&exe, &data).expect("the monitor starts"));
    reporter.session(&Session {
        version: "test".into(),
        pid: std::process::id(),
        ..Default::default()
    });
    let r = reporter.clone();
    let handler = Arc::new(
        CrashHandler::attach(unsafe {
            make_crash_event(move |cc| CrashEventResult::Handled(r.dump(cc)))
        })
        .unwrap(),
    );
    #[cfg(target_os = "linux")]
    handler.set_ptracer(reporter.monitor_pid());

    match mode.as_str() {
        "freeze" => {
            let lock = Arc::new(Mutex::new(()));
            let ui = os_thread_id();
            let (l, h, rep) = (lock.clone(), handler.clone(), reporter.clone());
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("watchdog".into())
                .spawn(move || {
                    let _held = l.lock().unwrap();
                    tx.send(()).unwrap();
                    std::thread::sleep(Duration::from_millis(500));
                    rep.freeze(&Freeze {
                        ui_thread: ui,
                        main: true,
                        seconds: 1,
                        busy: None,
                        step: String::new(),
                    });
                    simulate(&h);
                    // The dump is written while this thread waits for the answer: done.
                    rep.clean();
                    std::thread::sleep(Duration::from_millis(200));
                    std::process::exit(0);
                })
                .unwrap();
            rx.recv().unwrap();
            crashy_frozen_here(&lock);
        }
        "panic" => {
            let (h, rep) = (handler.clone(), reporter.clone());
            std::panic::set_hook(Box::new(move |info| {
                rep.panic(&Panic {
                    message: info.payload_as_str().unwrap_or("").to_owned(),
                    location: info.location().map(|l| l.to_string()).unwrap_or_default(),
                    thread: std::thread::current().name().unwrap_or("?").to_owned(),
                    backtrace: std::backtrace::Backtrace::force_capture().to_string(),
                });
                simulate(&h);
            }));
            crashy_panics_here();
        }
        _ => crashy_crashes_here(),
    }
}

#[inline(never)]
fn crashy_frozen_here(lock: &Mutex<()>) {
    let _never = lock.lock();
    unreachable!();
}

#[inline(never)]
fn crashy_panics_here() {
    panic!("crashy: a test panic");
}

#[inline(never)]
fn crashy_crashes_here() {
    // A write through a null pointer: an access violation / SIGSEGV, caught natively.
    unsafe { std::ptr::write_volatile(std::ptr::null_mut::<u32>(), 1) };
}

fn simulate(h: &CrashHandler) {
    #[cfg(target_os = "linux")]
    h.simulate_signal(libc::SIGUSR2 as u32);
    #[cfg(any(windows, target_os = "macos"))]
    h.simulate_exception(None);
}

#[cfg(windows)]
fn os_thread_id() -> u32 {
    unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
}

#[cfg(target_os = "linux")]
fn os_thread_id() -> u32 {
    unsafe { libc::gettid() as u32 }
}

#[cfg(target_os = "macos")]
fn os_thread_id() -> u32 {
    0
}

fn run_monitor(m: mdm_crash::MonitorArgs) {
    let (tx, rx) = std::sync::mpsc::channel();
    let (socket, data) = (m.socket.clone(), m.data.clone());
    let failed = tx.clone();
    std::thread::spawn(move || {
        if let Err(e) = monitor::serve(&socket, &data, tx) {
            let _ = failed.send(Event::ServeFailed(e));
        }
    });
    while let Ok(e) = rx.recv() {
        match e {
            Event::Dump { path, kind, notes } => {
                // The example's own debug info is beside it (no .sym store needed).
                mdm_crash::report::make(Some(&path), kind, notes, &m.data, &[]);
            }
            Event::DumpFailed { error, .. } => eprintln!("monitor: dump failed: {error}"),
            Event::Recovered => {}
            Event::ServeFailed(e) => {
                eprintln!("monitor: {e}");
                break;
            }
            Event::Gone { .. } => break,
        }
    }
}
