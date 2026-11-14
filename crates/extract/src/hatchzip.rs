//! HatchZip (hatchzip.dev, MIT or Apache-2.0), run as its own program like 7-Zip: an optional
//! extractor, used only when Options asks for it. It reads ZIP, 7z, RAR and tar, and extracts
//! *in place*: it cuts each finished part off the archive as it goes, so a large download needs
//! free space for its files only, not for the archive and its files at once.
//!
//! Only its documented command line is used (`hatchzip extract --output=<folder> -y -q --
//! <archive>`, the `HATCHZIP_PASSWORD` variable and the exit status of its README): 0 done,
//! 1 failed (progress saved), 3 not enough free space (nothing changed), 4 damaged (nothing
//! deleted), 5 password missing or wrong, 130 paused.
//!
//! In place, what is already extracted is no longer in the archive. So it is never extracted
//! into a staging folder that a failure would remove: it goes into a folder of its own beside the
//! archive ([`work_folder`]), which stays when HatchZip stops partway. Running it again with the
//! same folder continues where it stopped (HatchZip keeps its progress next to the archive).
//!
//! MDM continues only a run it started itself: it writes a record of it in its own data
//! ([`Runs`]) before HatchZip starts. HatchZip's progress file beside an archive (one that came
//! with a download or a torrent, say) is never enough.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::Error;

