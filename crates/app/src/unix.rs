//! Linux and macOS counterparts of what `win32` does through Windows (macOS's build experimental):
//! starting with the session, the browsers' host registration, sounds, showing a file in its
//! folder, and the torrent link handler. Each runs the system's own tool, so nothing here needs
//! a library; the exceptions are the desktop's dark mode on Linux (D-Bus through zbus, the
//! settings daemon's XSETTINGS through x11rb) and the
//! crash reports (at the end): signals and Mach exceptions through crash-handler, thread and
//! process ids through libc.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The program to start again later: the AppImage file itself when running from one (its
/// programs sit in a temporary mount), else this executable.
pub fn program() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
}

/// Run a tool quietly, without waiting for it.
fn spawn(cmd: &mut Command) {
    let _ = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Where "start with the session" is kept: an XDG autostart entry on Linux, a LaunchAgent on
/// macOS.
fn startup_file() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_os = "macos") {
        Some(home.join("Library/LaunchAgents/io.github.anefzaoui.misterdownloadmanager.plist"))
    } else {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        Some(config.join("autostart/misterdownloadmanager.desktop"))
    }
}

/// A headless run (#157) is asked to stop: SIGTERM (`docker stop`, systemd) or SIGINT (Ctrl+C).
pub async fn terminated() {
    use tokio::signal::unix::{SignalKind, signal};
    match (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) {
        (Ok(mut term), Ok(mut int)) => {
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
            }
        }
        _ => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

pub fn run_at_startup() -> bool {
    startup_file().is_some_and(|f| f.is_file())
}

/// The startup entry for the program `exe`: started with the session, MDM waits in the
/// notification area (`/startup`, #226). A LaunchAgent on macOS, an XDG autostart entry on
/// Linux.
fn startup_entry(exe: &Path, macos: bool) -> String {
    let exe = exe.display().to_string();
    let arg = mdm_ipc::STARTUP;
    if macos {
        let exe = exe
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n\
             \x20 <key>Label</key><string>io.github.anefzaoui.misterdownloadmanager</string>\n\
             \x20 <key>ProgramArguments</key><array><string>{exe}</string>\
             <string>{arg}</string></array>\n\
             \x20 <key>RunAtLoad</key><true/>\n\
             </dict>\n</plist>\n"
        )
    } else {
        // Desktop Entry quoting: the path in double quotes, with \ " ` $ escaped.
        let quoted: String = exe
            .chars()
            .flat_map(|c| match c {
                '\\' | '"' | '`' | '$' => vec!['\\', c],
                c => vec![c],
            })
            .collect();
        format!(
            "[Desktop Entry]\nType=Application\nName=Mister Download Manager\n\
             Exec=\"{quoted}\" {arg}\nIcon=misterdownloadmanager\nTerminal=false\n\
             X-GNOME-Autostart-enabled=true\n"
        )
    }
}

/// A startup entry written before #226 for the program `exe`: it names that program but passes
/// no `/startup`. Another copy's entry (an AppImage elsewhere) is not this one's to rewrite.
fn startup_outdated(text: &str, exe: &Path, macos: bool) -> bool {
    !text.contains(mdm_ipc::STARTUP)
        && entry_program(text, macos).is_some_and(|p| Path::new(&p) == exe)
}

/// The program a startup entry starts: a LaunchAgent's first argument, an autostart entry's
/// `Exec=` program (unquoted as the Desktop Entry specification quotes it).
fn entry_program(text: &str, macos: bool) -> Option<String> {
    if macos {
        let start = text.find("<array><string>")? + "<array><string>".len();
        let end = start + text[start..].find("</string>")?;
        return Some(
            text[start..end]
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&"),
        );
    }
    let line = text.lines().find_map(|l| l.strip_prefix("Exec="))?;
    let Some(rest) = line.strip_prefix('"') else {
        return line.split_whitespace().next().map(str::to_owned);
    };
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    None
}

pub fn set_run_at_startup(on: bool) {
    let (Some(file), Some(exe)) = (startup_file(), program()) else {
        return;
    };
    if !on {
        let _ = std::fs::remove_file(&file);
        return;
    }
    let text = startup_entry(&exe, cfg!(target_os = "macos"));
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&file, text);
}

/// The startup entry written again with its argument when it was written without (by versions
/// before 0.5.0) for this very program (#226).
pub fn update_run_at_startup() {
    let (Some(file), Some(exe)) = (startup_file(), program()) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    if startup_outdated(&text, &exe, cfg!(target_os = "macos")) {
        tracing::info!("startup entry written again, to start in the notification area");
        set_run_at_startup(true);
    }
}

/// The browsers' helper program (crates/nmhost), beside this one.
const HOST: &str = "misterdownloadmanager-nmhost";
/// The manifest it writes in each browser's folder.
const MANIFEST: &str = "com.misterdownloadmanager.host.json";
/// How long the helper's `--register` has at a start (it writes a few small files).
const REGISTER_LIMIT: std::time::Duration = std::time::Duration::from_secs(20);
/// Why the last registration failed, in the system's words (Test browser integration says it);
/// None once one succeeded.
static REGISTER_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Browser integration: the host's manifests in every installed browser's folder. There is no
/// installer step on Linux and macOS, so this runs at each start, on a thread of its own: a
/// moved app, a browser installed since, or a helper copy that went missing is repaired.
///
/// From an AppImage, whose programs sit in a mount that is gone once it quits, the host is first
/// copied to the data folder with the AppImage's path beside it (`app-path`: the host starts
/// that when MDM isn't running), and registered from there (#253).
pub fn register_browsers() {
    let started = std::thread::Builder::new()
        .name("register-browsers".into())
        .spawn(|| register_browsers_now(REGISTER_LIMIT));
    if let Err(e) = started {
        tracing::warn!("browser integration: not set up, no thread for it: {e}");
    }
}

/// `register_browsers`, waiting for it to finish, up to `limit` (Test browser integration's Set
/// it up, on a thread of its own: extension::set_up). Every failure is logged and kept for Test
/// browser integration ([`register_error`]).
pub fn register_browsers_now(limit: std::time::Duration) {
    let result = host_program().and_then(|host| run_register(&host, limit));
    match &result {
        Ok(said) => tracing::info!("browser integration set up: {said}"),
        Err(e) => tracing::warn!("browser integration not set up: {e}"),
    }
    if let Ok(mut last) = REGISTER_ERROR.lock() {
        *last = result.err();
    }
}

/// Why the last registration failed, if it did.
pub fn register_error() -> Option<String> {
    REGISTER_ERROR.lock().ok().and_then(|e| e.clone())
}

/// The helper to register: beside this program, or from an AppImage its copy in the data folder
/// (made or repaired first).
fn host_program() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("this program's path: {e}"))?;
    let host = exe.with_file_name(HOST);
    let Some(image) = std::env::var_os("APPIMAGE").filter(|i| !i.is_empty()) else {
        return if host.is_file() {
            Ok(host)
        } else {
            Err(format!("{}: not found", host.display()))
        };
    };
    let data = mdm_ipc::data_dir().ok_or("no data folder to copy the helper to")?;
    install_host_copy(&host, &data.join("browser-host"), Path::new(&image))
}

/// The AppImage's helper `from` copied into `dir`, with the AppImage's path (`image`) beside it
/// in `app-path`; each written only when it differs. A copy is written under a temporary name,
/// made executable, then renamed over the old one: a browser still running the old copy keeps it
/// (writing over a running program fails with "Text file busy"), and the manifest never names a
/// half-written file.
fn install_host_copy(from: &Path, dir: &Path, image: &Path) -> Result<PathBuf, String> {
    let failed = |what: &Path, e: std::io::Error| format!("{}: {e}", what.display());
    std::fs::create_dir_all(dir).map_err(|e| failed(dir, e))?;
    let copy = dir.join(HOST);
    let program = std::fs::read(from).map_err(|e| failed(from, e))?;
    if !(runs(&copy) && std::fs::read(&copy).is_ok_and(|old| old == program)) {
        replace(&copy, &program, 0o755).map_err(|e| failed(&copy, e))?;
        tracing::info!("browser integration: helper copied to {}", copy.display());
    }
    let app_path = dir.join("app-path");
    let image = image.as_os_str().as_encoded_bytes();
    if std::fs::read(&app_path).ok().as_deref() != Some(image) {
        replace(&app_path, image, 0o644).map_err(|e| failed(&app_path, e))?;
    }
    Ok(copy)
}

/// `path` written as `bytes` with `mode`, through a temporary file renamed over it.
fn replace(path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)
        .and_then(|mut f| {
            f.write_all(bytes)?;
            // The mode asked for, whatever the umask took away.
            f.set_permissions(std::fs::Permissions::from_mode(mode))?;
            f.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// A program file that can be run.
fn runs(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The helper's `--register`: what it says it did, or why it failed.
fn run_register(host: &Path, limit: std::time::Duration) -> Result<String, String> {
    use std::io::Read;
    let mut child = Command::new(host)
        .arg("--register")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", host.display()))?;
    let status = crate::launch::wait_within(&mut child, limit);
    // A few lines at most: they fit the pipes, so the helper never waits on them.
    let mut said = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut said);
    }
    if let Some(mut err) = child.stderr.take() {
        said.push('\n');
        let _ = err.read_to_string(&mut said);
    }
    let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
    match status {
        Some(s) if s.success() => Ok(said),
        Some(s) => Err(format!("{} --register: {s}: {said}", host.display())),
        None => Err(format!(
            "{} --register: no answer in {limit:?}",
            host.display()
        )),
    }
}

/// Where browser integration stands (Options → General → Test browser integration): the
/// manifests in the folders the browsers here read (mdm_ipc::browsers), for this user, or for
/// everyone when no browser is there for this user, and the helper each names. A browser that
/// reads the package's manifest for everyone naming this helper (#266) needs none of its own.
pub fn host_registration() -> crate::extension::Registration {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let mac = cfg!(target_os = "macos");
    let host = registered_host();
    let user = mdm_ipc::browsers::host_dirs(&home, &config, mac, false, host.as_deref());
    if user.is_empty() {
        registration_in(
            mdm_ipc::browsers::host_dirs(&home, &config, mac, true, None),
            true,
        )
    } else {
        registration_in(user, false)
    }
}

/// The helper `register_browsers` registers, without copying anything: beside this program, or
/// from an AppImage its copy in the data folder.
fn registered_host() -> Option<PathBuf> {
    if std::env::var_os("APPIMAGE").is_some_and(|i| !i.is_empty()) {
        return mdm_ipc::data_dir().map(|d| d.join("browser-host").join(HOST));
    }
    std::env::current_exe().ok().map(|e| e.with_file_name(HOST))
}

/// `host_registration` for the host folders `dirs`, everyone's (`all_users`) or one user's.
fn registration_in(
    dirs: Vec<(PathBuf, mdm_ipc::browsers::Family)>,
    all_users: bool,
) -> crate::extension::Registration {
    use crate::extension::Registration;
    if dirs.is_empty() {
        return Registration::NoBrowser;
    }
    // A browser reads its user folder's manifest first: each must be there and name a helper
    // that runs. Everyone's folders: one that works is enough.
    let (mut ready, mut missing, mut broken) = (0, 0, None);
    for (dir, _) in dirs {
        let manifest = dir.join(MANIFEST);
        if !manifest.is_file() {
            missing += 1;
            continue;
        }
        match mdm_ipc::browsers::manifest_program(&manifest) {
            Some(p) if runs(&p) => ready += 1,
            Some(p) => broken = broken.or(Some(p)),
            None => missing += 1,
        }
    }
    match broken {
        Some(p) if !(all_users && ready > 0) => Registration::HelperMissing(p),
        _ if ready > 0 && (all_users || missing == 0) => Registration::Ready,
        _ => Registration::Missing,
    }
}

/// Options → Sounds: play a sound file (macOS: afplay; Linux: PulseAudio / PipeWire's paplay,
/// else ALSA's aplay).
pub fn play_sound(file: &str) {
    if cfg!(target_os = "macos") {
        spawn(Command::new("afplay").arg(file));
    } else {
        let file = file.to_owned();
        std::thread::spawn(move || {
            let played = Command::new("paplay")
                .arg(&file)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !played {
                spawn(Command::new("aplay").arg("-q").arg(&file));
            }
        });
    }
}

/// The system and its version, for problem reports: "macOS 15.1" (sw_vers), or the Linux
/// distribution's name from /etc/os-release ("Ubuntu 24.04.1 LTS").
pub fn os_version() -> String {
    if cfg!(target_os = "macos") {
        let v = crate::launch::output_within(
            Command::new("sw_vers").arg("-productVersion"),
            std::time::Duration::from_secs(2),
        )
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
        return format!("macOS {v}").trim_end().to_owned();
    }
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_owned())
        })
        .unwrap_or_else(|| "Linux".into())
}

