//! Updating in place (Help → Check for updates → Update now): this copy's release file, fetched
//! and checked in the background (`mdm_tools::update`: the release's signed checksum list), then
//! put where this copy is, on a thread of its own too (unpacking, `tar`, `hdiutil`, `ditto`: the
//! window's thread waits for none of them, #222). MDM closes for it; the new copy starts with
//! `/updated`, waits for the old one to be gone, and removes what the update set aside.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mdm_tools::update::{Install, Progress};

/// The switch the updated copy starts with.
pub const UPDATED: &str = "/updated";

/// What follows once the file is in place.
#[derive(Debug)]
pub enum Next {
    /// MDM closes once this has started: the new copy (or the installer, which starts it).
    /// Started on the worker, but only when not cancelled meanwhile ([`Job::drop`]: closing the
    /// window starts nothing).
    Restart,
    /// Handed to the system's package installer (a .deb or .rpm): MDM stays open.
    Opened,
}

/// The release file this copy updates from, when `assets` (name, address) has it (by either of
/// its names, [`Install::assets`]) and the signature of the release's checksum list.
pub fn asset_for(install: Install, version: &str, assets: &[(String, String)]) -> Option<String> {
    let has = |n: &str| assets.iter().any(|(a, _)| a == n);
    if !(has("SHA256SUMS.txt") && has("SHA256SUMS.txt.sig")) {
        return None;
    }
    install.assets(version).into_iter().find(|n| has(n))
}

/// Where update files are kept until the next start.
fn folder() -> Option<PathBuf> {
    mdm_ipc::data_dir().map(|d| d.join("updates"))
}

/// A release file on its way: its progress, then the checked file put in place ([`apply`]), or
/// why not. Dropped (Cancel, the window closed), it stops.
pub struct Job {
    pub progress: Arc<Progress>,
    slot: Arc<Mutex<Option<Result<Next, String>>>>,
    /// The file is downloaded and being put in place (Cancel can't stop that).
    installing: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.task.abort();
    }
}

impl Job {
    pub fn start(
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
        install: Install,
        assets: Vec<(String, String)>,
        name: String,
    ) -> Job {
        let progress = Arc::new(Progress::default());
        let slot = Arc::new(Mutex::new(None));
        let installing = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (p, out, busy, canc, ctx) = (
            progress.clone(),
            slot.clone(),
            installing.clone(),
            cancelled.clone(),
            ctx.clone(),
        );
        // Repaint while it runs, for the progress bar.
        let ticker = ctx.clone();
        let net = crate::settings::own_net();
        let task = rt.spawn(async move {
            let fetched = match folder() {
                Some(dir) => mdm_tools::update::fetch(&assets, &name, &dir, &net, &p)
                    .await
                    .map_err(|e| e.to_string()),
                None => Err("no data folder".into()),
            };
            let res = match fetched {
                Ok(file) => {
                    busy.store(true, Ordering::Release);
                    ctx.request_repaint();
                    tokio::task::spawn_blocking(move || apply(install, &file, &canc))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r)
                }
                Err(e) => Err(e),
            };
            *out.lock().unwrap() = Some(res);
            ctx.request_repaint();
        });
        ticker.request_repaint_after(Duration::from_millis(200));
        Job {
            progress,
            slot,
            installing,
            cancelled,
            task,
        }
    }

    /// The update put in place (what follows), or why not, once it ended.
    pub fn result(&self) -> Option<Result<Next, String>> {
        self.slot.lock().unwrap().take()
    }

    /// The file is downloaded and being put in place.
    pub fn installing(&self) -> bool {
        self.installing.load(Ordering::Acquire)
    }
}

