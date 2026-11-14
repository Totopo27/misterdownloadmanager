//! A real freeze, panic and crash, end to end on each system: examples/crashy.rs does it, its
//! monitor writes the minidump, reads it with the example's own debug info, and the report names
//! the function where it happened.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// crashy's program name.
const CRASHY: &str = if cfg!(windows) {
    "crashy.exe"
} else {
    "crashy"
};

/// examples/crashy, which `cargo test -p mdm-crash` builds with the tests, from `test`, this
/// test's program. Cargo copies it to the profile's folder (target/debug/examples), which holds
/// the test's program too: in deps/ up to Cargo 1.99, in build/mdm-crash/<hash>/out/ from Cargo
/// 1.100 (#934). With a build folder of its own (Cargo's `build-dir`) that copy is in the target
/// folder only; crashy's own program is then beside the test's, in
/// build/mdm-crash/<another hash>/out/ (the newest, if there are several).
fn find_crashy(test: &Path) -> Option<PathBuf> {
    let copied = test
        .ancestors()
        .map(|dir| dir.join("examples").join(CRASHY))
        .find(|p| p.is_file());
    copied.or_else(|| {
        let built = test.parent()?.parent()?.parent()?;
        std::fs::read_dir(built)
            .ok()?
            .flatten()
            .map(|d| d.path().join("out").join(CRASHY))
            .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
            .max()
            .map(|(_, p)| p)
    })
}

fn crashy() -> PathBuf {
    let test = std::env::current_exe().unwrap();
    find_crashy(&test).unwrap_or_else(|| {
        panic!(
            "examples/crashy is built with the tests (cargo test -p mdm-crash); none found from {}",
            test.display()
        )
    })
}

/// #934: crashy is found where each Cargo puts it, whichever Cargo runs this.
#[test]
fn crashy_is_found_in_each_layout() {
    let root = tempfile::tempdir().unwrap();
    let file = |rel: &str| {
        let p = root.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"").unwrap();
        p
    };
    let crashy = |dir: &str| root.path().join(dir).join(CRASHY);
    // Cargo 1.99 and before.
    let test = file("old/debug/deps/e2e-1a2b");
    file(&format!("old/debug/examples/{CRASHY}"));
    assert_eq!(find_crashy(&test), Some(crashy("old/debug/examples")));
    // Cargo 1.100: the copy in examples/ first.
    let test = file("new/debug/build/mdm-crash/1a2b/out/e2e-1a2b");
    file(&format!("new/debug/build/mdm-crash/3c4d/out/{CRASHY}"));
    file(&format!("new/debug/examples/{CRASHY}"));
    assert_eq!(find_crashy(&test), Some(crashy("new/debug/examples")));
    // A build folder of its own: the newest beside the test.
    let test = file("build/debug/build/mdm-crash/1a2b/out/e2e-1a2b");
    let old = file(&format!("build/debug/build/mdm-crash/5e6f/out/{CRASHY}"));
    let earlier = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(earlier)
        .unwrap();
    file(&format!("build/debug/build/mdm-crash/3c4d/out/{CRASHY}"));
    assert_eq!(
        find_crashy(&test),
        Some(crashy("build/debug/build/mdm-crash/3c4d/out"))
    );
    // None at all: said, not a wrong path.
    assert_eq!(find_crashy(&file("none/debug/deps/e2e-1a2b")), None);
}

/// Run crashy in `mode` and wait (up to 90 s: reading the dump's symbols takes a while in a
/// debug build) for its report.
fn report(mode: &str) -> String {
    let data = tempfile::tempdir().unwrap();
    /// The program, ended and reaped however the test leaves (a failed assert too).
    struct Running(std::process::Child);
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _child = Running(
        Command::new(crashy())
            .arg(mode)
            .arg(data.path())
            .spawn()
            .expect("examples/crashy is built with the tests"),
    );
    let crashes = data.path().join("crashes");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let txt = std::fs::read_dir(&crashes).ok().and_then(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|x| x == "txt"))
        });
        if let Some(t) = txt {
            // Written in one go after the dump was read; give the write a moment.
            std::thread::sleep(Duration::from_millis(300));
            return std::fs::read_to_string(&t).unwrap();
        }
        if Instant::now() > deadline {
            panic!("no report for {mode} in {}", crashes.display());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn a_freeze_reports_the_frozen_function() {
    let r = report("freeze");
    assert!(r.contains("The window stopped responding"), "{r}");
    assert!(r.contains("crashy_frozen_here"), "{r}");
    assert!(
        r.contains("Signature: freeze in crashy::crashy_frozen_here"),
        "{r}"
    );
}

#[test]
fn a_panic_reports_its_message_and_function() {
    let r = report("panic");
    assert!(r.contains("crashy: a test panic"), "{r}");
    assert!(
        r.contains("Signature: panic in crashy::crashy_panics_here"),
        "{r}"
    );
}

#[test]
fn a_native_crash_reports_where_it_crashed() {
    let r = report("crash");
    assert!(r.contains("Description: MDM crashed"), "{r}");
    assert!(
        r.contains("Signature: crash in crashy::crashy_crashes_here"),
        "{r}"
    );
}