/// What `cmd` prints, trimmed, when it succeeds within `secs` seconds (`launch::output_within`).
fn printed_within(cmd: &mut Command, secs: u64) -> Option<String> {
    crate::launch::output_within(cmd, std::time::Duration::from_secs(secs))
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// The default browser (#668): the desktop's choice for web links (`xdg-settings`, else
/// `mimeapps.list`), the program its `.desktop` file runs, and that program's version (Firefox's
/// `application.ini` beside it, else its `--version`, on this thread, bounded in time). None when
/// it isn't a browser MDM knows (MDM then speaks as Chrome).
#[cfg(target_os = "linux")]
pub fn default_browser() -> Option<mdm_engine::agent::Found> {
    use crate::defaultbrowser::{exec_program, mimeapps_https, version_in};
    use crate::xdgicons::{Dirs, ini_value};
    use mdm_engine::agent::{Browser, Found};
    let var = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    let home = var("HOME");
    let desktop = printed_within(
        Command::new("xdg-settings").args(["get", "default-web-browser"]),
        5,
    )
    .filter(|d| d.ends_with(".desktop"))
    .or_else(|| {
        let config =
            var("XDG_CONFIG_HOME").or_else(|| home.as_ref().map(|h| format!("{h}/.config")));
        let lists = config
            .iter()
            .map(|c| format!("{c}/mimeapps.list"))
            .chain(["/etc/xdg/mimeapps.list".to_owned()]);
        lists
            .filter_map(|l| std::fs::read_to_string(l).ok())
            .find_map(|t| mimeapps_https(&t))
    })?;
    let dirs = Dirs::new(
        home.as_deref(),
        var("XDG_DATA_HOME").as_deref(),
        var("XDG_DATA_DIRS").as_deref(),
    );
    // Snaps' and Flatpaks' launchers, also when the session's XDG_DATA_DIRS doesn't list them.
    let entry = dirs
        .data
        .iter()
        .map(|d| d.join("applications"))
        .chain(
            [
                "/var/lib/snapd/desktop/applications",
                "/var/lib/flatpak/exports/share/applications",
            ]
            .map(PathBuf::from),
        )
        .find_map(|d| std::fs::read_to_string(d.join(&desktop)).ok());
    let program = entry
        .as_deref()
        .and_then(|e| ini_value(e, "Desktop Entry", "Exec"))
        .and_then(|e| exec_program(&e));
    let base = program
        .as_deref()
        .and_then(|p| Path::new(p).file_name())
        .map(|b| b.to_string_lossy().into_owned());
    let browser = Browser::named(&desktop).or_else(|| base.as_deref().and_then(Browser::named))?;
    let firefox = matches!(browser, Browser::Firefox | Browser::FirefoxFork);
    // A Flatpak's launcher would start its sandbox, and its files are inside it: its version
    // stays unknown (MDM's estimate of today's).
    let program = program.filter(|_| base.as_deref() != Some("flatpak"));
    let install = program
        .as_deref()
        .filter(|_| firefox)
        .and_then(|p| firefox_install(p, browser == Browser::Firefox));
    let version = install
        .as_ref()
        .and_then(|d| std::fs::read_to_string(d.join("application.ini")).ok())
        .and_then(|t| ini_value(&t, "App", "Version"))
        .or_else(|| {
            let program = program.as_deref()?;
            version_in(&printed_within(Command::new(program).arg("--version"), 10)?)
        });
    // Ubuntu's own Firefox (its snap) says so in its User-Agent, and its distribution file
    // names Ubuntu as its partner.
    let ubuntu = install
        .and_then(|d| std::fs::read_to_string(d.join("distribution/distribution.ini")).ok())
        .is_some_and(|t| t.contains("partner.id=\"ubuntu\""));
    Some(Found {
        browser,
        version,
        ubuntu,
    })
}

/// The folder a Firefox `program` runs from (the one holding its `application.ini`): a snap's
/// (`/snap/bin/firefox` → `/snap/firefox/current/usr/lib/firefox`), the program's own folder
/// with its links followed, else (`plain`: Firefox itself, not a fork) the distributions' usual
/// places, for a `/usr/bin/firefox` that is a script.
#[cfg(target_os = "linux")]
fn firefox_install(program: &str, plain: bool) -> Option<PathBuf> {
    let path = if program.contains('/') {
        PathBuf::from(program)
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join(program))
            .find(|p| p.is_file())?
    };
    let snap = path
        .strip_prefix("/snap/bin")
        .ok()
        .and_then(|n| n.to_str())
        .map(|n| PathBuf::from(format!("/snap/{n}/current/usr/lib/{n}")));
    let own = std::fs::canonicalize(&path)
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let usual = ["/usr/lib/firefox", "/usr/lib64/firefox", "/opt/firefox"]
        .map(PathBuf::from)
        .into_iter()
        .filter(|_| plain);
    snap.into_iter()
        .chain(own)
        .chain(usual)
        .find(|d| d.join("application.ini").is_file())
}

/// The default browser (#668): the app macOS opens https links with (LaunchServices' handlers;
/// none set is Safari), and its version (its Info.plist's). None when it isn't a browser MDM
/// knows (MDM then speaks as Chrome).
#[cfg(target_os = "macos")]
pub fn default_browser() -> Option<mdm_engine::agent::Found> {
    use mdm_engine::agent::{Browser, Found};
    let handlers = printed_within(
        Command::new("defaults").args([
            "read",
            "com.apple.LaunchServices/com.apple.launchservices.secure",
            "LSHandlers",
        ]),
        5,
    );
    let id = handlers
        .as_deref()
        .and_then(|h| crate::defaultbrowser::ls_handler(h, "https"))
        .unwrap_or_else(|| "com.apple.safari".to_owned());
    let browser = Browser::named(&id)?;
    // Where the app is: Spotlight's answer, else where browsers install themselves.
    let safe_id: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    let found = printed_within(
        Command::new("mdfind").arg(format!("kMDItemCFBundleIdentifier == '{safe_id}'c")),
        5,
    )
    .and_then(|o| o.lines().find(|l| l.ends_with(".app")).map(PathBuf::from));
    let app = found.or_else(|| {
        let name = match browser {
            Browser::Chrome => "Google Chrome",
            Browser::Edge => "Microsoft Edge",
            Browser::Brave => "Brave Browser",
            other => other.name(),
        };
        let home = std::env::var_os("HOME").map(PathBuf::from);
        [
            Some(PathBuf::from("/Applications")),
            home.map(|h| h.join("Applications")),
        ]
        .into_iter()
        .flatten()
        .map(|d| d.join(format!("{name}.app")))
        .find(|a| a.is_dir())
    });
    let version = app.and_then(|a| {
        printed_within(
            Command::new("defaults")
                .arg("read")
                .arg(a.join("Contents/Info"))
                .arg("CFBundleShortVersionString"),
            5,
        )
    });
    Some(Found {
        browser,
        version,
        ubuntu: false,
    })
}

/// How long the file manager has to answer ShowItems before its folder is opened instead: one
/// started for it (D-Bus activation) answers in a second or two.
const SHOW_ITEMS_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Show the file selected in the file manager (macOS: Finder; Linux: the desktop's file
/// manager through D-Bus, else its folder opened). Returns at once: the file manager is asked
/// on a thread of its own (crate::launch), as one slow to start, or that never answers, froze
/// the window for as long as `gdbus` waited (#222); when it doesn't answer within
/// [`SHOW_ITEMS_LIMIT`], the folder is opened instead.
///
/// `gdbus` rather than a zbus call: the same tool and call the desktops answered before, ended
/// at the limit by crate::launch (zbus's blocking calls have no time limit of their own, and a
/// connection to the session bus would be made for this one call).
pub fn show_in_folder(path: &Path) {
    let dir = path.parent().map(Path::to_path_buf);
    if !path.exists() {
        if let Some(dir) = dir.filter(|d| d.is_dir()) {
            crate::launch::open(dir);
        }
        return;
    }
    if cfg!(target_os = "macos") {
        spawn(Command::new("open").arg("-R").arg(path));
        return;
    }
    let Ok(url) = url::Url::from_file_path(path) else {
        return;
    };
    let mut gdbus = Command::new("gdbus");
    gdbus.args([
        "call",
        "--session",
        "--dest",
        "org.freedesktop.FileManager1",
        "--object-path",
        "/org/freedesktop/FileManager1",
        "--method",
        "org.freedesktop.FileManager1.ShowItems",
        &format!("['{}']", url.as_str().replace('\'', "%27")),
        "",
    ]);
    crate::launch::run_detached_with_fallback(gdbus, SHOW_ITEMS_LIMIT, move || {
        if let Some(dir) = dir {
            let _ = open::that_detached(dir);
        }
    });
}

/// Torrent settings → open magnet links and .torrent files with Mister Download Manager: Linux
/// records it with xdg-mime for the desktop entry the packages install, macOS with Launch
/// Services for the app (`open_events::make_default`, #698). Turning it off leaves the choice to
/// the desktop's settings (macOS: Finder's Get Info → Open with).
pub fn set_torrent_handlers(on: bool) {
    if !on {
        return;
    }
    #[cfg(target_os = "macos")]
    open_events::make_default();
    #[cfg(not(target_os = "macos"))]
    spawn(Command::new("xdg-mime").args([
        "default",
        "misterdownloadmanager.desktop",
        "x-scheme-handler/magnet",
        "application/x-bittorrent",
    ]));
}

/// `win32::assistive_technology_listening`: Linux's AT-SPI adapter answers a screen reader with
/// the first tree a frame produces (no placeholder to replace), and macOS has no such cheap
/// question; there, as on Windows, a window's tree is ready at once for any window read after
/// the first one (egui keeps AccessKit on from the first request).
pub fn assistive_technology_listening() -> bool {
    false
}

/// `win32::wheel_lines`: Linux and macOS have no such setting shared by every toolkit; three
/// rows a notch, as GTK, Qt and Windows' default do.
pub fn wheel_lines() -> Option<f32> {
    Some(3.0)
}

/// The desktop's light or dark preference on Linux: `Some(true)` dark, `Some(false)` light,
/// `None` when nothing says (then the window system's answer is used). X11 reports no theme to
/// winit at all, so without this "Dark Mode support" never turns dark there.
///
/// First the freedesktop portal (`org.freedesktop.appearance` `color-scheme`, which KDE, GNOME,
/// Cinnamon, COSMIC and the others set). When it has no Settings interface (XFCE's, MATE's and
/// older desktops' portals) or no preference, the desktop's theme settings decide
/// ([`theme_settings_dark`]). The first call starts a thread that reads the portal and follows
/// its changes (D-Bus can take seconds to answer, or start the portal), and the fallback's
/// watchers when it is needed; `ctx` is repainted when either changes.
#[cfg(target_os = "linux")]
pub fn desktop_dark(ctx: &egui::Context) -> Option<bool> {
    use std::sync::atomic::Ordering;
    static WATCH: std::sync::Once = std::sync::Once::new();
    WATCH.call_once(|| {
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("desktop-theme".into())
            .spawn(move || {
                let set = |v: &zbus::zvariant::Value| {
                    let scheme = match color_scheme(v) {
                        Some(1) => 1,
                        Some(2) => 2,
                        _ => 0,
                    };
                    if SCHEME.swap(scheme, Ordering::Relaxed) != scheme {
                        desktop_changed(&ctx);
                    }
                    // No preference: the theme settings may still say (GNOME's "default" with
                    // a dark GTK theme, a portal that answers 0 for every desktop).
                    if scheme == 0 {
                        watch_theme_settings(&ctx);
                    }
                };
                let watch = || -> zbus::Result<()> {
                    let conn = zbus::blocking::Connection::session()?;
                    let portal = zbus::blocking::Proxy::new(
                        &conn,
                        "org.freedesktop.portal.Desktop",
                        "/org/freedesktop/portal/desktop",
                        "org.freedesktop.portal.Settings",
                    )?;
                    let changes = portal.receive_signal("SettingChanged")?;
                    let key = ("org.freedesktop.appearance", "color-scheme");
                    // ReadOne since version 2 of the portal; Read (the value wrapped once more)
                    // before it.
                    let now: zbus::zvariant::OwnedValue = portal
                        .call("ReadOne", &key)
                        .or_else(|_| portal.call("Read", &key))?;
                    set(&now);
                    for change in changes {
                        let (namespace, name, value): (String, String, zbus::zvariant::OwnedValue) =
                            change.body().deserialize()?;
                        if (namespace.as_str(), name.as_str()) == key {
                            set(&value);
                        }
                    }
                    Ok(())
                };
                if let Err(e) = watch() {
                    tracing::info!("desktop theme: no answer from the portal ({e})");
                    SCHEME.store(0, Ordering::Relaxed);
                    watch_theme_settings(&ctx);
                }
            });
    });
    let known = |v: u8| match v {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    };
    known(SCHEME.load(Ordering::Relaxed)).or_else(|| known(SETTINGS.load(Ordering::Relaxed)))
}

/// How many times the desktop's light or dark, accent or contrast changed (the watchers here).
/// The app's theme check runs every 2 s; a change seen here runs it at once ([`desktop_changed`]),
/// so the window follows the desktop in the frame that the change wakes (#156).
#[cfg(target_os = "linux")]
static DESKTOP_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// See [`DESKTOP_GENERATION`].
#[cfg(target_os = "linux")]
pub fn desktop_generation() -> u64 {
    DESKTOP_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// The desktop's look changed: counted, and a frame woken to show it.
#[cfg(target_os = "linux")]
fn desktop_changed(ctx: &egui::Context) {
    DESKTOP_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    ctx.request_repaint();
}

// The portal's answer and the theme settings' (0: not known, 1: dark, 2: light).
#[cfg(target_os = "linux")]
static SCHEME: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
#[cfg(target_os = "linux")]
static SETTINGS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// The running settings daemon's `Net/ThemeName` (XSETTINGS), kept by its watcher.
#[cfg(target_os = "linux")]
static XSETTINGS_THEME: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The desktop's dark or light from its theme settings, for when the portal can't say. In order:
/// - GNOME and its relatives: `org.gnome.desktop.interface color-scheme` (what the portal
///   would have answered), when it says prefer-dark or prefer-light;
/// - `GTK_THEME` (e.g. `Adwaita:dark`), which overrides everything for GTK programs;
/// - XSETTINGS `Net/ThemeName`: the GTK theme that the running settings daemon gives every
///   program (xfsettingsd, mate-, cinnamon- and gnome-settings-daemon, LXDE's, Budgie's);
/// - the desktop's own setting: XFCE's `xfconf` xsettings channel, MATE's and Cinnamon's
///   `gtk-theme`, GNOME's (Wayland without XWayland's daemon);
/// - GTK's `settings.ini` (window managers with no settings daemon);
/// - on a window manager's session (`window_manager_session`), GNOME's `gtk-theme` when it names
///   a dark theme: Hyprland's, sway's and the others' guides set it with `gsettings`, and a light
///   value there is only the key's default.
///
/// A theme is dark when a word of its name is "dark" (Adwaita-dark, Yaru-dark, Mint-Y-Dark-Aqua,
/// Greybird-dark, Breeze-Dark); not "Arc-Darker", whose views are light. Another desktop's
/// leftover settings are not read: GNOME's dconf keys stay after a switch to XFCE. A window
/// manager's session has no theme setting of its own, so GNOME's `color-scheme` and `gtk-theme`
/// are the ones its users set for GTK programs (#324).
#[cfg(target_os = "linux")]
fn theme_settings_dark() -> Option<(bool, &'static str)> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let is = |names: &[&str]| desktop.split(':').any(|d| names.contains(&d));
    let gnome = is(&[
        "gnome",
        "unity",
        "ubuntu",
        "budgie",
        "pantheon",
        "gnome-classic",
    ]);
    let window_manager = window_manager_session(&desktop);
    if (gnome || window_manager)
        && let Some(dark) =
            gsetting("org.gnome.desktop.interface", "color-scheme").and_then(|s| scheme_word(&s))
    {
        return Some((dark, "GNOME's color-scheme"));
    }
    if let Ok(theme) = std::env::var("GTK_THEME")
        && !theme.is_empty()
    {
        return Some((theme_name_dark(&theme), "GTK_THEME"));
    }
    if let Some(theme) = XSETTINGS_THEME
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return Some((theme_name_dark(&theme), "XSETTINGS Net/ThemeName"));
    }
    let own = if is(&["xfce"]) {
        command_line("xfconf-query", &["-c", "xsettings", "-p", "/Net/ThemeName"])
    } else if is(&["mate"]) {
        gsetting("org.mate.interface", "gtk-theme")
    } else if is(&["x-cinnamon", "cinnamon"]) {
        gsetting("org.cinnamon.desktop.interface", "gtk-theme")
    } else if gnome {
        gsetting("org.gnome.desktop.interface", "gtk-theme")
    } else {
        None
    };
    if let Some(theme) = own {
        return Some((theme_name_dark(&theme), "the desktop's GTK theme"));
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    let settings_ini = config.and_then(|config| {
        ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"]
            .iter()
            .find_map(|f| gtk_settings_dark(&std::fs::read_to_string(config.join(f)).ok()?))
    });
    if let Some(dark) = settings_ini {
        return Some((dark, "GTK's settings.ini"));
    }
    // After settings.ini: a key's default ('Adwaita') says nothing here, only a dark name does.
    if window_manager
        && gsetting("org.gnome.desktop.interface", "gtk-theme").is_some_and(|t| theme_name_dark(&t))
    {
        return Some((true, "GNOME's gtk-theme"));
    }
    None
}

/// Is `desktop` (`XDG_CURRENT_DESKTOP`, lower case) a window manager's session rather than a
/// desktop with theme settings of its own (Hyprland, sway, i3, niri, an unset value)? There the
/// freedesktop portal is often missing its Settings part (no `xdg-desktop-portal-gtk`), so what
/// its users set for GTK programs, `gsettings set org.gnome.desktop.interface color-scheme`, is
/// the only theme setting there is (#324).
#[cfg(target_os = "linux")]
fn window_manager_session(desktop: &str) -> bool {
    const DESKTOPS: &[&str] = &[
        "gnome",
        "gnome-classic",
        "unity",
        "ubuntu",
        "budgie",
        "pantheon",
        "xfce",
        "mate",
        "x-cinnamon",
        "cinnamon",
        "kde",
        "plasma",
        "lxqt",
        "lxde",
        "cosmic",
        "deepin",
        "dde",
        "ukui",
    ];
    !desktop.split(':').any(|d| DESKTOPS.contains(&d))
}

/// One GSettings key's value, unquoted (`gsettings get`); None without gsettings or the schema.
#[cfg(target_os = "linux")]
fn gsetting(schema: &str, key: &str) -> Option<String> {
    command_line("gsettings", &["get", schema, key]).map(|s| s.trim_matches('\'').to_string())
}

/// A tool's first line of output, trimmed; None when it fails or prints nothing.
#[cfg(target_os = "linux")]
fn command_line(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    (out.status.success() && !line.is_empty()).then_some(line)
}

/// GNOME's `color-scheme`: prefer-dark or prefer-light; None for "default" (no preference).
// Linux only: the desktop theme fallback, and `xdgicons` (its INI reader) is built for Linux.
#[cfg(target_os = "linux")]
fn scheme_word(s: &str) -> Option<bool> {
    match s {
        "prefer-dark" => Some(true),
        "prefer-light" => Some(false),
        _ => None,
    }
}

/// Is a GTK theme's name a dark one? A word of it (split at anything but letters and digits)
/// is "dark", in any case: `Adwaita-dark`, `Adwaita:dark` (GTK_THEME's variant), `Mint-Y-Dark`.
// Linux only: the desktop theme fallback, and `xdgicons` (its INI reader) is built for Linux.
#[cfg(target_os = "linux")]
fn theme_name_dark(name: &str) -> bool {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| word.eq_ignore_ascii_case("dark"))
}

/// GTK's `settings.ini`: `gtk-application-prefer-dark-theme`, else the theme's name.
// Linux only: the desktop theme fallback, and `xdgicons` (its INI reader) is built for Linux.
#[cfg(target_os = "linux")]
fn gtk_settings_dark(text: &str) -> Option<bool> {
    use crate::xdgicons::ini_value;
    let prefer = ini_value(text, "Settings", "gtk-application-prefer-dark-theme");
    if let Some(p) = prefer.as_deref()
        && matches!(p, "1" | "true")
    {
        return Some(true);
    }
    ini_value(text, "Settings", "gtk-theme-name").map(|name| theme_name_dark(&name))
}

/// Starts the theme settings' watchers (once): they read [`theme_settings_dark`] now and again
/// whenever the settings daemon's XSETTINGS or a dconf key of the desktop's interface changes.
#[cfg(target_os = "linux")]
fn watch_theme_settings(ctx: &egui::Context) {
    static WATCH: std::sync::Once = std::sync::Once::new();
    WATCH.call_once(|| {
        let refresh = {
            let ctx = ctx.clone();
            move || {
                use std::sync::atomic::Ordering;
                let found = theme_settings_dark();
                let v = match found {
                    Some((true, _)) => 1,
                    Some((false, _)) => 2,
                    None => 0,
                };
                if SETTINGS.swap(v, Ordering::Relaxed) != v {
                    match found {
                        Some((dark, from)) => tracing::info!(
                            "desktop theme: {} from {from}",
                            if dark { "dark" } else { "light" }
                        ),
                        None => tracing::info!("desktop theme: no setting says"),
                    }
                    desktop_changed(&ctx);
                }
            }
        };
        let x = refresh.clone();
        let _ = std::thread::Builder::new()
            .name("desktop-theme-x".into())
            .spawn(move || {
                if let Err(e) = watch_xsettings(&x) {
                    tracing::info!("desktop theme: no XSETTINGS ({e})");
                }
                // Read once without it (Wayland, no X server).
                x();
            });
        let _ = std::thread::Builder::new()
            .name("desktop-theme-dconf".into())
            .spawn(move || {
                if let Err(e) = watch_dconf(&refresh) {
                    tracing::info!("desktop theme: no dconf changes ({e})");
                }
            });
    });
}

/// Follows the XSETTINGS manager's `_XSETTINGS_SETTINGS` (the settings daemon of an X11
/// session, XWayland's in a Wayland one): keeps `Net/ThemeName` in [`XSETTINGS_THEME`] and calls
/// `changed` after each read, also when the daemon ends or another takes over.
#[cfg(target_os = "linux")]
fn watch_xsettings(changed: &dyn Fn()) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask};
    let (conn, screen) = x11rb::rust_connection::RustConnection::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &str| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
    };
    let selection = atom(&format!("_XSETTINGS_S{screen}"))?;
    let settings = atom("_XSETTINGS_SETTINGS")?;
    let manager = atom("MANAGER")?;
    // A new manager announces itself on the root window (MANAGER client message).
    conn.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
    )?
    .check()?;
    loop {
        let owner = conn.get_selection_owner(selection)?.reply()?.owner;
        let mut theme = None;
        if owner != x11rb::NONE {
            // The owner may be gone already: then no theme, and its end or the next manager
            // brings us back here.
            let watched = conn
                .change_window_attributes(
                    owner,
                    &ChangeWindowAttributesAux::new()
                        .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
                )?
                .check();
            if watched.is_ok()
                && let Ok(reply) = conn
                    .get_property(false, owner, settings, AtomEnum::ANY, 0, u32::MAX / 4)?
                    .reply()
            {
                theme = xsettings_string(&reply.value, "Net/ThemeName");
            }
        }
        *XSETTINGS_THEME.lock().unwrap_or_else(|e| e.into_inner()) = theme;
        changed();
        // Until the settings change, the daemon ends or a new one starts.
        loop {
            match conn.wait_for_event()? {
                Event::PropertyNotify(e) if e.window == owner && e.atom == settings => break,
                Event::DestroyNotify(e) if e.window == owner => break,
                Event::ClientMessage(e) if e.type_ == manager => break,
                _ => {}
            }
        }
    }
}