/// HatchZip's command line program on the PATH (its installers and packages put `hatchzip`
/// there; the desktop app alone, `hatchzip-gui`, isn't enough).
pub fn find_hatchzip() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "hatchzip.exe"
    } else {
        "hatchzip"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// The folder an extraction in place goes into before it is put where the settings say:
/// `<archive's name>.hatchzip-out` beside the archive (numbered when that name is taken). The
/// record of the run keeps it, so a second run continues the first in the same folder.
pub fn work_folder(dest: &Path, base: &str) -> PathBuf {
    let name = mdm_engine::naming::sanitize(base);
    let name = if name.is_empty() {
        "extracted".to_owned()
    } else {
        name
    };
    let folder = dest.join(format!("{name}.hatchzip-out"));
    if std::fs::symlink_metadata(&folder).is_ok() {
        crate::safe::numbered(&folder)
    } else {
        folder
    }
}

/// Whether HatchZip's error says it can't extract this archive in place, only while keeping it
/// (a solid RAR), which it says before it touches anything: "… Run the command again with
/// --keep-archive …" (HatchZip 0.1.10's own words). Its sentence, not any mention of the flag
/// (an archive's name can hold it); the caller also checks that nothing was changed.
pub fn not_in_place(error: &str) -> bool {
    error.contains("Run the command again with --keep-archive")
}

/// Whether HatchZip's progress file is beside `archive` (an unfinished extraction, whoever
/// started it).
pub fn progress(archive: &Path) -> bool {
    let mut p = archive.as_os_str().to_owned();
    p.push(".hatchzip");
    std::fs::symlink_metadata(Path::new(&p)).is_ok()
}

/// A path as run records are named and compared: as it is, or without regard to case where
/// file names ignore it (Windows: `D:\Dl\A.zip` and `d:\dl\a.zip` are one archive).
fn path_key(path: &Path, ignore_case: bool) -> Vec<u8> {
    if ignore_case {
        path.to_string_lossy().to_lowercase().into_bytes()
    } else {
        path.as_os_str().as_encoded_bytes().to_vec()
    }
}

/// The extractions in place MDM started, one small file each in a folder of MDM's own data:
/// the archive's full path and the folder its files go into. Written before HatchZip starts,
/// removed once its progress file is gone (finished, or nothing changed).
pub struct Runs {
    dir: PathBuf,
}

impl Runs {
    pub fn new(dir: PathBuf) -> Runs {
        Runs { dir }
    }

    /// One file per archive, named after its path (FNV-1a: the same in every MDM version).
    fn file(&self, archive: &Path) -> PathBuf {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in path_key(archive, cfg!(windows)) {
            h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.dir.join(format!("{h:016x}.run"))
    }

    /// The folder of the run MDM started for `archive`, if it recorded one.
    pub fn find(&self, archive: &Path) -> Option<PathBuf> {
        let text = std::fs::read_to_string(self.file(archive)).ok()?;
        let mut lines = text.lines();
        let (path, out) = (lines.next()?, lines.next()?);
        let same = path_key(Path::new(path), cfg!(windows)) == path_key(archive, cfg!(windows));
        (same && !out.is_empty()).then(|| PathBuf::from(out))
    }

    /// Record that MDM starts extracting `archive` in place into `out`. Paths that aren't
    /// Unicode can't be written down, so they are never extracted in place.
    pub fn start(&self, archive: &Path, out: &Path) -> std::io::Result<()> {
        let (Some(a), Some(o)) = (archive.to_str(), out.to_str()) else {
            return Err(std::io::Error::other("not a Unicode path"));
        };
        if a.contains('\n') || o.contains('\n') {
            return Err(std::io::Error::other("a line break in a path"));
        }
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.file(archive), format!("{a}\n{o}\n"))
    }

    /// After a run: forgotten when HatchZip has nothing left to continue.
    pub fn end(&self, archive: &Path) {
        if !progress(archive) {
            let _ = std::fs::remove_file(self.file(archive));
        }
    }
}

/// Extract `archive` in place into `out` (made if missing). The password goes through the
/// environment, never the command line (which other programs can read). `cancel` stops it:
/// HatchZip is safe to stop at any moment. What it wrote is checked by the caller.
pub fn extract(
    tool: &Path,
    archive: &Path,
    out: &Path,
    password: Option<&str>,
    cancel: Option<&AtomicBool>,
) -> Result<(), Error> {
    std::fs::create_dir_all(out)?;
    // Full paths: HatchZip runs in its output folder, MDM's own and writable, never in the
    // folder MDM was started in (its install folder from the Start menu), as 7-Zip does (#845).
    let full = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_owned());
    let (out, archive) = (full(out), full(archive));
    let mut output = OsString::from("--output=");
    output.push(&out);
    let mut cmd = Command::new(tool);
    // `--`: the archive's name is never read as an option, whatever it starts with.
    cmd.arg("extract")
        .arg(output)
        .args(["-y", "-q", "--"])
        .arg(&archive)
        .current_dir(&out);
    cmd.env_remove("HATCHZIP_PASSWORD");
    if let Some(pw) = password {
        cmd.env("HATCHZIP_PASSWORD", pw);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // The password is never logged.
    tracing::info!(
        "extract: {} extract {} (in place)",
        tool.display(),
        archive.display()
    );
    let mut child = cmd.spawn()?;
    // stderr is read on its own thread so a long message can't fill the pipe and stall it.
    let mut pipe = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(p) = pipe.as_mut() {
            let _ = std::io::Read::read_to_string(p, &mut s);
        }
        s
    });
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break Some(st);
        }
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let err = reader.join().unwrap_or_default();
    let Some(status) = status else {
        return Err(Error::Cancelled);
    };
    let first = || {
        err.lines()
            .map(|l| l.trim().trim_start_matches("error:").trim())
            .find(|l| !l.is_empty())
            .unwrap_or("HatchZip failed")
            .to_owned()
    };
    match status.code() {
        Some(0) => {}
        Some(5) => {
            return Err(if password.is_some() {
                Error::WrongPassword
            } else {
                Error::NeedsPassword
            });
        }
        Some(3) => return Err(Error::NoRoom(first())),
        Some(130) => return Err(Error::Cancelled),
        _ => return Err(Error::Damaged(first())),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_is_found_whatever_the_case_where_names_ignore_it() {
        let (a, b) = (Path::new("D:\\Dl\\Game.ZIP"), Path::new("d:\\dl\\game.zip"));
        assert_eq!(path_key(a, true), path_key(b, true));
        assert_ne!(path_key(a, false), path_key(b, false));
        // Elsewhere a record is found by the path as it is.
        let t = tempfile::tempdir().unwrap();
        let runs = Runs::new(t.path().join("runs"));
        let archive = t.path().join("a.zip");
        runs.start(&archive, &t.path().join("a.hatchzip-out"))
            .unwrap();
        assert_eq!(runs.find(&archive), Some(t.path().join("a.hatchzip-out")));
        assert_eq!(runs.find(&t.path().join("b.zip")), None);
    }
}