/// Put `file` (checked) in place of this `install`. Long (unpacking, copying, programs waited
/// for): never on the window's thread ([`Job`] runs it).
fn apply(install: Install, file: &Path, cancelled: &AtomicBool) -> Result<Next, String> {
    // Tests only (feature `test-update`): "Installing the update..." held this many seconds.
    #[cfg(feature = "test-update")]
    if let Some(secs) = std::env::var("MDM_TEST_UPDATE_HOLD")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        std::thread::sleep(Duration::from_secs(secs));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("no program folder")?.to_owned();
    let unpack_dir = || -> Result<PathBuf, String> {
        let d = folder().ok_or("no data folder")?.join("unpacked");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        Ok(d)
    };
    let restart = |program: &Path| -> Result<Next, String> {
        if cancelled.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        let mut cmd = Command::new(program);
        cmd.arg(UPDATED);
        cmd.spawn().map_err(|e| e.to_string())?;
        Ok(Next::Restart)
    };
    match install {
        // The installer closes MDM itself (/exit), installs where it was (the same install
        // mode), and starts it again (/relaunch, tools/installer/mdm.iss).
        Install::Installer => {
            if cancelled.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
            let mut cmd = Command::new(file);
            cmd.args([
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/relaunch=1",
            ]);
            cmd.spawn().map_err(|e| e.to_string())?;
            Ok(Next::Restart)
        }
        Install::PortableZip | Install::PlainZip => {
            let out = unpack_dir()?;
            mdm_tools::update::unzip(file, &out).map_err(|e| e.to_string())?;
            let new = mdm_tools::update::single_folder(&out);
            // A portable copy's list and settings stay where they are.
            mdm_tools::update::replace_files(&new, &dir, &["data"]).map_err(|e| e.to_string())?;
            restart(&exe)
        }
        Install::TarGz => {
            let out = unpack_dir()?;
            let ok = Command::new("tar")
                .arg("-xzf")
                .arg(file)
                .arg("-C")
                .arg(&out)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                return Err("couldn't unpack the update".into());
            }
            let new = mdm_tools::update::single_folder(&out);
            mdm_tools::update::replace_files(&new, &dir, &[]).map_err(|e| e.to_string())?;
            restart(&exe)
        }
        Install::AppImage => {
            let target = std::env::var_os("APPIMAGE")
                .map(PathBuf::from)
                .ok_or("not running as an AppImage")?;
            let staged = target.with_extension("AppImage.new");
            std::fs::copy(file, &staged).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
                    .map_err(|e| e.to_string())?;
            }
            std::fs::rename(&staged, &target).map_err(|e| e.to_string())?;
            restart(&target)
        }
        Install::Dmg => {
            let bundle = mdm_tools::update::app_bundle(&exe).ok_or("not in an app bundle")?;
            let mount = folder().ok_or("no data folder")?.join("mount");
            std::fs::create_dir_all(&mount).map_err(|e| e.to_string())?;
            let run = |c: &mut Command| c.status().is_ok_and(|s| s.success());
            if !run(Command::new("hdiutil")
                .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
                .arg(&mount)
                .arg(file))
            {
                return Err("couldn't open the update's disk image".into());
            }
            let name = bundle.file_name().ok_or("no app name")?.to_owned();
            let staged = bundle.with_extension("app.new");
            let _ = std::fs::remove_dir_all(&staged);
            let copied = run(Command::new("ditto").arg(mount.join(&name)).arg(&staged));
            run(Command::new("hdiutil").arg("detach").arg(&mount));
            if !copied {
                return Err("couldn't copy the new app".into());
            }
            // The running app is renamed aside (removed on the next start), the new one takes
            // its place.
            let old = bundle.with_extension("app.old");
            let _ = std::fs::remove_dir_all(&old);
            std::fs::rename(&bundle, &old).map_err(|e| e.to_string())?;
            std::fs::rename(&staged, &bundle).map_err(|e| e.to_string())?;
            if cancelled.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
            let mut cmd = Command::new("open");
            cmd.arg("-n").arg(&bundle).args(["--args", UPDATED]);
            cmd.spawn().map_err(|e| e.to_string())?;
            Ok(Next::Restart)
        }
        // Installing a package needs the administrator: the system's installer asks.
        Install::Deb | Install::Rpm => {
            if cancelled.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
            open::that_detached(file).map_err(|e| e.to_string())?;
            Ok(Next::Opened)
        }
        Install::System => Err("installed by the system's package manager: update it there".into()),
        Install::Source => Err("built from source: update it from the repository".into()),
    }
}