/// Calls `changed` whenever dconf writes a key of a desktop's interface settings (GNOME's,
/// MATE's, Cinnamon's color-scheme or gtk-theme): dconf's writer announces every change on the
/// session bus.
#[cfg(target_os = "linux")]
fn watch_dconf(changed: &dyn Fn()) -> zbus::Result<()> {
    let conn = zbus::blocking::Connection::session()?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("ca.desrt.dconf.Writer")?
        .member("Notify")?
        .build();
    for message in zbus::blocking::MessageIterator::for_match_rule(rule, &conn, None)? {
        let message = message?;
        let Ok((prefix, keys, _tag)) = message
            .body()
            .deserialize::<(String, Vec<String>, String)>()
        else {
            continue;
        };
        let interface = |path: &str| {
            [
                "/org/gnome/desktop/interface/",
                "/org/mate/desktop/interface/",
                "/org/cinnamon/desktop/interface/",
            ]
            .iter()
            .any(|dir| path.starts_with(dir) || dir.starts_with(path))
        };
        let touched = if keys.is_empty() {
            interface(&prefix)
        } else {
            keys.iter().any(|k| interface(&format!("{prefix}{k}")))
        };
        if touched {
            changed();
        }
    }
    Ok(())
}

/// A string setting from an XSETTINGS property (the freedesktop XSETTINGS format: a byte-order
/// byte, the serial, the number of settings, then each one's type, name, serial and value, all
/// padded to 4 bytes). None when it isn't there, isn't a string, or the data is cut short.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn xsettings_string(data: &[u8], key: &str) -> Option<String> {
    let big = *data.first()? == 1;
    let u16_at = |at: usize| -> Option<usize> {
        let b: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        } as usize)
    };
    let u32_at = |at: usize| -> Option<usize> {
        let b: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        } as usize)
    };
    let pad = |n: usize| n.div_ceil(4) * 4;
    let count = u32_at(8)?;
    let mut at = 12;
    for _ in 0..count {
        let kind = *data.get(at)?;
        let name_len = u16_at(at + 2)?;
        let name = data.get(at + 4..at + 4 + name_len)?;
        at += 4 + pad(name_len) + 4; // the name, then its last-change serial
        match kind {
            0 => at += 4, // an integer
            1 => {
                let len = u32_at(at)?;
                let value = data.get(at + 4..at + 4 + len)?;
                if name == key.as_bytes() {
                    return Some(String::from_utf8_lossy(value).into_owned());
                }
                at += 4 + pad(len);
            }
            2 => at += 8, // a colour: four 16-bit parts
            _ => return None,
        }
    }
    None
}

/// The desktop's accent colour and whether it asks for higher contrast (the freedesktop portal's
/// `accent-color` and `contrast`, GNOME 47+, KDE Plasma), read on a thread of their own that
/// follows their changes; `ctx` is repainted when one changes. None / false until the portal
/// answers, or when it has neither.
#[cfg(target_os = "linux")]
fn desktop_look(ctx: &egui::Context) -> (Option<egui::Color32>, bool) {
    use std::sync::Mutex;
    static LOOK: Mutex<(Option<egui::Color32>, bool)> = Mutex::new((None, false));
    static WATCH: std::sync::Once = std::sync::Once::new();
    WATCH.call_once(|| {
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("desktop-look".into())
            .spawn(move || {
                let set = |name: &str, v: &zbus::zvariant::Value| {
                    let mut look = LOOK.lock().unwrap_or_else(|e| e.into_inner());
                    let before = *look;
                    match name {
                        "accent-color" => look.0 = accent_color(v),
                        "contrast" => look.1 = color_scheme(v) == Some(1),
                        _ => {}
                    }
                    if *look != before {
                        desktop_changed(&ctx);
                    }
                };
                let watch = || -> zbus::Result<()> {
                    let conn = zbus::blocking::Connection::session()?;
                    let portal = zbus::blocking::Proxy::new(
                        &conn,
                        "org.freedesktop.portal.Desktop",
                        "/org/freedesktop/portal/desktop",
                        "org.freedesktop.portal.Settings",
                    )?;
                    let changes = portal.receive_signal("SettingChanged")?;
                    for name in ["accent-color", "contrast"] {
                        let key = ("org.freedesktop.appearance", name);
                        let now: zbus::Result<zbus::zvariant::OwnedValue> = portal
                            .call("ReadOne", &key)
                            .or_else(|_| portal.call("Read", &key));
                        if let Ok(now) = now {
                            set(name, &now);
                        }
                    }
                    for change in changes {
                        let (namespace, name, value): (String, String, zbus::zvariant::OwnedValue) =
                            change.body().deserialize()?;
                        if namespace == "org.freedesktop.appearance" {
                            set(&name, &value);
                        }
                    }
                    Ok(())
                };
                if let Err(e) = watch() {
                    tracing::info!("desktop accent: no answer from the portal ({e})");
                }
            });
    });
    *LOOK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The desktop's accent colour (the Native theme's), if it has one.
#[cfg(target_os = "linux")]
pub fn desktop_accent(ctx: &egui::Context) -> Option<egui::Color32> {
    desktop_look(ctx).0
}

/// The desktop asks for higher contrast.
#[cfg(target_os = "linux")]
pub fn desktop_contrast(ctx: &egui::Context) -> bool {
    desktop_look(ctx).1
}

/// The portal's `accent-color`: sRGB, each part from 0 to 1, unwrapped from `Read`'s variants;
/// a part out of range means none is set.
#[cfg(target_os = "linux")]
fn accent_color(v: &zbus::zvariant::Value) -> Option<egui::Color32> {
    use zbus::zvariant::Value;
    match v {
        Value::Value(inner) => accent_color(inner),
        Value::Structure(s) => {
            let parts: Vec<f64> = s
                .fields()
                .iter()
                .filter_map(|f| match f {
                    Value::F64(x) => Some(*x),
                    _ => None,
                })
                .collect();
            accent_parts(&parts)
        }
        _ => None,
    }
}

/// Three parts from 0 to 1 as a colour; None for anything else.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn accent_parts(parts: &[f64]) -> Option<egui::Color32> {
    if parts.len() != 3 || parts.iter().any(|p| !(0.0..=1.0).contains(p)) {
        return None;
    }
    let b = |p: f64| (p * 255.0).round() as u8;
    Some(egui::Color32::from_rgb(
        b(parts[0]),
        b(parts[1]),
        b(parts[2]),
    ))
}

/// macOS's accent colour (System Settings → Appearance), from `defaults read -g
/// AppleAccentColor`: -1 graphite, 0 red, 1 orange, 2 yellow, 3 green, 4 blue, 5 purple, 6 pink;
/// no value is the default (multicolour: blue). Mapped to Apple's published system colours.
///
/// The Native theme asks every 2 s, from the window's thread, so `defaults` runs on a thread of
/// its own (#222, crate::launch::Asked) and this answers what it said last, one check late; only
/// the first answer is waited for, at most half a second, so Native starts in the right colour.
#[cfg(target_os = "macos")]
pub fn mac_accent() -> Option<egui::Color32> {
    use std::time::Duration;
    /// What `defaults` said last (None inside: it couldn't be asked).
    static SAID: std::sync::LazyLock<crate::launch::Asked<(), Option<egui::Color32>>> =
        std::sync::LazyLock::new(Default::default);
    SAID.get(
        &(),
        Duration::ZERO,
        Duration::from_millis(500),
        None,
        |_| {
            let out = crate::launch::output_within(
                Command::new("defaults").args(["read", "-g", "AppleAccentColor"]),
                Duration::from_secs(2),
            );
            out.map(|out| {
                let n = String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .parse::<i32>()
                    .ok();
                mac_accent_color(n)
            })
        },
    )
    .flatten()
}

/// AppleAccentColor's number (None: not set) as the colour it stands for.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_accent_color(n: Option<i32>) -> egui::Color32 {
    let rgb = egui::Color32::from_rgb;
    match n {
        Some(-1) => rgb(0x8c, 0x8c, 0x8c),
        Some(0) => rgb(0xff, 0x3b, 0x30),
        Some(1) => rgb(0xff, 0x95, 0x00),
        Some(2) => rgb(0xff, 0xcc, 0x00),
        Some(3) => rgb(0x28, 0xcd, 0x41),
        Some(5) => rgb(0xaf, 0x52, 0xde),
        Some(6) => rgb(0xff, 0x2d, 0x55),
        _ => rgb(0x00, 0x7a, 0xff),
    }
}

/// The portal's `color-scheme` number (0 no preference, 1 dark, 2 light), unwrapped from the
/// variants `Read` adds around it.
#[cfg(target_os = "linux")]
fn color_scheme(v: &zbus::zvariant::Value) -> Option<u32> {
    match v {
        zbus::zvariant::Value::U32(n) => Some(*n),
        zbus::zvariant::Value::Value(inner) => color_scheme(inner),
        _ => None,
    }
}

/// Linux: a library winit loads when the window opens (not linked: the program starts without
/// it, and the window then can't open), with its package in each family of distributions.
#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
pub struct WindowLib {
    /// The names winit's loaders try, the usual one first.
    pub names: &'static [&'static str],
    /// Its package on Debian and Ubuntu, Fedora (and RHEL), openSUSE, Arch, Alpine.
    packages: [&'static str; 5],
}

#[cfg(target_os = "linux")]
const XKBCOMMON: WindowLib = WindowLib {
    names: &["libxkbcommon.so.0", "libxkbcommon.so"],
    packages: [
        "libxkbcommon0",
        "libxkbcommon",
        "libxkbcommon0",
        "libxkbcommon",
        "libxkbcommon",
    ],
};

/// What an X11 window (or XWayland's) needs: the keyboard (xkbcommon-dl) and Xlib's parts
/// (x11-dl), as winit opens them.
#[cfg(target_os = "linux")]
const X11_LIBS: &[WindowLib] = &[
    WindowLib {
        names: &["libxkbcommon-x11.so.0", "libxkbcommon-x11.so"],
        packages: [
            "libxkbcommon-x11-0",
            "libxkbcommon-x11",
            "libxkbcommon-x11-0",
            "libxkbcommon-x11",
            "libxkbcommon",
        ],
    },
    XKBCOMMON,
    WindowLib {
        names: &["libX11.so.6", "libX11.so"],
        packages: ["libx11-6", "libX11", "libX11-6", "libx11", "libx11"],
    },
    WindowLib {
        names: &["libX11-xcb.so.1", "libX11-xcb.so"],
        packages: [
            "libx11-xcb1",
            "libX11-xcb",
            "libX11-xcb1",
            "libx11",
            "libx11",
        ],
    },
    WindowLib {
        names: &["libXcursor.so.1", "libXcursor.so"],
        packages: [
            "libxcursor1",
            "libXcursor",
            "libXcursor1",
            "libxcursor",
            "libxcursor",
        ],
    },
    WindowLib {
        names: &["libXi.so.6", "libXi.so"],
        packages: ["libxi6", "libXi", "libXi6", "libxi", "libxi"],
    },
];

/// What a Wayland window needs: the keyboard, and Wayland's client library (winit loads it).
#[cfg(target_os = "linux")]
const WAYLAND_LIBS: &[WindowLib] = &[
    XKBCOMMON,
    WindowLib {
        names: &["libwayland-client.so.0", "libwayland-client.so"],
        packages: [
            "libwayland-client0",
            "libwayland-client",
            "libwayland-client0",
            "wayland",
            "wayland-libs-client",
        ],
    },
];

/// Linux: the display system MDM's windows use, decided once (`window_system`), and what the
/// system lacks for it.
#[cfg(target_os = "linux")]
#[derive(Debug, Default, PartialEq, Eq)]
pub struct WindowSystem {
    /// Wayland's windows, not X11's (or XWayland's).
    pub wayland: bool,
    /// X11 is chosen in a Wayland session (XWayland): winit is told so.
    x11_over_wayland: bool,
    /// X11 was MDM's choice, but these are missing: Wayland's windows instead.
    pub x11_missing: Vec<&'static WindowLib>,
    /// The chosen system lacks these: no window can open (#265).
    pub missing: Vec<&'static WindowLib>,
}

/// The windows run through X11 (XWayland, in a Wayland session) when there is one, unless
/// `MDM_WAYLAND=1` (#50). Wayland lets no program place its own windows, and MDM's menus and drop
/// lists are windows it places under what was clicked: Wayland put them in the middle of the
/// screen. Without the libraries X11's windows need, Wayland's when there is one (#265).
/// `loads`: whether a library can be loaded.
#[cfg(target_os = "linux")]
fn choose_window_system(
    wayland_session: bool,
    x11_session: bool,
    keep_wayland: bool,
    loads: impl Fn(&WindowLib) -> bool,
) -> WindowSystem {
    let lacking = |libs: &'static [WindowLib]| libs.iter().filter(|l| !loads(l)).collect();
    let mut ws = WindowSystem::default();
    if x11_session && !(wayland_session && keep_wayland) {
        ws.missing = lacking(X11_LIBS);
        if ws.missing.is_empty() {
            ws.x11_over_wayland = wayland_session;
            return ws;
        }
        if !wayland_session {
            return ws;
        }
        ws.x11_missing = std::mem::take(&mut ws.missing);
    }
    if wayland_session {
        ws.wayland = true;
        ws.missing = lacking(WAYLAND_LIBS);
    }
    ws
}

/// Linux: the display system MDM's windows use (see `choose_window_system`), decided at the
/// first question.
#[cfg(target_os = "linux")]
pub fn window_system() -> &'static WindowSystem {
    static CHOSEN: std::sync::OnceLock<WindowSystem> = std::sync::OnceLock::new();
    CHOSEN.get_or_init(|| {
        let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        choose_window_system(
            set("WAYLAND_DISPLAY"),
            set("DISPLAY"),
            std::env::var_os("MDM_WAYLAND").is_some_and(|v| v == "1"),
            |lib| lib.names.iter().any(|n| loads(n)),
        )
    })
}

/// Whether the library `name` can be loaded, as winit's loaders would load it (and unloaded
/// again at once).
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn loads(name: &str) -> bool {
    let Ok(name) = std::ffi::CString::new(name) else {
        return false;
    };
    // SAFETY: a NUL-terminated name; the handle, when there is one, is closed right after and
    // nothing of the library is used.
    let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    if handle.is_null() {
        return false;
    }
    // SAFETY: the handle dlopen just gave.
    unsafe { libc::dlclose(handle) };
    true
}

/// The hook for `NativeOptions::event_loop_builder` that opens the windows where
/// `window_system` chose; None: winit's own choice (the same).
#[cfg(target_os = "linux")]
pub fn event_loop_hook() -> Option<eframe::EventLoopBuilderHook> {
    let ws = window_system();
    if ws.x11_over_wayland {
        Some(Box::new(|b| {
            use winit::platform::x11::EventLoopBuilderExtX11;
            b.with_x11();
        }))
    } else if ws.wayland {
        Some(Box::new(|b| {
            use winit::platform::wayland::EventLoopBuilderExtWayland;
            b.with_wayland();
        }))
    } else {
        None
    }
}

/// Linux: MDM's windows are Wayland's (not X11's or XWayland's), as `window_system` decided.
#[cfg(target_os = "linux")]
pub fn on_wayland() -> bool {
    window_system().wayland
}

/// Linux, X11 and XWayland (MDM's default): a menu's window as X11 menus are (GTK's, Qt's).
/// Override-redirect: no window manager manages it, so it opens exactly where it's placed (a
/// tiling compositor, sway, centred a managed one, #267), and compositors treat it as a menu:
/// Hyprland gives it no border and doesn't slide or pop it in as it does a new window (it only
/// fades it, as any X11 menu), where a managed one came down from the top of the screen (#277).
/// Its type says which menu it is (`menu_window_type`), for the compositors that style each.
/// Never activated either way: the main window keeps the keyboard, and the menu's keys reach it
/// (chrome::popup). Native Wayland (`MDM_WAYLAND=1`) ignores both: winit 0.30 has no popup
/// windows (xdg_popup) there, so a menu is a window the compositor places and animates as any.
#[cfg(target_os = "linux")]
pub fn menu_window(
    builder: egui::ViewportBuilder,
    kind: crate::chrome::MenuKind,
) -> egui::ViewportBuilder {
    builder
        .with_override_redirect(true)
        .with_window_type(menu_window_type(kind))
}

/// The X11 window type (`_NET_WM_WINDOW_TYPE`) of a menu's window: the menu bar's are drop-down
/// menus, context menus popup menus, a drop-down list's choices a combo's.
#[cfg(target_os = "linux")]
fn menu_window_type(kind: crate::chrome::MenuKind) -> egui::X11WindowType {
    use crate::chrome::MenuKind;
    match kind {
        MenuKind::Bar => egui::X11WindowType::DropdownMenu,
        MenuKind::Context => egui::X11WindowType::PopupMenu,
        MenuKind::List => egui::X11WindowType::Combo,
    }
}

/// The family of distributions /etc/os-release (`os_release`) names: the index of its packages
/// in `WindowLib::packages`, and how it installs one.
#[cfg(target_os = "linux")]
fn installer(os_release: &str) -> Option<(usize, &'static str)> {
    let field = |key: &str| {
        os_release
            .lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches(['"', '\'']).to_lowercase())
            .unwrap_or_default()
    };
    let ids = format!("{} {}", field("ID"), field("ID_LIKE"));
    let is = |name: &str| ids.split_whitespace().any(|id| id == name);
    if is("debian") || is("ubuntu") {
        Some((0, "sudo apt install"))
    } else if is("suse") || is("opensuse") || ids.contains("opensuse") {
        Some((2, "sudo zypper install"))
    } else if is("fedora") || is("rhel") || is("centos") {
        Some((1, "sudo dnf install"))
    } else if is("arch") {
        Some((3, "sudo pacman -S"))
    } else if is("alpine") {
        Some((4, "sudo apk add"))
    } else {
        None
    }
}

/// The command that installs `libs` on the distribution /etc/os-release (`os_release`) names;
/// None for one MDM doesn't know.
#[cfg(target_os = "linux")]
fn install_command(libs: &[&WindowLib], os_release: &str) -> Option<String> {
    let (family, command) = installer(os_release)?;
    let mut packages: Vec<&str> = Vec::new();
    for lib in libs {
        if !packages.contains(&lib.packages[family]) {
            packages.push(lib.packages[family]);
        }
    }
    Some(format!("{command} {}", packages.join(" ")))
}

/// What the system lacks for the windows, and how to install it, in the interface language
/// (#265): `opening` says the window can't open, else that X11's windows fell back to Wayland's.
#[cfg(target_os = "linux")]
pub fn missing_text(libs: &[&WindowLib], opening: bool) -> String {
    use crate::i18n::{fmt, ltr, t};
    let names: Vec<&str> = libs.iter().map(|l| l.names[0]).collect();
    let names = ltr(&names.join(", "));
    let what = if opening {
        // i18n: Linux, at start, on the terminal and in a message box: a library the window needs isn't installed; {libraries} = its file names (libxkbcommon-x11.so.0…), left as they are
        t("Mister Download Manager can't open its window: this system lacks {libraries}.")
    } else {
        // i18n: Linux, at start, on the terminal: the libraries an X11 window needs aren't installed, so the window opens through Wayland instead, where menus can't be placed under what was clicked; {libraries} = their file names, left as they are
        t(
            "Mister Download Manager's X11 window needs {libraries}, which this system lacks: it opens on Wayland instead, where its menus may open away from their titles.",
        )
    };
    let what = fmt(what, &[("libraries", &names)]);
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let how = match install_command(libs, &release) {
        Some(command) => fmt(
            // i18n: Linux, after a missing library's name: {command} = the terminal command that installs it ("sudo apt install libxkbcommon-x11-0"), left as it is
            t("To install what's missing: {command}"),
            &[("command", &ltr(&command))],
        ),
        // i18n: Linux, after a missing library's name, on a distribution MDM doesn't know
        None => t("Install what's missing from your distribution's packages.").to_owned(),
    };
    format!("{what}\n{how}")
}

/// Says `text` on the terminal, in the log and, when a display is there, in a message box
/// (zenity or kdialog, the first there): MDM's own window can't open.
#[cfg(target_os = "linux")]
pub fn tell_window_failed(text: &str) {
    eprintln!("{text}");
    tracing::error!("{text}");
    let title = "Mister Download Manager";
    // A program of the desktop's draws it: without isolates (#919).
    let text = &crate::i18n::os_text(text);
    let boxes: [(&str, Vec<&str>); 2] = [
        (
            "zenity",
            vec!["--error", "--no-markup", "--title", title, "--text", text],
        ),
        ("kdialog", vec!["--title", title, "--error", text]),
    ];
    for (tool, args) in boxes {
        let shown = Command::new(tool)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // Not there, or it couldn't show (no display): the next one.
        if shown.is_ok_and(|s| s.code().is_some_and(|c| c != 255 && c != 127)) {
            return;
        }
    }
}

/// Linux: a tray host is there to show MDM's icon (a StatusNotifierWatcher on the session bus:
/// KDE, waybar's tray, GNOME's AppIndicator extension), #59. Without one (MangoWC, plain
/// window managers), closing the window would hide it where nothing can bring it back.
///
/// The window asks every frame, so the bus is asked on a thread of its own at most every 2 s
/// (crate::launch::Asked) and this answers what it said last; only the first answer is waited
/// for, at most a quarter of a second. A bus that doesn't answer reads as no tray host.
#[cfg(target_os = "linux")]
pub fn tray_host() -> bool {
    static ASKED: std::sync::LazyLock<crate::launch::Asked<(), bool>> =
        std::sync::LazyLock::new(Default::default);
    let has = || -> zbus::Result<bool> {
        let bus = zbus::blocking::connection::Builder::session()?
            .method_timeout(std::time::Duration::from_secs(5))
            .build()?;
        let dbus = zbus::blocking::fdo::DBusProxy::new(&bus)?;
        let name = zbus::names::BusName::try_from("org.kde.StatusNotifierWatcher")?;
        Ok(dbus.name_has_owner(name)?)
    };
    ASKED
        .get(
            &(),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_millis(250),
            None,
            move |_| has().unwrap_or(false),
        )
        .unwrap_or(false)
}

/// `win32::on_a_screen`: winit has no monitor list before the window exists, so a saved
/// position is trusted (the window manager keeps windows on screen on Linux and macOS).
pub fn on_a_screen(_x: i32, _y: i32) -> bool {
    true
}

// ---- File icons (fileicons.rs) -----------------------------------------------------------------

/// `win32::file_icon` on Linux: the desktop icon theme's file for a type, as its file manager
/// shows it (xdgicons.rs): the type's MIME type from shared-mime-info, its icon names, looked up
/// in the desktop's theme (read once: KDE's kdeglobals, GNOME's gsettings, GTK's settings.ini),
/// the themes it inherits, Adwaita and Breeze when installed, then hicolor. None when no theme
/// has one (fileicons.rs then draws Tango's).
#[cfg(target_os = "linux")]
pub fn file_icon(what: &crate::fileicons::What, px: u32) -> Option<crate::fileicons::Native> {
    use crate::fileicons::{Native, What};
    let (mime, icons) = icon_lookup();
    let names = match what {
        What::Folder => vec!["folder".to_string()],
        What::FolderOpen => vec!["folder-open".to_string(), "folder".to_string()],
        What::File(ext) => {
            let found = ext
                .as_ref()
                .and_then(|e| mime.mime_for(&format!("file.{e}")));
            mime.icon_names(found.unwrap_or("application/octet-stream"))
        }
    };
    icons.find(&names, px).map(Native::File)
}