/// The single-instance lock for a copy started by an update (`/updated`): the old copy may
/// still be saving and closing, so it is waited for (up to 20 s).
pub fn lock_after_update() -> Option<std::fs::File> {
    let start = Instant::now();
    loop {
        if let Some(lock) = mdm_ipc::instance_lock() {
            return Some(lock);
        }
        if start.elapsed() > Duration::from_secs(20) {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// At start, once this is the running copy: what an update set aside (the old programs, a
/// Mac's old app) and its downloaded files are removed.
pub fn tidy() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if let Some(dir) = exe.parent() {
        mdm_tools::update::remove_old(dir);
    }
    if let Some(bundle) = mdm_tools::update::app_bundle(&exe) {
        let _ = std::fs::remove_dir_all(bundle.with_extension("app.old"));
    }
    if let Some(dir) = folder() {
        // A running installer (it starts the new copy before it ends) keeps its file: the next
        // start removes it.
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_signed_release_with_this_copys_file_updates_it() {
        let a = |names: &[&str]| -> Vec<(String, String)> {
            names
                .iter()
                .map(|n| (n.to_string(), format!("https://x/{n}")))
                .collect()
        };
        let full = a(&[
            "MisterDownloadManager-0.2.2-setup.exe",
            "SHA256SUMS.txt",
            "SHA256SUMS.txt.sig",
        ]);
        assert_eq!(
            asset_for(Install::Installer, "0.2.2", &full).as_deref(),
            Some("MisterDownloadManager-0.2.2-setup.exe")
        );
        // An unsigned release, another kind's file, a build from source: no.
        let unsigned = a(&["MisterDownloadManager-0.2.2-setup.exe", "SHA256SUMS.txt"]);
        assert_eq!(asset_for(Install::Installer, "0.2.2", &unsigned), None);
        assert_eq!(asset_for(Install::PortableZip, "0.2.2", &full), None);
        assert_eq!(asset_for(Install::Source, "0.2.2", &full), None);
    }

    /// Linux's and macOS's files lose "-experimental" in a release: a copy finds its file by
    /// either name, the plain one first.
    #[test]
    fn a_release_names_linux_files_either_way() {
        let release = |names: &[&str]| -> Vec<(String, String)> {
            names
                .iter()
                .chain(&["SHA256SUMS.txt", "SHA256SUMS.txt.sig"])
                .map(|n| (n.to_string(), format!("https://x/{n}")))
                .collect()
        };
        let old = "MisterDownloadManager-0.3.0-linux-x86_64-experimental.AppImage";
        let new = "MisterDownloadManager-0.3.0-linux-x86_64.AppImage";
        let deb = "MisterDownloadManager-0.3.0-linux-x86_64.deb";
        for (assets, want) in [
            (release(&[old]), Some(old)),
            (release(&[new, deb]), Some(new)),
            (release(&[old, new]), Some(new)),
            (release(&[deb]), None),
        ] {
            assert_eq!(
                asset_for(Install::AppImage, "0.3.0", &assets).as_deref(),
                want,
                "{assets:?}"
            );
        }
        assert_eq!(
            asset_for(Install::Deb, "0.3.0", &release(&[new, deb])).as_deref(),
            Some(deb)
        );
        // Unsigned: neither.
        let unsigned = vec![(new.to_string(), format!("https://x/{new}"))];
        assert_eq!(asset_for(Install::AppImage, "0.3.0", &unsigned), None);
    }

    #[test]
    fn dropped_before_apply_spawns_starts_nothing() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let job = Job {
            progress: Arc::new(Progress::default()),
            slot: Arc::new(Mutex::new(None)),
            installing: Arc::new(AtomicBool::new(false)),
            cancelled: cancelled.clone(),
            task: rt.spawn(async {}),
        };
        assert!(!cancelled.load(Ordering::Acquire));
        drop(job);
        assert!(cancelled.load(Ordering::Acquire));

        let res = apply(Install::Installer, Path::new("setup.exe"), &cancelled);
        assert_eq!(res.unwrap_err(), "cancelled");
    }
}