/// The MIME database and the desktop's icon themes, read once: started at start-up by
/// [`prepare_file_icons`], off the window's thread (it asks `gsettings`, #222).
#[cfg(target_os = "linux")]
fn icon_lookup() -> &'static (crate::xdgicons::MimeDb, crate::xdgicons::Icons) {
    use crate::xdgicons::{Dirs, Icons, MimeDb};
    static LOOKUP: std::sync::OnceLock<(MimeDb, Icons)> = std::sync::OnceLock::new();
    LOOKUP.get_or_init(|| {
        let var = |n: &str| std::env::var(n).ok();
        let dirs = Dirs::new(
            var("HOME").as_deref(),
            var("XDG_DATA_HOME").as_deref(),
            var("XDG_DATA_DIRS").as_deref(),
        );
        let kde = var("XDG_CURRENT_DESKTOP").is_some_and(|d| d.to_uppercase().contains("KDE"));
        let theme = icon_theme(kde);
        tracing::info!("file icons: the {:?} icon theme", theme);
        let default = if kde { "breeze" } else { "Adwaita" };
        let themes = [theme.as_deref().unwrap_or(default), "Adwaita", "breeze"];
        (MimeDb::load(&dirs.data), Icons::new(&dirs.icons, &themes))
    })
}

/// Read the file icons' themes while the window opens ([`icon_lookup`]); the list's first
/// icons only wait for what is still being read.
#[cfg(target_os = "linux")]
pub fn prepare_file_icons() {
    let _ = std::thread::Builder::new()
        .name("file-icons".into())
        .spawn(|| {
            icon_lookup();
        });
}

/// The desktop's icon theme name: KDE's (`kdeglobals` [Icons] Theme) first on KDE, else GNOME's
/// (`gsettings`), then GTK's `settings.ini`.
#[cfg(target_os = "linux")]
fn icon_theme(kde: bool) -> Option<String> {
    use crate::xdgicons::ini_value;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let read = |rel: &str| std::fs::read_to_string(config.join(rel)).ok();
    let kdeglobals = || ini_value(&read("kdeglobals")?, "Icons", "Theme");
    let gnome = || {
        let out = crate::launch::output_within(
            Command::new("gsettings").args(["get", "org.gnome.desktop.interface", "icon-theme"]),
            std::time::Duration::from_secs(2),
        )?;
        let name = String::from_utf8_lossy(&out.stdout)
            .trim()
            .trim_matches('\'')
            .to_string();
        (out.status.success() && !name.is_empty()).then_some(name)
    };
    let gtk = || {
        ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"]
            .iter()
            .find_map(|f| ini_value(&read(f)?, "Settings", "gtk-icon-theme-name"))
    };
    if kde {
        kdeglobals().or_else(gnome).or_else(gtk)
    } else {
        gnome().or_else(gtk).or_else(kdeglobals)
    }
}

/// `win32::file_icon` on macOS: Finder's icon for a type (NSWorkspace), drawn at `px` pixels
/// into a bitmap of our own (AppKit picks the icon's picture made for that size).
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn file_icon(what: &crate::fileicons::What, px: u32) -> Option<crate::fileicons::Native> {
    use crate::fileicons::{Native, What};
    use objc2::AllocAnyThread as _;
    use objc2_app_kit::{NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext, NSWorkspace};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    // An extension, or a type identifier where there is none.
    let ty = match what {
        // Finder has no open folder of its own: its folder.
        What::Folder | What::FolderOpen => "public.folder".to_string(),
        What::File(Some(ext)) => ext.clone(),
        What::File(None) => "public.data".to_string(),
    };
    let side = px.clamp(1, 1024) as usize;
    objc2::rc::autoreleasepool(|_| {
        #[allow(deprecated)] // iconForContentType needs a UTType; this takes the extension as is.
        let image = NSWorkspace::sharedWorkspace().iconForFileType(&NSString::from_str(&ty));
        // SAFETY: no planes, so the bitmap allocates and owns its pixels: 8-bit RGBA, packed rows
        // (0 lets it choose the row length and bits per pixel). The colour space name is AppKit's
        // own constant.
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                side as isize,
                side as isize,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                0,
                0,
            )
        }?;
        let gc = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&gc));
        let edge = side as f64;
        image.drawInRect(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(edge, edge)));
        gc.flushGraphics();
        NSGraphicsContext::restoreGraphicsState_class();
        let row = rep.bytesPerRow() as usize;
        let data = rep.bitmapData();
        if data.is_null() || row < side * 4 {
            return None;
        }
        let mut rgba = Vec::with_capacity(side * side * 4);
        for y in 0..side {
            // SAFETY: the bitmap holds `side` rows of `row` bytes, alive while `rep` is.
            rgba.extend_from_slice(unsafe {
                std::slice::from_raw_parts(data.add(y * row), side * 4)
            });
        }
        Some(Native::Rgba {
            side: side as u32,
            rgba,
            premultiplied: true,
        })
    })
}

/// macOS hands an app what it should open (a .torrent double-clicked in Finder or dropped on
/// MDM's icon in the Dock, a magnet link clicked in a browser) as Apple events, not on its
/// command line, also to an MDM already running (#698). These receive them and send each one
/// down the command channel, as a .torrent or magnet link on Windows' or Linux's command line
/// goes (`mdm_ipc::parse_args`): Add torrent opens for it. The app's Info.plist
/// (tools/installer/build-macos.sh) tells macOS that MDM opens them.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub mod open_events {
    use std::ffi::c_void;
    use std::sync::Arc;
    use std::sync::mpsc::Sender;

    use objc2::rc::Retained;
    use objc2::{DefinedClass as _, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_foundation::{
        NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter,
        NSObject, NSString,
    };

    /// Apple event codes (four characters): open documents, get URL, and an event's direct
    /// object (the files, the link).
    const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
    const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
    const INTERNET_EVENT_CLASS: u32 = u32::from_be_bytes(*b"GURL");
    const GET_URL: u32 = u32::from_be_bytes(*b"GURL");
    const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

    pub struct Ivars {
        tx: Sender<mdm_ipc::Incoming>,
        wake: Arc<dyn Fn() + Send + Sync>,
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and OpenEvents doesn't implement Drop.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "MDMOpenEvents"]
        #[ivars = Ivars]
        pub struct OpenEvents;

        impl OpenEvents {
            /// AppKit installs its own handlers as the app starts, and dispatches the events
            /// that launched it after this notification: Apple's moment for an app's own
            /// handlers (installed earlier, AppKit's would replace them).
            #[unsafe(method(willFinishLaunching:))]
            fn will_finish_launching(&self, _note: &NSNotification) {
                let manager = NSAppleEventManager::sharedAppleEventManager();
                // SAFETY: both selectors are this class's, taking (event, reply) as the manager
                // calls them; the handler lives as long as MDM (`start`). The event codes are
                // AEEventClass and AEEventID, 32-bit codes.
                unsafe {
                    let _: () = msg_send![&*manager, setEventHandler: self,
                        andSelector: sel!(openDocuments:withReplyEvent:),
                        forEventClass: CORE_EVENT_CLASS, andEventID: OPEN_DOCUMENTS];
                    let _: () = msg_send![&*manager, setEventHandler: self,
                        andSelector: sel!(getURL:withReplyEvent:),
                        forEventClass: INTERNET_EVENT_CLASS, andEventID: GET_URL];
                }
            }

            #[unsafe(method(openDocuments:withReplyEvent:))]
            fn open_documents(
                &self,
                event: &NSAppleEventDescriptor,
                _reply: &NSAppleEventDescriptor,
            ) {
                for path in direct_object(event).map(|l| file_paths(&l)).unwrap_or_default() {
                    self.hand(path);
                }
            }

            #[unsafe(method(getURL:withReplyEvent:))]
            fn get_url(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
                if let Some(link) = direct_object(event).and_then(|d| link(&d)) {
                    self.hand(link);
                }
            }
        }
    );

    impl OpenEvents {
        /// A .torrent's path or a magnet link to the app, as its command line would.
        fn hand(&self, item: String) {
            match mdm_ipc::parse_args(std::slice::from_ref(&item)) {
                Some(cmd) => {
                    tracing::info!("opened from the system: {item}");
                    let _ = self.ivars().tx.send(mdm_ipc::Incoming { cmd, reply: None });
                    (self.ivars().wake)();
                }
                None => tracing::info!("opened from the system, not a torrent: {item}"),
            }
        }
    }

    /// Receive the open-documents and get-URL events from now on (`app` calls it on the main
    /// thread, before the window's event loop starts).
    pub fn start(tx: Sender<mdm_ipc::Incoming>, wake: Arc<dyn Fn() + Send + Sync>) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let this = OpenEvents::alloc(mtm).set_ivars(Ivars { tx, wake });
        // SAFETY: NSObject's init, on the object just allocated with its ivars set.
        let this: Retained<OpenEvents> = unsafe { msg_send![super(this), init] };
        let name = NSString::from_str("NSApplicationWillFinishLaunchingNotification");
        // SAFETY: the selector is this class's and takes the notification.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &this,
                sel!(willFinishLaunching:),
                Some(&*name),
                None,
            );
        }
        // Neither the notification centre nor the event manager keeps its observer or handler:
        // this one lives as long as MDM.
        std::mem::forget(this);
    }

    /// The app's identifier and the .torrent file type, as Info.plist names them
    /// (tools/installer/build-macos.sh).
    pub const BUNDLE_ID: &str = "io.github.anefzaoui.misterdownloadmanager";
    pub const TORRENT_TYPE: &str = "org.bittorrent.torrent";

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSSetDefaultHandlerForURLScheme(scheme: *const c_void, bundle_id: *const c_void) -> i32;
        fn LSSetDefaultRoleHandlerForContentType(
            content_type: *const c_void,
            role: u32,
            bundle_id: *const c_void,
        ) -> i32;
    }

    /// MDM the default app for magnet links and .torrent files (Torrent settings' switch). A
    /// copy started outside its app (`cargo run`) has nothing to register: the log says so.
    pub fn make_default() {
        let id = NSString::from_str(BUNDLE_ID);
        let scheme = NSString::from_str("magnet");
        let torrent = NSString::from_str(TORRENT_TYPE);
        // kLSRolesAll: viewer, editor, shell.
        const ALL_ROLES: u32 = 0xFFFF_FFFF;
        // SAFETY: NSString is toll-free bridged to CFString; the strings outlive the calls.
        let (magnet, file) = unsafe {
            (
                LSSetDefaultHandlerForURLScheme(cf(&scheme), cf(&id)),
                LSSetDefaultRoleHandlerForContentType(cf(&torrent), ALL_ROLES, cf(&id)),
            )
        };
        if magnet != 0 || file != 0 {
            tracing::info!(
                "torrents: Launch Services didn't take MDM for magnet links ({magnet}) or                  .torrent files ({file})"
            );
        }
    }

    fn cf(s: &NSString) -> *const c_void {
        (s as *const NSString).cast()
    }

    fn direct_object(event: &NSAppleEventDescriptor) -> Option<Retained<NSAppleEventDescriptor>> {
        // SAFETY: paramDescriptorForKeyword: takes an AEKeyword (a 32-bit code).
        unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] }
    }

    /// The paths of an open-documents event's files (a list, numbered from 1, of file URLs or
    /// aliases).
    pub fn file_paths(list: &NSAppleEventDescriptor) -> Vec<String> {
        (1..=list.numberOfItems())
            .filter_map(|i| list.descriptorAtIndex(i))
            .filter_map(|d| d.fileURLValue())
            .filter_map(|u| u.path())
            .map(|p| p.to_string())
            .collect()
    }

    /// A get-URL event's link.
    pub fn link(d: &NSAppleEventDescriptor) -> Option<String> {
        d.stringValue()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use objc2_foundation::NSURL;

        /// #698: the files of an open-documents event, as Finder sends them, and a magnet
        /// link's get-URL event become what MDM's command line takes.
        #[test]
        fn finder_events_become_commands() {
            let list = NSAppleEventDescriptor::listDescriptor();
            let files = [
                "/Users/me/Downloads/debian.iso.torrent",
                "/tmp/a b/ü.torrent",
            ];
            for (i, p) in files.iter().enumerate() {
                let url = NSURL::fileURLWithPath(&NSString::from_str(p));
                list.insertDescriptor_atIndex(
                    &NSAppleEventDescriptor::descriptorWithFileURL(&url),
                    i as isize + 1,
                );
            }
            let paths = file_paths(&list);
            // A file URL's path comes back as the file system spells it: `ü` decomposed (u and
            // its diaeresis), the same file to every macOS call that opens it.
            assert_eq!(
                paths,
                [
                    "/Users/me/Downloads/debian.iso.torrent",
                    "/tmp/a b/u\u{308}.torrent"
                ]
            );
            let cmd = mdm_ipc::parse_args(&paths[..1]).unwrap();
            assert_eq!(cmd.url.as_deref(), Some(files[0]));

            let magnet = "magnet:?xt=urn:btih:7acf8fb590b2060dd9c3146ef770169d593433b0&dn=x";
            let d = NSAppleEventDescriptor::descriptorWithString(&NSString::from_str(magnet));
            let got = link(&d).unwrap();
            assert_eq!(got, magnet);
            let cmd = mdm_ipc::parse_args(&[got]).unwrap();
            assert_eq!(cmd.url.as_deref(), Some(magnet));
            assert!(file_paths(&NSAppleEventDescriptor::listDescriptor()).is_empty());
        }
    }
}

// ---- Crash reports (crash.rs, mdm-crash) -------------------------------------------------------

static CRASH_HANDLER: std::sync::OnceLock<mdm_crash::crash_handler::CrashHandler> =
    std::sync::OnceLock::new();

/// Catch native crashes (SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGABRT on Linux; Mach exceptions on
/// macOS) and have `reporter`'s monitor dump this process when one comes. On Linux only the
/// monitor may read this process (ptrace).
#[allow(unsafe_code)]
pub fn attach_crash_handler(reporter: std::sync::Arc<mdm_crash::client::Reporter>) -> bool {
    use mdm_crash::crash_handler::{CrashEventResult, CrashHandler, make_crash_event};
    #[cfg(target_os = "linux")]
    let monitor = reporter.monitor_pid();
    // SAFETY: the closure runs in the signal / exception handler; it only sends the dump request
    // over the already-open socket.
    let event = unsafe { make_crash_event(move |cc| CrashEventResult::Handled(reporter.dump(cc))) };
    match CrashHandler::attach(event) {
        Ok(h) => {
            #[cfg(target_os = "linux")]
            h.set_ptracer(monitor);
            CRASH_HANDLER.set(h).is_ok()
        }
        Err(e) => {
            tracing::warn!("crash handler not attached: {e}");
            false
        }
    }
}

/// Have the monitor dump this process now (a panic, a freeze): every thread, this one asking.
/// The process goes on.
pub fn dump_now() -> bool {
    use mdm_crash::crash_handler::CrashEventResult;
    CRASH_HANDLER.get().is_some_and(|h| {
        #[cfg(target_os = "linux")]
        return matches!(
            h.simulate_signal(libc::SIGUSR2 as u32),
            CrashEventResult::Handled(true)
        );
        #[cfg(target_os = "macos")]
        return h.simulate_exception(None);
        #[allow(unreachable_code)]
        {
            let _ = (h, CrashEventResult::Handled(false));
            false
        }
    })
}

/// This thread's id as the dump knows it: the tid on Linux; on macOS none (the dump's ids are
/// Mach port names of the monitor's own; the window thread is found as the main thread).
#[allow(unsafe_code)]
pub fn thread_id() -> u32 {
    #[cfg(target_os = "linux")]
    // SAFETY: a plain query.
    return unsafe { libc::gettid() } as u32;
    #[allow(unreachable_code)]
    0
}

/// Thread `tid`'s processor time and its page faults from disk so far, from /proc (crash.rs's
/// freeze notes).
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
pub fn thread_usage(tid: u32) -> Option<crate::crash::Usage> {
    let stat = std::fs::read_to_string(format!("/proc/self/task/{tid}/stat")).ok()?;
    // The fields after the name (in parentheses, and it may hold spaces), field 3 on.
    let fields: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    let num = |field: usize| fields.get(field - 3)?.parse::<u64>().ok();
    let (majflt, utime, stime) = (num(12)?, num(14)?, num(15)?);
    // SAFETY: a plain query.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let hz = if hz > 0 { hz as u64 } else { 100 };
    Some(crate::crash::Usage {
        cpu_ms: (utime + stime) * 1000 / hz,
        faults: Some(majflt),
    })
}

/// macOS: not measured, since the window thread has no id there (`thread_id` is 0).
#[cfg(not(target_os = "linux"))]
pub fn thread_usage(_tid: u32) -> Option<crate::crash::Usage> {
    None
}

/// While the window is hidden (#72): glibc hands the heap's free pages back to the system
/// (`malloc_trim`); it keeps them otherwise. Other systems: nothing to do here, since macOS and
/// non-glibc allocators return free pages by themselves, and neither has a working set to trim
/// as Windows does.
#[allow(unsafe_code)]
pub fn trim_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: a plain call; it only releases memory the allocator already holds as free.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Move `files` to the Trash (an archive and its volumes after extracting, #37), with the
/// system's own tool: `gio trash` (GLib's, on most Linux desktops) or `trash-put`; on macOS
/// `trash` (macOS 14 and later), else the Finder. False when none could: the files are kept.
pub fn to_trash(files: &[PathBuf]) -> bool {
    if files.is_empty() {
        return true;
    }
    let run = |cmd: &mut Command| {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if cfg!(target_os = "macos") {
        if run(Command::new("trash").args(files)) {
            return true;
        }
        // The Finder moves them as a person would (it may ask once to let MDM drive it).
        let list: Vec<String> = files
            .iter()
            .map(|f| {
                let p = f
                    .display()
                    .to_string()
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"");
                format!("POSIX file \"{p}\"")
            })
            .collect();
        let script = format!(
            "tell application \"Finder\" to delete {{{}}}",
            list.join(", ")
        );
        return run(Command::new("osascript").arg("-e").arg(script));
    }
    run(Command::new("gio").arg("trash").arg("--").args(files))
        || run(Command::new("trash-put").arg("--").args(files))
}

/// Is process `pid` still running?
#[allow(unsafe_code)]
pub fn process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// The OpenGL driver in its own words, for the log and the crash reports (#280): its renderer
/// (the graphics card) and version (Mesa's carries its own: "4.6 (Core Profile) Mesa 24.0.5").
/// Windows: win32.rs.
#[allow(unsafe_code)]
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

/// End process `pid` (a frozen MDM the user chose to close from the crash report).
#[allow(unsafe_code)]
pub fn end_process(pid: u32) -> bool {
    // SAFETY: a signal to a process id we were given.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) == 0 }
}

/// The crash test's segmentation fault (MDM_CRASH_TEST=1, `/show crash-test:crash`).
#[allow(unsafe_code)]
pub fn crash_now() {
    // SAFETY: none: a write through a null pointer, on purpose, to test the crash reports.
    unsafe { std::ptr::write_volatile(std::ptr::null_mut::<u32>(), 1) };
}

/// The browsers installed, by name, with their program: the app bundle on macOS (in
/// /Applications or ~/Applications), the command on the PATH on Linux. The browser extension's
/// offer (extension.rs) opens its page in each.
pub fn browsers() -> Vec<(String, PathBuf)> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let found = |names: &[&str], dirs: &[PathBuf]| {
        names
            .iter()
            .flat_map(|n| dirs.iter().map(move |d| d.join(n)))
            .find(|p| p.exists())
    };
    let mac = cfg!(target_os = "macos");
    let dirs: Vec<PathBuf> = if mac {
        vec![PathBuf::from("/Applications"), home.join("Applications")]
    } else {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default()
    };
    // A Flatpak browser (`app.zen_browser.zen` …) isn't listed: its sandbox can't start the
    // native host, so the extension couldn't reach MDM (docs/browser-help.md). Nor is a Chromium
    // browser that runs as a Snap (Ubuntu's Chromium): it starts the host inside its sandbox,
    // which can't read MDM's data folder (~/.local/share is hidden from Snaps) and so started a
    // second, sandboxed MDM. The Snap Firefox is listed: it starts the host through the desktop
    // portal, outside its sandbox.
    mdm_ipc::browsers::BROWSERS
        .iter()
        .filter_map(|b| {
            let names = if mac { b.mac_apps } else { b.linux_cmds };
            found(names, &dirs).map(|p| (b, p))
        })
        .filter(|(b, p)| mac || b.family == mdm_ipc::browsers::Family::Gecko || !is_snap(p))
        .map(|(b, p)| (b.name.to_owned(), p))
        .collect()
}

/// Linux: whether the command `p` starts a Snap: /snap/bin's links to `snap`, or the short
/// scripts some distributions put in /usr/bin to start the Snap in a package's place.
fn is_snap(p: &Path) -> bool {
    if std::fs::canonicalize(p).is_ok_and(|real| real.ends_with("usr/bin/snap")) {
        return true;
    }
    std::fs::metadata(p).is_ok_and(|m| m.len() < 16 * 1024)
        && std::fs::read(p)
            .is_ok_and(|b| b.starts_with(b"#!") && b.windows(10).any(|w| w == b"/snap/bin/"))
}

/// Open `url` in the browser whose program is `program` (from `browsers`).
pub fn open_in(program: &Path, url: &str) {
    if cfg!(target_os = "macos") {
        spawn(Command::new("open").arg("-a").arg(program).arg(url));
    } else {
        spawn(Command::new(program).arg(url));
    }
}

/// The counterpart of win32's Mark-of-the-Web on a file being downloaded from `url`: macOS'
/// quarantine attribute (Gatekeeper checks a program before its first start, as for a browser's
/// download), and on Linux the freedesktop.org origin attributes file managers show. Through
/// `webmark::mark`, which asks Options' "Mark downloaded files as coming from the internet" first
/// (neither system has a policy like Windows' "Do not preserve zone information").
pub fn mark_downloaded(path: &Path, url: &str, referrer: Option<&str>) {
    #[cfg(target_os = "macos")]
    {
        let _ = referrer;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        set_xattr(
            path,
            "com.apple.quarantine",
            &format!("0081;{now:x};Mister Download Manager;"),
        );
        let _ = url;
    }
    #[cfg(not(target_os = "macos"))]
    {
        set_xattr(path, "user.xdg.origin.url", url);
        if let Some(r) = referrer.filter(|r| r.starts_with("http")) {
            set_xattr(path, "user.xdg.referrer.url", r);
        }
    }
}

/// One extended attribute; a file system without them is left as it is.
#[allow(unsafe_code)]
fn set_xattr(path: &Path, name: &str, value: &str) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let (Ok(p), Ok(n)) = (
        CString::new(path.as_os_str().as_bytes()),
        CString::new(name),
    ) else {
        return;
    };
    // SAFETY: both strings are NUL-terminated and live through the call; the value is a byte
    // slice with its length.
    #[cfg(target_os = "macos")]
    let _ = unsafe {
        libc::setxattr(
            p.as_ptr(),
            n.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    #[cfg(not(target_os = "macos"))]
    let _ = unsafe {
        libc::setxattr(
            p.as_ptr(),
            n.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
        )
    };
}

/// Rename `from` to `to` without ever replacing an existing entry (`ErrorKind::AlreadyExists`
/// when `to` is already there).
#[allow(unsafe_code)]
pub fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let from_c = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "interior NUL"))?;
    let to_c = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "interior NUL"))?;

    #[cfg(target_os = "macos")]
    {
        // SAFETY: both paths are valid NUL-terminated C strings alive for the call.
        let rc = unsafe { libc::renamex_np(from_c.as_ptr(), to_c.as_ptr(), libc::RENAME_EXCL) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: AT_FDCWD with two valid NUL-terminated C strings and RENAME_NOREPLACE.
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    libc::AT_FDCWD,
                    from_c.as_ptr(),
                    libc::AT_FDCWD,
                    to_c.as_ptr(),
                    libc::RENAME_NOREPLACE as libc::c_uint,
                )
            };
            if rc == 0 {
                return Ok(());
            }
            let err = std::io::Error::last_os_error();
            if !matches!(err.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL)) {
                return Err(err);
            }
        }

        fallback_rename_no_replace(from, to)
    }
}

/// Non-replacing POSIX fallback when `RENAME_NOREPLACE` is unavailable (`ENOSYS` / `EINVAL` or
/// non-Linux Unix):
/// - Directories claim `to` atomically via `create_dir(to)` (`EEXIST` -> `AlreadyExists` even if
///   `to` is an empty folder), then `fs::rename(from, to)` replaces only our own placeholder.
/// - Files link atomically via `hard_link(from, to)` (`EEXIST` -> `AlreadyExists` without ever
///   falling through to `fs::rename`), falling back to `symlink_metadata` + `fs::rename` only on
///   filesystems that reject hard links (`EXDEV` / `EOPNOTSUPP` / `EPERM`).
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn fallback_rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir(to)?;
        if let Err(e) = std::fs::rename(from, to) {
            let _ = std::fs::remove_dir(to);
            return Err(e);
        }
        return Ok(());
    }
    match std::fs::hard_link(from, to) {
        Ok(()) => {
            if let Err(e) = std::fs::remove_file(from) {
                let _ = std::fs::remove_file(to);
                return Err(e);
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => {
            if std::fs::symlink_metadata(to).is_ok() {
                return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
            }
            std::fs::rename(from, to)
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use zbus::zvariant::Value;

    use super::{WindowLib, X11_LIBS, choose_window_system, install_command, menu_window_type};

    /// #277: each menu's window says which menu it is, as GTK's and Qt's do.
    #[test]
    fn menu_windows_have_their_x11_type() {
        use crate::chrome::MenuKind;
        use egui::X11WindowType;
        assert_eq!(menu_window_type(MenuKind::Bar), X11WindowType::DropdownMenu);
        assert_eq!(
            menu_window_type(MenuKind::Context),
            X11WindowType::PopupMenu
        );
        assert_eq!(menu_window_type(MenuKind::List), X11WindowType::Combo);
        let b = super::menu_window(egui::ViewportBuilder::default(), MenuKind::Bar);
        assert_eq!(b.override_redirect, Some(true));
        assert_eq!(b.window_type, Some(X11WindowType::DropdownMenu));
    }

    const X11_KEYBOARD: &str = "libxkbcommon-x11.so.0";

    /// Every library loads but X11's keyboard.
    fn without_x11_keyboard(lib: &WindowLib) -> bool {
        lib.names[0] != X11_KEYBOARD
    }

    /// #265: without libxkbcommon-x11, an X11 session can't open the window (said, not a
    /// panic); a Wayland session opens it on Wayland instead; with everything there, X11 as
    /// before (#50).
    #[test]
    fn a_missing_x11_library_is_named_or_wayland_used() {
        let x11_only = choose_window_system(false, true, false, without_x11_keyboard);
        assert!(!x11_only.wayland);
        let names: Vec<&str> = x11_only.missing.iter().map(|l| l.names[0]).collect();
        assert_eq!(names, [X11_KEYBOARD]);

        let both = choose_window_system(true, true, false, without_x11_keyboard);
        assert!(both.wayland && both.missing.is_empty(), "{both:?}");
        assert_eq!(both.x11_missing.len(), 1);
        assert!(!both.x11_over_wayland);

        let fine = choose_window_system(true, true, false, |_| true);
        assert!(!fine.wayland && fine.x11_over_wayland && fine.missing.is_empty());
        let asked = choose_window_system(true, true, true, |_| true);
        assert!(asked.wayland && !asked.x11_over_wayland && asked.x11_missing.is_empty());

        // Nothing loads: on Wayland, its own libraries are named.
        let none = choose_window_system(true, false, false, |_| false);
        assert!(none.wayland);
        assert_eq!(none.missing[0].names[0], "libxkbcommon.so.0");
    }

    /// The package to install, by the distribution's family (/etc/os-release's ID and ID_LIKE).
    #[test]
    fn the_install_command_fits_the_distribution() {
        let keyboard = [&X11_LIBS[0]];
        let cases = [
            (
                "ID=debian
",
                Some("sudo apt install libxkbcommon-x11-0"),
            ),
            (
                "ID=linuxmint
ID_LIKE=\"ubuntu debian\"
",
                Some("sudo apt install libxkbcommon-x11-0"),
            ),
            (
                "ID=fedora
",
                Some("sudo dnf install libxkbcommon-x11"),
            ),
            (
                "ID=\"rocky\"
ID_LIKE=\"rhel centos fedora\"
",
                Some("sudo dnf install libxkbcommon-x11"),
            ),
            (
                "ID=\"opensuse-tumbleweed\"
ID_LIKE=\"opensuse suse\"
",
                Some("sudo zypper install libxkbcommon-x11-0"),
            ),
            (
                "ID=arch
",
                Some("sudo pacman -S libxkbcommon-x11"),
            ),
            (
                "ID=endeavouros
ID_LIKE=arch
",
                Some("sudo pacman -S libxkbcommon-x11"),
            ),
            (
                "ID=alpine
",
                Some("sudo apk add libxkbcommon"),
            ),
            (
                "ID=nixos
",
                None,
            ),
            ("", None),
        ];
        for (release, want) in cases {
            assert_eq!(
                install_command(&keyboard, release).as_deref(),
                want,
                "{release}"
            );
        }
        // Two libraries from one package are asked once (Arch's libx11).
        let xlib = [&X11_LIBS[2], &X11_LIBS[3]];
        assert_eq!(
            install_command(
                &xlib, "ID=arch
"
            )
            .as_deref(),
            Some("sudo pacman -S libx11")
        );
    }

    /// `ReadOne` answers the number, `Read` the number wrapped in one more variant.
    #[test]
    fn color_scheme_unwraps_the_portals_answers() {
        assert_eq!(super::color_scheme(&Value::U32(1)), Some(1));
        assert_eq!(
            super::color_scheme(&Value::Value(Box::new(Value::U32(2)))),
            Some(2)
        );
        assert_eq!(super::color_scheme(&Value::Str("dark".into())), None);
    }

    /// An XSETTINGS property as a settings daemon writes it, in either byte order: an integer,
    /// a colour and two strings.
    fn xsettings(big: bool) -> Vec<u8> {
        let n16 = |n: u16| {
            if big {
                n.to_be_bytes()
            } else {
                n.to_le_bytes()
            }
        };
        let n32 = |n: u32| {
            if big {
                n.to_be_bytes()
            } else {
                n.to_le_bytes()
            }
        };
        let mut d = vec![u8::from(big), 0, 0, 0];
        d.extend(n32(7)); // serial
        d.extend(n32(4)); // settings
        let head = |d: &mut Vec<u8>, kind: u8, name: &str| {
            d.extend([kind, 0]);
            d.extend(n16(name.len() as u16));
            d.extend(name.as_bytes());
            d.resize(d.len().div_ceil(4) * 4, 0);
            d.extend(n32(3)); // last change
        };
        head(&mut d, 0, "Net/DoubleClickTime");
        d.extend(n32(400));
        head(&mut d, 2, "Gtk/Color");
        d.extend([0; 8]);
        head(&mut d, 1, "Net/IconThemeName");
        d.extend(n32(10));
        d.extend(b"elementary\0\0"); // padded to 4
        head(&mut d, 1, "Net/ThemeName");
        d.extend(n32(12));
        d.extend(b"Adwaita-dark");
        d
    }

    #[test]
    fn xsettings_strings_are_found_in_either_byte_order() {
        for big in [false, true] {
            let d = xsettings(big);
            assert_eq!(
                super::xsettings_string(&d, "Net/ThemeName").as_deref(),
                Some("Adwaita-dark")
            );
            assert_eq!(
                super::xsettings_string(&d, "Net/IconThemeName").as_deref(),
                Some("elementary")
            );
            assert_eq!(super::xsettings_string(&d, "Net/DoubleClickTime"), None);
            assert_eq!(super::xsettings_string(&d, "Gtk/FontName"), None);
            // Cut short anywhere: nothing, no panic.
            for end in 0..d.len() {
                let _ = super::xsettings_string(&d[..end], "Net/ThemeName");
            }
        }
    }

    #[test]
    fn dark_theme_names() {
        for dark in [
            "Adwaita-dark",
            "Adwaita:dark",
            "Yaru-dark",
            "Mint-Y-Dark-Aqua",
            "Greybird-dark",
            "Breeze-Dark",
            "Materia-dark-compact",
        ] {
            assert!(super::theme_name_dark(dark), "{dark}");
        }
        for light in ["Adwaita", "Yaru", "Arc-Darker", "Greybird", "Darkroom", ""] {
            assert!(!super::theme_name_dark(light), "{light}");
        }
    }

    /// A window manager's session reads GNOME's dark settings (what its users set for GTK
    /// programs, #324); a desktop with theme settings of its own keeps to those.
    #[test]
    fn window_managers_read_gnomes_settings() {
        for wm in ["hyprland", "sway", "i3", "niri", "river", "wayfire", ""] {
            assert!(super::window_manager_session(wm), "{wm:?}");
        }
        for desktop in [
            "gnome",
            "ubuntu:gnome",
            "kde",
            "xfce",
            "x-cinnamon",
            "mate",
            "lxqt",
            "cosmic",
        ] {
            assert!(!super::window_manager_session(desktop), "{desktop:?}");
        }
    }

    #[test]
    fn gnome_color_scheme_and_gtk_settings() {
        assert_eq!(super::scheme_word("prefer-dark"), Some(true));
        assert_eq!(super::scheme_word("prefer-light"), Some(false));
        assert_eq!(super::scheme_word("default"), None);
        let ini = |body: &str| format!("[Settings]\n{body}\n");
        assert_eq!(
            super::gtk_settings_dark(&ini("gtk-application-prefer-dark-theme=1")),
            Some(true)
        );
        assert_eq!(
            super::gtk_settings_dark(&ini(
                "gtk-application-prefer-dark-theme=false\ngtk-theme-name=Arc-Dark"
            )),
            Some(true)
        );
        assert_eq!(
            super::gtk_settings_dark(&ini("gtk-theme-name=Adwaita")),
            Some(false)
        );
        assert_eq!(
            super::gtk_settings_dark(&ini("gtk-font-name=Sans 10")),
            None
        );
    }

    /// The child of [`show_in_folder_never_waits_for_the_file_manager`]: the file it shows.
    const SHOW_CHILD: &str = "MDM_TEST_SHOW_IN_FOLDER_CHILD";

    /// #222: a file manager that never answers ShowItems (here a `gdbus` that hangs for 30 s)
    /// leaves `show_in_folder` returning at once, and the folder is opened instead after
    /// SHOW_ITEMS_LIMIT (here an `xdg-open` that notes what it was given). PATH is the test's
    /// own, so the test runs itself again as a child with it (no `set_var` in a test).
    #[test]
    fn show_in_folder_never_waits_for_the_file_manager() {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};
        if let Some(file) = std::env::var_os(SHOW_CHILD) {
            let file = std::path::PathBuf::from(file);
            let start = Instant::now();
            super::show_in_folder(&file);
            println!(
                "show_in_folder returned after {} ms",
                start.elapsed().as_millis()
            );
            // The fallback runs on its thread: wait for it before the child ends.
            let opened = file.with_file_name("opened.txt");
            let until = Instant::now() + Duration::from_secs(15);
            while !opened.exists() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(50));
            }
            return;
        }
        let root = std::env::temp_dir().join(format!("mdm-show-222-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (bin, folder) = (root.join("bin"), root.join("Downloads"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("done.zip");
        std::fs::write(&file, b"test").unwrap();
        let opened = folder.join("opened.txt");
        let tool = |name: &str, body: String| {
            let p = bin.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        tool("gdbus", "exec sleep 30".into());
        tool("xdg-open", format!("echo \"$1\" > '{}'", opened.display()));
        // Only these tools and the system's (no Windows programs under WSL, no real opener).
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let start = Instant::now();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "unix::tests::show_in_folder_never_waits_for_the_file_manager",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(SHOW_CHILD, &file)
            .env("PATH", path)
            .output()
            .unwrap();
        let took = start.elapsed();
        let text = String::from_utf8_lossy(&out.stdout);
        // libtest prints the test's name on the same line, before it.
        let returned = text
            .split("show_in_folder returned after ")
            .nth(1)
            .and_then(|l| l.split(" ms").next()?.parse::<u64>().ok())
            .unwrap_or_else(|| panic!("the child said nothing: {text}"));
        assert!(
            returned < 500,
            "show_in_folder waited {returned} ms for gdbus"
        );
        let shown = std::fs::read_to_string(&opened).unwrap_or_default();
        assert_eq!(
            shown.trim(),
            folder.display().to_string(),
            "the folder wasn't opened"
        );
        // The folder opened after the limit, not after gdbus's 30 s.
        assert!(took < Duration::from_secs(20), "took {took:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod host_copy_tests {
    use super::{HOST, MANIFEST, install_host_copy, registration_in, runs};
    use crate::extension::Registration;
    use mdm_ipc::browsers::Family;
    use std::path::Path;

    fn program(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// #253: the AppImage's helper is copied out of its mount, executable, with the AppImage's
    /// path beside it; a copy that went missing is made again at the next start.
    #[test]
    fn the_helper_is_copied_and_repaired() {
        let t = tempfile::tempdir().unwrap();
        let from = program(t.path(), "mount-host", "echo one");
        let dir = t.path().join("data/browser-host");
        let image = Path::new("/home/u/Apps/MDM.AppImage");
        let copy = install_host_copy(&from, &dir, image).unwrap();
        assert_eq!(copy, dir.join(HOST));
        assert!(runs(&copy));
        assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&from).unwrap());
        assert_eq!(
            std::fs::read_to_string(dir.join("app-path")).unwrap(),
            "/home/u/Apps/MDM.AppImage"
        );
        // Moved by an AppImage manager, and the copy deleted: both put right.
        std::fs::remove_file(&copy).unwrap();
        let moved = Path::new("/home/u/Applications/mdm.appimage");
        install_host_copy(&from, &dir, moved).unwrap();
        assert!(runs(&copy));
        assert_eq!(
            std::fs::read_to_string(dir.join("app-path")).unwrap(),
            moved.to_str().unwrap()
        );
        // Not executable any more: made so again.
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        assert!(!runs(&copy));
        install_host_copy(&from, &dir, moved).unwrap();
        assert!(runs(&copy));
        // Nothing temporary is left behind.
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(left.len(), 2, "{left:?}");
    }

    /// #253: a browser still runs the old copy (writing over a running program fails with "Text
    /// file busy"): the new one replaces it all the same, and the old one keeps running.
    #[test]
    fn the_helper_is_replaced_while_a_browser_runs_it() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("browser-host");
        std::fs::create_dir_all(&dir).unwrap();
        // A real program (a script would run as the shell, its file only read). The shell, which
        // runs under any name: Ubuntu 26.04's `sleep` is one program for every core tool, telling
        // them apart by the name it's started under, so a copy of it named after the helper ended
        // at once. `; :` keeps the shell from replacing itself with `sleep`.
        let old = dir.join(HOST);
        std::fs::copy("/bin/sh", &old).unwrap();
        let mut running = std::process::Command::new(&old)
            .args(["-c", "sleep 30; :"])
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(running.try_wait().unwrap().is_none(), "the old copy runs");
        assert!(
            std::fs::write(&old, b"x").is_err() || cfg!(target_os = "macos"),
            "Linux refuses to write over a running program"
        );
        let from = program(t.path(), "new-host", "echo two");
        let copy = install_host_copy(&from, &dir, Path::new("/x.AppImage")).unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&from).unwrap());
        assert!(
            running.try_wait().unwrap().is_none(),
            "the old copy still runs"
        );
        let _ = running.kill();
        let _ = running.wait();
    }

    #[test]
    fn a_copy_that_cant_be_made_says_why() {
        let t = tempfile::tempdir().unwrap();
        let err = install_host_copy(
            &t.path().join("no-such-host"),
            &t.path().join("browser-host"),
            Path::new("/x.AppImage"),
        )
        .unwrap_err();
        assert!(err.contains("no-such-host"), "{err}");
    }

    /// Test browser integration: every user folder must have the manifest, naming a helper that
    /// runs; a manifest naming a missing helper is said as such (#253).
    #[test]
    fn registration_reads_the_manifests_and_their_helper() {
        let t = tempfile::tempdir().unwrap();
        let helper = program(t.path(), HOST, "true");
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        for d in [&a, &b] {
            std::fs::create_dir_all(d).unwrap();
        }
        let write = |dir: &Path, path: &Path| {
            let json =
                serde_json::json!({ "name": "com.misterdownloadmanager.host", "path": path });
            std::fs::write(dir.join(MANIFEST), json.to_string()).unwrap();
        };
        let dirs = || vec![(a.clone(), Family::Chromium), (b.clone(), Family::Gecko)];
        assert_eq!(registration_in(vec![], false), Registration::NoBrowser);
        assert_eq!(registration_in(dirs(), false), Registration::Missing);
        write(&a, &helper);
        assert_eq!(
            registration_in(dirs(), false),
            Registration::Missing,
            "one browser's folder lacks it"
        );
        assert_eq!(
            registration_in(dirs(), true),
            Registration::Ready,
            "everyone's: one is enough"
        );
        write(&b, &helper);
        assert_eq!(registration_in(dirs(), false), Registration::Ready);
        let gone = t.path().join("gone/misterdownloadmanager-nmhost");
        write(&b, &gone);
        assert_eq!(
            registration_in(dirs(), false),
            Registration::HelperMissing(gone.clone())
        );
        assert_eq!(registration_in(dirs(), true), Registration::Ready);
    }
}

#[cfg(test)]
mod startup_tests {
    use super::{entry_program, startup_entry, startup_outdated};
    use std::path::Path;

    /// The entries versions before 0.5.0 wrote: the program alone.
    fn old_entry(exe: &str, macos: bool) -> String {
        if macos {
            format!(
                "<plist version=\"1.0\">\n<dict>\n  <key>ProgramArguments</key><array>\
                 <string>{exe}</string></array>\n  <key>RunAtLoad</key><true/>\n</dict>\n</plist>\n"
            )
        } else {
            format!("[Desktop Entry]\nType=Application\nExec=\"{exe}\"\nTerminal=false\n")
        }
    }

    /// The autostart entry and the LaunchAgent start MDM with `/startup` (#226), and name the
    /// program as it is, quoting and all.
    #[test]
    fn the_startup_entries_start_mdm_in_the_tray() {
        for exe in [
            "/home/ana/Apps/Mister Download Manager.AppImage",
            "/opt/mdm/misterdownloadmanager",
            "/Applications/Mister Download Manager.app/Contents/MacOS/misterdownloadmanager",
            "/home/a\"b$c/m&d<m>",
        ] {
            for macos in [false, true] {
                let text = startup_entry(Path::new(exe), macos);
                assert!(text.contains("/startup"), "{text}");
                assert_eq!(entry_program(&text, macos).as_deref(), Some(exe), "{text}");
                assert!(!startup_outdated(&text, Path::new(exe), macos));
            }
        }
        let desktop = startup_entry(Path::new("/opt/mdm/misterdownloadmanager"), false);
        assert!(desktop.contains("Exec=\"/opt/mdm/misterdownloadmanager\" /startup\n"));
        let plist = startup_entry(Path::new("/opt/mdm/misterdownloadmanager"), true);
        assert!(plist.contains(
            "<array><string>/opt/mdm/misterdownloadmanager</string><string>/startup</string></array>"
        ));
    }

    /// An entry of a version before 0.5.0 for this program is written again; another copy's is not.
    #[test]
    fn old_startup_entries_are_rewritten() {
        let exe = "/home/ana/Apps/Mister Download Manager.AppImage";
        for macos in [false, true] {
            assert!(startup_outdated(
                &old_entry(exe, macos),
                Path::new(exe),
                macos
            ));
            assert!(!startup_outdated(
                &old_entry("/elsewhere/mdm.AppImage", macos),
                Path::new(exe),
                macos
            ));
        }
        assert!(!startup_outdated("", Path::new(exe), false));
    }
}

// Sign-in secrets in the system's keychain (#44, secrets.rs). Linux: the Secret Service
// (GNOME Keyring, KeePassXC, KDE's wallet in its Secret Service mode) over D-Bus, its default
// collection only, and only while it is unlocked: MDM never unlocks it nor answers a prompt
// (desktops that log in on their own often have a locked keyring, #164), so a locked or missing
// keyring leaves the secrets in the owner-only settings. macOS: the login keychain's generic
// passwords (SecItemAdd / SecItemCopyMatching, through security-framework).

#[cfg(target_os = "linux")]
pub use secret_service::{
    delete as keychain_delete, list as keychain_list, probe as keychain_probe,
    read as keychain_read, write as keychain_write,
};

#[cfg(target_os = "macos")]
pub use mac_keychain::{
    delete as keychain_delete, list as keychain_list, probe as keychain_probe,
    read as keychain_read, write as keychain_write,
};

#[cfg(target_os = "linux")]
mod secret_service {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

    use crate::secrets::Fault;

    const BUS: &str = "org.freedesktop.secrets";
    const ROOT: &str = "/org/freedesktop/secrets";
    const SERVICE: &str = "org.freedesktop.Secret.Service";
    const COLLECTION: &str = "org.freedesktop.Secret.Collection";
    const ITEM: &str = "org.freedesktop.Secret.Item";
    const SESSION: &str = "org.freedesktop.Secret.Session";
    /// Groups MDM's entries in Seahorse and the like.
    const SCHEMA: &str = "io.github.anefzaoui.misterdownloadmanager.Secret";

    /// The D-Bus error's kind; its text (the service's own words) is never kept.
    fn fault(e: zbus::Error) -> Fault {
        let name = match &e {
            zbus::Error::MethodError(name, _, _) => name.as_str().to_owned(),
            zbus::Error::FDO(f) => zbus::DBusError::name(&**f).as_str().to_owned(),
            zbus::Error::InputOutput(_)
            | zbus::Error::Address(_)
            | zbus::Error::Connection(..)
            | zbus::Error::Handshake(_) => return Fault::Missing,
            _ => return Fault::Failed("D-Bus call failed".into()),
        };
        if name == "org.freedesktop.Secret.Error.IsLocked" {
            Fault::Locked
        } else if name.starts_with("org.freedesktop.DBus.Error.") {
            // No such service, none to start, no such object or method: no keyring.
            Fault::Missing
        } else {
            Fault::Failed(name)
        }
    }

    /// The session bus, once (a call answers within 5 s or is taken as no keyring).
    fn connection() -> Result<Connection, Fault> {
        static CONN: Mutex<Option<Connection>> = Mutex::new(None);
        let mut held = CONN.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = &*held {
            return Ok(c.clone());
        }
        let conn = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.method_timeout(std::time::Duration::from_secs(5)).build())
            .map_err(|_| Fault::Missing)?;
        *held = Some(conn.clone());
        Ok(conn)
    }

    fn proxy<'a>(conn: &Connection, path: &'a str, iface: &'a str) -> Result<Proxy<'a>, Fault> {
        zbus::blocking::proxy::Builder::<Proxy<'a>>::new(conn)
            .destination(BUS)
            .and_then(|b| b.path(path))
            .and_then(|b| b.interface(iface))
            .map(|b| b.cache_properties(zbus::proxy::CacheProperties::No))
            .and_then(|b| b.build())
            .map_err(fault)
    }

    /// A session with the service ("plain": the secret crosses only the user's own session bus),
    /// closed when dropped.
    struct Session {
        conn: Connection,
        path: OwnedObjectPath,
    }

    impl Session {
        fn open() -> Result<Session, Fault> {
            let conn = connection()?;
            let (_, path): (OwnedValue, OwnedObjectPath) = proxy(&conn, ROOT, SERVICE)?
                .call("OpenSession", &("plain", Value::from("")))
                .map_err(fault)?;
            Ok(Session { conn, path })
        }

        fn path(&self) -> ObjectPath<'_> {
            self.path.as_ref()
        }

        /// The default collection, only while it is unlocked (never unlocked from here).
        fn collection(&self) -> Result<OwnedObjectPath, Fault> {
            let path: OwnedObjectPath = proxy(&self.conn, ROOT, SERVICE)?
                .call("ReadAlias", &("default",))
                .map_err(fault)?;
            if path.as_str() == "/" {
                return Err(Fault::Missing);
            }
            let locked: bool = proxy(&self.conn, path.as_str(), COLLECTION)?
                .get_property("Locked")
                .map_err(fault)?;
            if locked {
                return Err(Fault::Locked);
            }
            Ok(path)
        }

        /// MDM's items of `service` (one account's, or all): (unlocked, locked).
        fn find(
            &self,
            service: &str,
            account: Option<&str>,
        ) -> Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>), Fault> {
            let mut attrs = HashMap::from([("service", service)]);
            if let Some(account) = account {
                attrs.insert("account", account);
            }
            proxy(&self.conn, ROOT, SERVICE)?
                .call("SearchItems", &(attrs,))
                .map_err(fault)
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            if let Ok(p) = proxy(&self.conn, self.path.as_str(), SESSION) {
                let _ = p.call::<_, _, ()>("Close", &());
            }
        }
    }

    pub fn probe(_service: &str) -> Result<(), Fault> {
        Session::open()?.collection().map(|_| ())
    }

    pub fn read(service: &str, account: &str) -> Result<Option<String>, Fault> {
        let s = Session::open()?;
        let (unlocked, locked) = s.find(service, Some(account))?;
        let Some(item) = unlocked.first() else {
            return if locked.is_empty() {
                Ok(None)
            } else {
                Err(Fault::Locked)
            };
        };
        let (_, _, value, _): (OwnedObjectPath, Vec<u8>, Vec<u8>, String) =
            proxy(&s.conn, item.as_str(), ITEM)?
                .call("GetSecret", &(s.path(),))
                .map_err(fault)?;
        String::from_utf8(value)
            .map(Some)
            .map_err(|_| Fault::Failed("not UTF-8".into()))
    }

    pub fn write(service: &str, account: &str, label: &str, secret: &str) -> Result<(), Fault> {
        let s = Session::open()?;
        let collection = s.collection()?;
        let attrs = HashMap::from([
            ("service", service),
            ("account", account),
            ("xdg:schema", SCHEMA),
        ]);
        let props = HashMap::from([
            ("org.freedesktop.Secret.Item.Label", Value::from(label)),
            ("org.freedesktop.Secret.Item.Attributes", Value::from(attrs)),
        ]);
        let value = (
            s.path(),
            Vec::<u8>::new(),
            secret.as_bytes(),
            "text/plain; charset=utf8",
        );
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) =
            proxy(&s.conn, collection.as_str(), COLLECTION)?
                .call("CreateItem", &(props, value, true))
                .map_err(fault)?;
        // It wants to ask the user (to unlock): never done from here.
        if prompt.as_str() != "/" {
            return Err(Fault::Locked);
        }
        Ok(())
    }

    pub fn delete(service: &str, account: &str) -> Result<(), Fault> {
        let s = Session::open()?;
        let (unlocked, locked) = s.find(service, Some(account))?;
        if unlocked.is_empty() && !locked.is_empty() {
            return Err(Fault::Locked);
        }
        for item in unlocked {
            let prompt: OwnedObjectPath = proxy(&s.conn, item.as_str(), ITEM)?
                .call("Delete", &())
                .map_err(fault)?;
            if prompt.as_str() != "/" {
                return Err(Fault::Locked);
            }
        }
        Ok(())
    }

    /// The accounts of `service`'s unlocked items.
    pub fn list(service: &str) -> Result<Vec<String>, Fault> {
        let s = Session::open()?;
        let (unlocked, _) = s.find(service, None)?;
        let mut accounts = Vec::new();
        for item in unlocked {
            let attrs: OwnedValue = proxy(&s.conn, item.as_str(), ITEM)?
                .get_property("Attributes")
                .map_err(fault)?;
            let attrs = HashMap::<String, String>::try_from(attrs)
                .map_err(|_| Fault::Failed("attributes".into()))?;
            if attrs.get("service").is_some_and(|s| s == service)
                && let Some(account) = attrs.get("account")
            {
                accounts.push(account.clone());
            }
        }
        Ok(accounts)
    }
}

#[cfg(target_os = "macos")]
mod mac_keychain {
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
    use security_framework::passwords::{
        PasswordOptions, delete_generic_password, generic_password, set_generic_password_options,
    };

    use crate::secrets::Fault;

    const ITEM_NOT_FOUND: i32 = -25300;
    const INTERACTION_NOT_ALLOWED: i32 = -25308;
    const NO_SUCH_KEYCHAIN: i32 = -25294;
    const NOT_AVAILABLE: i32 = -25291;

    fn fault(e: security_framework::base::Error) -> Fault {
        match e.code() {
            INTERACTION_NOT_ALLOWED => Fault::Locked,
            NO_SUCH_KEYCHAIN | NOT_AVAILABLE => Fault::Missing,
            code => Fault::Failed(format!("Security error {code}")),
        }
    }

    pub fn probe(service: &str) -> Result<(), Fault> {
        list(service).map(|_| ())
    }

    pub fn read(service: &str, account: &str) -> Result<Option<String>, Fault> {
        match generic_password(PasswordOptions::new_generic_password(service, account)) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| Fault::Failed("not UTF-8".into())),
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(fault(e)),
        }
    }

    pub fn write(service: &str, account: &str, label: &str, secret: &str) -> Result<(), Fault> {
        let mut options = PasswordOptions::new_generic_password(service, account);
        options.set_label(label);
        set_generic_password_options(secret.as_bytes(), options).map_err(fault)
    }

    pub fn delete(service: &str, account: &str) -> Result<(), Fault> {
        match delete_generic_password(service, account) {
            Err(e) if e.code() != ITEM_NOT_FOUND => Err(fault(e)),
            _ => Ok(()),
        }
    }

    /// The accounts of `service`'s generic passwords (their attributes only, not their data).
    pub fn list(service: &str) -> Result<Vec<String>, Fault> {
        let found = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(service)
            .load_attributes(true)
            .limit(Limit::All)
            .search();
        match found {
            Ok(items) => Ok(items
                .iter()
                .filter_map(|i| i.simplify_dict()?.get("acct").cloned())
                .collect()),
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(Vec::new()),
            Err(e) => Err(fault(e)),
        }
    }
}

// ---- waking the computer for a queue (#142) -----------------------------------------------

/// Linux: the clock chip's wake alarm, in seconds since 1970. Root's alone unless an
/// administrator lets users write it (docs/queues-and-scheduler.md shows a tmpfiles.d line).
/// Without it nothing else wakes a sleeping Linux for a program: a systemd user timer's
/// `WakeSystem=` needs the system manager, rtcwake needs root, alarm timers CAP_WAKE_ALARM.
#[cfg(target_os = "linux")]
const WAKEALARM: &str = "/sys/class/rtc/rtc0/wakealarm";

/// Whether MDM may wake this computer.
pub fn wake_support() -> crate::wake::Support {
    #[cfg(target_os = "linux")]
    {
        use crate::wake::Support;
        let path = Path::new(WAKEALARM);
        if !path.exists() {
            return Support::NoAlarm;
        }
        // Opening it for writing changes nothing: it only asks whether we may.
        if std::fs::OpenOptions::new().write(true).open(path).is_ok() {
            Support::Ready
        } else {
            Support::NeedsAdmin
        }
    }
    // macOS: `pmset schedule wake` and IOPMSchedulePowerEvent need an administrator, and MDM
    // never asks for one by itself.
    #[cfg(target_os = "macos")]
    {
        crate::wake::Support::MacAdmin
    }
}

/// Seconds to add to a system time to have the clock chip's: the chip may keep local time
/// (/etc/adjtime: LOCAL), or have drifted from the system's clock (a virtual machine's was a
/// minute and a half behind). The alarm is set in the chip's time, as rtcwake does.
#[cfg(target_os = "linux")]
fn rtc_offset() -> i64 {
    let chip = std::fs::read_to_string("/sys/class/rtc/rtc0/since_epoch")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok());
    match chip {
        Some(chip) => chip - chrono::Utc::now().timestamp(),
        None => 0,
    }
}

/// Wake the computer at `at` (seconds since 1970, UTC), or cancel MDM's wake (None).
#[cfg(target_os = "linux")]
pub fn wake_at(at: Option<i64>) -> Result<(), String> {
    // The alarm MDM set (in the chip's time), to tell it from another program's: kept in the
    // data folder, so an alarm left by an MDM that was killed (or signed out of) is still ours.
    let file = mdm_ipc::data_dir().map(|d| d.join("wake-alarm"));
    let mut ours = file
        .as_ref()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|s| s.trim().parse::<i64>().ok());
    let keep = |v: Option<i64>| {
        if let Some(f) = &file {
            let _ = match v {
                Some(v) => std::fs::write(f, v.to_string()),
                None => std::fs::remove_file(f),
            };
        }
    };
    let set_now = std::fs::read_to_string(WAKEALARM)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok());
    let want = at.map(|t| t + rtc_offset());
    // The same alarm within a couple of seconds (the two clocks are read a moment apart).
    let close = |a: i64, b: i64| (a - b).abs() <= 2;
    match (set_now, want) {
        (Some(now), Some(w)) if close(now, w) => {
            if ours != Some(now) {
                keep(Some(now));
            }
            return Ok(());
        }
        (None, None) => {
            if ours.is_some() {
                keep(None);
            }
            return Ok(());
        }
        _ => {}
    }
    // There is one alarm: one that another program (rtcwake, a script) set stays theirs.
    if let Some(theirs) = set_now
        && ours.is_none_or(|o| !close(o, theirs))
    {
        if want.is_none() {
            return Ok(());
        }
        return Err(crate::i18n::t(
            // i18n: the Scheduler, under "Wake the computer to run this queue" (Linux): the computer's one wake alarm is already set by something else
            "Another program has set the computer's wake alarm",
        )
        .to_owned());
    }
    let write = |v: &str| std::fs::write(WAKEALARM, v).map_err(|e| e.to_string());
    // An alarm is replaced by clearing it first (writing over it is refused).
    if set_now.is_some() {
        write("0")?;
    }
    ours = None;
    if let Some(v) = want {
        write(&v.to_string())?;
        ours = Some(v);
    }
    keep(ours);
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn wake_at(at: Option<i64>) -> Result<(), String> {
    match at {
        None => Ok(()),
        Some(_) => Err("macOS lets only an administrator schedule a wake".to_owned()),
    }
}

/// Keep the computer from sleeping while `reason` is given; None lets it sleep again as the
/// system's settings say. Linux: logind's sleep and idle inhibitor, held by its file
/// descriptor (`systemd-inhibit --list` shows it with its reason); a user in the active
/// session may take it without a password. False when logind refused, or didn't answer within
/// 25 s. Called on wake::Holder's thread, never the window's: logind can be slow right after a
/// resume.
#[cfg(target_os = "linux")]
pub fn keep_awake(reason: Option<&str>) -> bool {
    use std::sync::Mutex;
    static LOCK: Mutex<Option<zbus::zvariant::OwnedFd>> = Mutex::new(None);
    let mut held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Closing the descriptor ends the inhibitor.
    *held = None;
    let Some(reason) = reason else {
        return true;
    };
    let take = || -> zbus::Result<zbus::zvariant::OwnedFd> {
        let conn = zbus::blocking::connection::Builder::system()?
            .method_timeout(std::time::Duration::from_secs(25))
            .build()?;
        let logind = zbus::blocking::Proxy::new(
            &conn,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
        )?;
        logind.call(
            "Inhibit",
            &("sleep:idle", "Mister Download Manager", reason, "block"),
        )
    };
    match take() {
        Ok(fd) => {
            *held = Some(fd);
            true
        }
        Err(e) => {
            tracing::warn!("wake: logind's inhibitor refused ({e})");
            false
        }
    }
}

/// macOS: `caffeinate -i` (no idle sleep) for as long as MDM runs, ended when not needed.
#[cfg(target_os = "macos")]
pub fn keep_awake(reason: Option<&str>) -> bool {
    use std::sync::Mutex;
    static CHILD: Mutex<Option<std::process::Child>> = Mutex::new(None);
    let mut held = CHILD.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut c) = held.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    if reason.is_some() {
        *held = Command::new("caffeinate")
            .args(["-i", "-w", &std::process::id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
        return held.is_some();
    }
    true
}
