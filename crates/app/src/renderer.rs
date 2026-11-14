//! What draws the windows (#89, #206): OpenGL (eframe's glow), or the system's own graphics API
//! through eframe's wgpu with that one backend only ([`GPU`]): Direct3D 12 on Windows, Vulkan on
//! Linux (Wayland and X11 windows alike), Metal on macOS. View → Draw with picks one, from the
//! next start. Windows draws with Direct3D by default (#89); Linux and macOS keep OpenGL by
//! default until Vulkan and Metal have clean reports from real graphics cards (#206), as
//! Direct3D had before it became Windows' default.
//!
//! The windows always open: when the chosen way can't start (no driver for it, a virtual
//! machine, Remote Desktop), the next is tried, and the reason is logged. A start with the
//! system's API that ends before its first frame (a crash or a kill inside the driver) is
//! remembered, and so is one whose first frame doesn't come within [`FIRST_FRAME_WITHIN`] (MDM
//! starts again, the next way): the next starts pass over it until that API is chosen again,
//! and View → Draw with says why. A graphics card lost while MDM runs (a driver update or reset)
//! starts MDM again; a second loss soon after counts as the API failing. The same code does all
//! of this on every system: only [`GPU`] and the backend in `gpu::options` differ.
//!
//! Windows: a card without Direct3D 12 draws with OpenGL, and with Direct3D 12 on Microsoft's
//! software renderer (WARP) once OpenGL fails here (#847), remembered (`opengl-unfit.txt`) until
//! OpenGL is chosen again: it can't start, it is too old to draw MDM ([`gl_unfit`], #835), its
//! first frame doesn't come within [`FIRST_FRAME_WITHIN`], its last start ended before its first
//! frame, or the window froze inside its driver ([`gl_driver_frame`]: the crash report's window
//! reads that from the freeze's stack; the cards of #847 froze or crashed there while showing a
//! frame). WARP draws right on those cards, but keeps the processor busier, so OpenGL comes
//! first. WARP failing too is remembered apart (`direct3d-software-failed.txt`), and OpenGL draws.
//! Linux and macOS remember no OpenGL failure ([`WATCH_GL`]): their OpenGL is their default, and
//! their way out is Vulkan or Metal, chosen in View → Draw with.
//!
//! Testing overrides, in this order: `/renderer <name>` on the command line, then
//! `MDM_RENDERER=<name>`. Names: `opengl` (`gl`); `direct3d` (`d3d`) on Windows, `vulkan` (`vk`)
//! on Linux, `metal` on macOS, or `gpu` for whichever this system has; `warp` (Windows),
//! `vulkan-software` (`lavapipe`, Linux) or `software`: the system's API drawn by a software
//! renderer (Microsoft's WARP, Mesa's lavapipe), as on a computer without a graphics driver.
//! Another system's name is ignored. Off-screen test copies also take `MDM_TEST_GL_VERSION=2.1`:
//! the version OpenGL says it is, as an old card's driver does; `MDM_TEST_NO_HARDWARE=1` (no
//! card's driver), `MDM_TEST_NO_SOFTWARE=1` (no software renderer either),
//! `MDM_TEST_NO_FIRST_FRAME=opengl` (only OpenGL's first frame never comes) and
//! `MDM_TEST_GL_STALL=1` (the window stalls once while OpenGL draws, as in a driver's present).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// A choice of View → Draw with, as saved in the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Api {
    #[serde(rename = "opengl")]
    OpenGl,
    #[serde(rename = "direct3d")]
    Direct3D,
    #[serde(rename = "vulkan")]
    Vulkan,
    #[serde(rename = "metal")]
    Metal,
}

impl Api {
    /// Its name in the log and the crash reports (the menu's are translated texts, app.rs).
    pub fn name(self) -> &'static str {
        match self {
            Api::OpenGl => "OpenGL",
            Api::Direct3D => "Direct3D 12",
            Api::Vulkan => "Vulkan",
            Api::Metal => "Metal",
        }
    }

    /// The first part of its files' names in the data folder (`direct3d-failed.txt`…).
    fn key(self) -> &'static str {
        match self {
            Api::OpenGl => "opengl",
            Api::Direct3D => "direct3d",
            Api::Vulkan => "vulkan",
            Api::Metal => "metal",
        }
    }
}

/// The system's own graphics API, through wgpu (build.rs sets `gpu` where wgpu is built):
/// Direct3D 12 on Windows, Vulkan on Linux, Metal on macOS. None elsewhere (the BSDs: OpenGL
/// only).
pub const GPU: Option<Api> = if !cfg!(gpu) {
    None
} else if cfg!(windows) {
    Some(Api::Direct3D)
} else if cfg!(target_os = "macos") {
    Some(Api::Metal)
} else {
    Some(Api::Vulkan)
};

/// What draws when the user chose nothing (the settings' `renderer: None`): Direct3D on Windows
/// (#89), OpenGL elsewhere (#206: Vulkan and Metal are choices until real cards have tried them).
pub const DEFAULT: Api = if cfg!(windows) {
    Api::Direct3D
} else {
    Api::OpenGl
};

/// What the setting means here: OpenGL, or this system's API; [`DEFAULT`] when nothing was
/// chosen, or another system's API was (a settings file brought from another computer).
pub fn chosen(setting: Option<Api>) -> Api {
    setting
        .filter(|&a| a == Api::OpenGl || Some(a) == GPU)
        .unwrap_or(DEFAULT)
}

/// OpenGL's starts are watched and its failures remembered, so the next starts draw with the
/// system's API on its software renderer instead: Windows only (#847: WARP, part of every
/// Windows 10 and 11, draws MDM right on the cards whose OpenGL froze, crashed or drew garbage).
/// Linux and macOS keep their plans: OpenGL is their default (#206), Mesa's software Vulkan is
/// no better a way out than its OpenGL, and macOS has no software Metal.
pub const WATCH_GL: bool = cfg!(windows);

/// How long a watched start with the system's API may take to its first frame before MDM starts
/// again the next way (a driver stuck in its first present, a window that never shows). Generous:
/// a slow computer just after it starts (#57) can take seconds.
#[cfg(gpu)]
const FIRST_FRAME_WITHIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Which graphics adapters a start with the system's API may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapters {
    /// A graphics card's driver only.
    Hardware,
    /// A software renderer only (Microsoft's WARP, Mesa's lavapipe).
    Software,
    /// A card first, else a software renderer: the last way to open the windows at all.
    Any,
    /// None at all: the API fails to start (`MDM_RENDERER=gpu-none`, or `direct3d-none`…, in
    /// off-screen test copies, to check the way back to OpenGL).
    Nothing,
}

/// One way to start drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attempt {
    OpenGl,
    /// The system's API ([`GPU`]).
    Gpu(Adapters),
}

impl Attempt {
    pub fn api(self) -> Api {
        match self {
            Attempt::OpenGl => Api::OpenGl,
            Attempt::Gpu(_) => GPU.unwrap_or(Api::OpenGl),
        }
    }

    fn name(self) -> &'static str {
        match (self, GPU) {
            (Attempt::OpenGl, _) | (_, None) => "OpenGL",
            (Attempt::Gpu(Adapters::Software), Some(Api::Direct3D)) => "Direct3D 12 (WARP)",
            (Attempt::Gpu(Adapters::Software), Some(Api::Vulkan)) => "Vulkan (software)",
            (Attempt::Gpu(Adapters::Software), Some(Api::Metal)) => "Metal (software)",
            (Attempt::Gpu(_), Some(api)) => api.name(),
        }
    }
}

/// A testing override's name. Another system's API isn't built here: no attempt.
fn parse(name: &str) -> Option<Attempt> {
    let name = name.trim().to_ascii_lowercase();
    // `<name>-none` (off-screen test copies): that API, with no adapter at all.
    if let Some(base) = name.strip_suffix("-none") {
        return match parse(base) {
            Some(Attempt::Gpu(_)) if crate::offscreen() => Some(Attempt::Gpu(Adapters::Nothing)),
            _ => None,
        };
    }
    let (api, adapters) = match name.as_str() {
        "opengl" | "gl" | "glow" => return Some(Attempt::OpenGl),
        "direct3d" | "d3d" | "d3d12" | "dx12" => (Some(Api::Direct3D), Adapters::Hardware),
        "warp" => (Some(Api::Direct3D), Adapters::Software),
        "vulkan" | "vk" => (Some(Api::Vulkan), Adapters::Hardware),
        "vulkan-software" | "lavapipe" => (Some(Api::Vulkan), Adapters::Software),
        "metal" => (Some(Api::Metal), Adapters::Hardware),
        "gpu" | "wgpu" => (GPU, Adapters::Hardware),
        "software" => (GPU, Adapters::Software),
        _ => return None,
    };
    (api.is_some() && api == GPU).then_some(Attempt::Gpu(adapters))
}

/// What the data folder remembers about this computer, read as MDM starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Known {
    /// The system's API failed here on the graphics card (`direct3d-failed.txt`).
    pub card_failed: bool,
    /// ... and on its software renderer (`direct3d-software-failed.txt`: WARP).
    pub software_failed: bool,
    /// OpenGL can't draw MDM here (`opengl-unfit.txt`): it failed to start, is too old
    /// ([`gl_unfit`]), drew no first frame, or froze inside its driver (Windows, #835, #847).
    pub gl_failed: bool,
}

impl Known {
    /// Read as the app starts: a start that left its marker behind ended before its first frame,
    /// and is remembered as that.
    pub fn read() -> Known {
        Known {
            card_failed: failed_before(Way::Card).is_some(),
            software_failed: failed_before(Way::Software).is_some(),
            gl_failed: failed_before(Way::OpenGl).is_some(),
        }
    }

    /// Only looked at (the crash report's window: its report may be about a start not yet
    /// drawn, which counts as failed).
    pub fn suspect() -> Known {
        let any = |w: Way| {
            w.files()
                .is_some_and(|(starting, failed)| starting.exists() || failed.exists())
        };
        Known {
            card_failed: any(Way::Card),
            software_failed: any(Way::Software),
            gl_failed: any(Way::OpenGl),
        }
    }
}

/// The ways to try, in order, and which of them are watched (see [`run`]).
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub attempts: Vec<Attempt>,
    /// The starts watched: each one's start is marked until its first frame, which must come in
    /// time, and its failure is remembered, so the next start goes on to the next way. The app's
    /// own plan's only (not an override's, nor the crash report's window's).
    pub watch: Vec<Attempt>,
    /// Why the first way is the one tried first, for the log and the crash reports.
    pub because: Because,
    /// An OpenGL that can't draw MDM ([`gl_unfit`]) ends this start, is remembered, and MDM
    /// starts again without it: the app's own starts (not an override's, nor the crash report's,
    /// which only passes over one already remembered).
    pub gl_checked: bool,
}

#[cfg(test)]
impl Plan {
    /// Is any of its starts watched?
    pub fn watched(&self) -> bool {
        !self.watch.is_empty()
    }
}

/// Why a plan's first way is tried first (#280: a report drawn with OpenGL where Direct3D is the
/// default must say why).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Because {
    /// Chosen in View → Draw with.
    Chosen,
    /// Nothing chosen (or another system's API): this system's default.
    Default,
    /// `/renderer` or `MDM_RENDERER`.
    Override,
    /// The system's API is the choice, but it failed here before on the card: OpenGL until it is
    /// chosen again.
    FailedBefore,
    /// OpenGL can't draw MDM here (remembered): the system's API, on a software renderer if it
    /// failed on the card, until OpenGL is chosen again.
    OpenGlUnfit,
}

impl Because {
    /// Its words after "Drawing with: …" (`api`: the system's API's name).
    fn words(self, api: &str) -> String {
        let details = |d: &Mutex<Option<String>>| d.lock().ok().and_then(|d| d.clone());
        match self {
            Because::Chosen => "chosen in View → Draw with".into(),
            Because::Default => "the default here".into(),
            Because::Override => "the testing override (/renderer, MDM_RENDERER)".into(),
            Because::OpenGlUnfit => format!(
                "OpenGL can't draw MDM here: {}",
                details(&GL_DETAILS).unwrap_or_default()
            ),
            Because::FailedBefore => format!(
                "{api} failed here before: {}",
                details(&FAILED_DETAILS).unwrap_or_default()
            ),
        }
    }
}

/// What to try: the override if any (then the other way), else the setting (or [`DEFAULT`]).
/// Only OpenGL where wgpu isn't built.
///
/// The system's API chosen: on the card, unless it failed there before; then OpenGL, and the
/// system's API on a software renderer last (Remote Desktop, a virtual machine: no card's
/// driver, and Windows' own OpenGL 1.1 can't draw MDM). OpenGL chosen: OpenGL, the system's API
/// (card, else software) only if OpenGL can't start.
///
/// `known.gl_failed`: this computer's OpenGL can't draw MDM (remembered, Windows: #835, #847). It
/// becomes the last way, after the system's API on the card (unless that failed here before) and
/// on its software renderer (unless that failed too: then OpenGL first, nothing else drew),
/// whatever the setting; an override still gets it. A card without Direct3D 12 on Windows:
/// `[OpenGL, any]`, then `[WARP, OpenGL]` once OpenGL failed there.
pub fn plan(setting: Option<Api>, overridden: Option<&str>, known: Known) -> Plan {
    let forced = overridden.and_then(|o| {
        let a = parse(o);
        if a.is_none() {
            tracing::warn!(
                "unknown renderer {o:?} (here: opengl{}): not used",
                GPU.map(|g| format!(", {}, software", g.key()))
                    .unwrap_or_default()
            );
        }
        a
    });
    let Known {
        card_failed,
        software_failed,
        gl_failed,
    } = known;
    let (card, soft, gl, any) = (
        Attempt::Gpu(Adapters::Hardware),
        Attempt::Gpu(Adapters::Software),
        Attempt::OpenGl,
        Attempt::Gpu(Adapters::Any),
    );
    let wants_gpu = Some(chosen(setting)) == GPU;
    // OpenGL passed over: it can't draw MDM here, and the software renderer is left to try.
    let skip_gl = gl_failed && GPU.is_some() && !software_failed;
    // Only this system's own choices count as chosen (chosen()).
    let as_set = if setting.is_some_and(|s| s == chosen(setting)) {
        Because::Chosen
    } else {
        Because::Default
    };
    let (attempts, because) = match forced {
        // OpenGL last, even one that can't draw MDM: the windows always open.
        Some(a @ Attempt::Gpu(_)) => (vec![a, gl], Because::Override),
        Some(_) => (vec![gl, any], Because::Override),
        None if wants_gpu => {
            let mut ways = Vec::new();
            if !card_failed {
                ways.push(card);
            }
            if skip_gl {
                // OpenGL can't draw MDM here: the software renderer (what drew #835's card
                // right), OpenGL only if that can't start.
                ways.extend([soft, gl]);
            } else if card_failed {
                // Nothing left before OpenGL: then whatever adapter there is.
                ways.extend([gl, any]);
            } else {
                ways.extend([gl, soft]);
            }
            let because = match (card_failed, skip_gl) {
                (false, _) => as_set,
                (true, true) => Because::OpenGlUnfit,
                (true, false) => Because::FailedBefore,
            };
            (ways, because)
        }
        // OpenGL would have drawn first, but can't draw MDM: the system's API instead, on a
        // software renderer when it failed on the card here, else on the card if any.
        None if skip_gl => (
            vec![if card_failed { soft } else { any }, gl],
            Because::OpenGlUnfit,
        ),
        None => (vec![gl, any], as_set),
    };
    let attempts: Vec<Attempt> = attempts
        .into_iter()
        .filter(|a| GPU.is_some() || *a == Attempt::OpenGl)
        .collect();
    let watch = match forced {
        Some(_) => Vec::new(),
        None => attempts
            .iter()
            .copied()
            .filter(|&a| watchable(&attempts, a, known))
            .collect(),
    };
    Plan {
        watch,
        attempts,
        because,
        gl_checked: forced.is_none(),
    }
}

/// Is this way's start watched (in the app's own plan)? Only where remembering its failure
/// changes the next start's plan (else a way that never draws would start MDM again and again):
///
/// - the system's API on the card or on its software renderer, before OpenGL;
/// - OpenGL (Windows, [`WATCH_GL`]), not known to fail here, with the system's API on a software
///   renderer, or any adapter, after it (#847).
///
/// Never "any adapter" nor a way after OpenGL: OpenGL would come first again.
fn watchable(attempts: &[Attempt], a: Attempt, known: Known) -> bool {
    let at = |w: Attempt| attempts.iter().position(|&x| x == w);
    let Some((i, gl)) = at(a).zip(at(Attempt::OpenGl)) else {
        return false;
    };
    match a {
        Attempt::Gpu(Adapters::Hardware | Adapters::Software) => i < gl,
        Attempt::OpenGl => {
            WATCH_GL
                && !known.gl_failed
                && attempts[gl..]
                    .iter()
                    .any(|w| matches!(w, Attempt::Gpu(Adapters::Software | Adapters::Any)))
        }
        _ => false,
    }
}

/// The testing override: `/renderer <name>` (or `--renderer`) among the arguments, else
/// `MDM_RENDERER`.
pub fn overridden(args: &[String]) -> Option<String> {
    args.iter()
        .position(|a| {
            a.trim_start_matches(['/', '-'])
                .eq_ignore_ascii_case("renderer")
        })
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| std::env::var("MDM_RENDERER").ok())
        .filter(|s| !s.trim().is_empty())
}

// ---- Remembering a way that failed ----------------------------------------------------------

/// Written before a watched start with the system's API on the card, removed once its first
/// frame is drawn.
const STARTING: &str = "starting";
/// Why the system's API couldn't draw here on the card; while it is there, the card isn't tried.
const FAILED: &str = "failed.txt";
/// The same two for its software renderer (WARP: `direct3d-software-failed.txt`).
const SOFTWARE_STARTING: &str = "software-starting";
const SOFTWARE_FAILED: &str = "software-failed.txt";
/// The same two for OpenGL (Windows): why it can't draw MDM here; while it is there, OpenGL is
/// the last way (#835, #847).
const GL_STARTING: &str = "opengl-starting";
const GL_FAILED: &str = "opengl-unfit.txt";

/// A way whose failure is remembered in the data folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    /// The system's API on the graphics card.
    Card,
    /// The system's API on its software renderer (WARP, lavapipe).
    Software,
    /// OpenGL.
    OpenGl,
}

impl Way {
    /// The way a start is, when its start is one that can be watched.
    fn of(a: Attempt) -> Option<Way> {
        match a {
            Attempt::Gpu(Adapters::Hardware) => Some(Way::Card),
            Attempt::Gpu(Adapters::Software) => Some(Way::Software),
            Attempt::OpenGl => Some(Way::OpenGl),
            Attempt::Gpu(Adapters::Any | Adapters::Nothing) => None,
        }
    }

    /// Its files in the data folder: (starting, failed). None where there is no data folder, or
    /// no system's API for the card's and the software renderer's.
    fn files(self) -> Option<(PathBuf, PathBuf)> {
        match self {
            Way::Card => file(STARTING).zip(file(FAILED)),
            Way::Software => file(SOFTWARE_STARTING).zip(file(SOFTWARE_FAILED)),
            Way::OpenGl => mdm_ipc::data_dir().map(|d| (d.join(GL_STARTING), d.join(GL_FAILED))),
        }
    }

    /// As the log names it.
    fn name(self) -> &'static str {
        match self {
            Way::Card => Attempt::Gpu(Adapters::Hardware).name(),
            Way::Software => Attempt::Gpu(Adapters::Software).name(),
            Way::OpenGl => "OpenGL",
        }
    }

    /// The details of its failure, as known in this process.
    fn details(self) -> &'static Mutex<Option<String>> {
        match self {
            Way::Card => &FAILED_DETAILS,
            Way::Software => &SOFTWARE_DETAILS,
            Way::OpenGl => &GL_DETAILS,
        }
    }
}

/// Why a way couldn't draw here, for View → Draw with's notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// No adapter for it (no driver; only a software renderer when a card was wanted), or no
    /// way to draw to the window with it.
    NoAdapter,
    /// Its first frame didn't come within [`FIRST_FRAME_WITHIN`].
    NoFirstFrame,
    /// A start with it ended before its first frame (a crash or a kill in the driver).
    EndedEarly,
    /// The graphics card was lost twice within 10 minutes.
    LostTwice,
    /// OpenGL older than MDM can draw with ([`gl_unfit`], #835).
    TooOld,
    /// The window froze inside OpenGL's driver ([`gl_driver_frame`], #847).
    FrozeInDriver,
    /// Anything else (the log has eframe's error).
    Other,
}

impl Why {
    const ALL: [Why; 7] = [
        Why::NoAdapter,
        Why::NoFirstFrame,
        Why::EndedEarly,
        Why::LostTwice,
        Why::TooOld,
        Why::FrozeInDriver,
        Why::Other,
    ];

    /// Its first line in the failure file.
    fn key(self) -> &'static str {
        match self {
            Why::NoAdapter => "no-adapter",
            Why::NoFirstFrame => "no-first-frame",
            Why::EndedEarly => "ended-early",
            Why::LostTwice => "lost-twice",
            Why::TooOld => "too-old",
            Why::FrozeInDriver => "froze-in-driver",
            Why::Other => "other",
        }
    }

    /// A failure file's text: its reason's key, then the details on the next line. An older one
    /// (one line of details) is `older`: `Other` from test builds before 0.5.0 (Direct3D's),
    /// `TooOld` from #835's (OpenGL's, which only knew that one).
    fn read(text: &str, older: Why) -> (Why, &str) {
        let (first, rest) = text.trim().split_once('\n').unwrap_or((text.trim(), ""));
        match Why::ALL.into_iter().find(|w| w.key() == first.trim()) {
            Some(w) => (w, rest.trim()),
            None => (older, text.trim()),
        }
    }
}

/// The system's API failed here (before this start, or in it) and wasn't chosen again since:
/// why (the first failure: the card's, when it was tried).
static GPU_FAILED: Mutex<Option<Why>> = Mutex::new(None);
/// OpenGL can't draw MDM here (remembered, or found in this start) and wasn't chosen again since:
/// why.
static GL_FAILED_WHY: Mutex<Option<Why>> = Mutex::new(None);

/// The details of each way's failure, as its file has them (for the log and the crash reports).
static FAILED_DETAILS: Mutex<Option<String>> = Mutex::new(None);
static SOFTWARE_DETAILS: Mutex<Option<String>> = Mutex::new(None);
static GL_DETAILS: Mutex<Option<String>> = Mutex::new(None);

/// A way's failure, known in this process: the system API's first one is kept (the card's, the
/// one View → Draw with explains), OpenGL's as it comes.
fn set_failed(way: Option<Way>, why: Why, details: Option<&str>) {
    match way {
        Some(Way::OpenGl) => {
            if let Ok(mut f) = GL_FAILED_WHY.lock() {
                *f = Some(why);
            }
        }
        _ => {
            if let Ok(mut f) = GPU_FAILED.lock() {
                f.get_or_insert(why);
            }
        }
    }
    if let (Some(way), Some(details)) = (way, details)
        && let Ok(mut d) = way.details().lock()
    {
        *d = Some(details.to_owned());
    }
}

/// For View → Draw with's note: why the system's API doesn't draw here, if it failed.
#[cfg(gpu)]
pub fn gpu_failed() -> Option<Why> {
    GPU_FAILED.lock().ok().and_then(|f| *f)
}

/// For View → Draw with's note: why OpenGL can't draw MDM here, if it can't.
#[cfg(gpu)]
pub fn gl_failed() -> Option<Why> {
    GL_FAILED_WHY.lock().ok().and_then(|f| *f)
}

/// The system's API's file `name` in the data folder (`direct3d-failed.txt`,
/// `vulkan-starting`…). None where there is no such API.
fn file(name: &str) -> Option<PathBuf> {
    let key = GPU?.key();
    mdm_ipc::data_dir().map(|d| d.join(format!("{key}-{name}")))
}

/// Why this way failed here before, if it did. A start that left its marker behind ended before
/// its first frame: that becomes the reason.
fn failed_before(way: Way) -> Option<String> {
    let (starting, failed) = way.files()?;
    let name = way.name();
    if starting.exists() {
        let why = format!("the last start with {name} ended before its first frame");
        tracing::warn!("{name}: {why}; passed over until it is chosen again");
        remember_failure(way, Why::EndedEarly, &why);
        let _ = std::fs::remove_file(&starting);
    }
    let text = std::fs::read_to_string(&failed).ok()?;
    let older = if way == Way::OpenGl {
        Why::TooOld
    } else {
        Why::Other
    };
    let (why, details) = Why::read(&text, older);
    tracing::info!("{name} failed here before ({why:?}: {details})");
    set_failed(Some(way), why, Some(details));
    Some(details.to_owned())
}

/// This way couldn't draw here: remembered with its reason, in the data folder and in this
/// process (View → Draw with's notes).
fn remember_failure(way: Way, why: Why, details: &str) {
    if let Some((_, f)) = way.files() {
        let _ = std::fs::write(f, format!("{}\n{details}", why.key()));
    }
    if way == Way::OpenGl {
        set_failed(Some(way), why, Some(details));
    }
}

/// The system's API chosen again (View → Draw with): its next start is tried, on the card and
/// on its software renderer.
#[cfg(gpu)]
pub fn forget_failure() {
    if let Ok(mut f) = GPU_FAILED.lock() {
        *f = None;
    }
    forget(&[Way::Card, Way::Software]);
}

fn forget(ways: &[Way]) {
    for (starting, failed) in ways.iter().filter_map(|w| w.files()) {
        let _ = std::fs::remove_file(starting);
        let _ = std::fs::remove_file(failed);
    }
}

// ---- An OpenGL that can't draw MDM (#835, #847) ----------------------------------------------

/// What an OpenGL driver says it is (glow's `version()`, read from `GL_VERSION`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlVersion {
    pub major: u32,
    pub minor: u32,
    /// OpenGL ES.
    pub embedded: bool,
}

/// Why this OpenGL can't draw MDM, if it can't: one rule per line, so the policy can widen
/// (`windows`: on Windows, given apart so each system's rules are tested everywhere).
///
/// - Windows, OpenGL older than 3.0 (#835: Intel's first HD Graphics, OpenGL 2.1, drew the
///   windows as garbage, and Direct3D 12 on WARP drew them right). Linux and macOS keep it:
///   no report there, and their old drivers (Mesa's) aren't the same code.
pub fn gl_unfit(v: GlVersion, windows: bool) -> Option<String> {
    let name = if v.embedded { "OpenGL ES" } else { "OpenGL" };
    if windows && (v.major, v.minor) < (3, 0) {
        return Some(format!("{name} {}.{} is older than 3.0", v.major, v.minor));
    }
    None
}

/// OpenGL chosen again (View → Draw with): its next start is tried, and checked again.
#[cfg(gpu)]
pub fn forget_gl_failure() {
    if let Ok(mut f) = GL_FAILED_WHY.lock() {
        *f = None;
    }
    forget(&[Way::OpenGl]);
}

/// Where a frozen window thread's stack (its frames' modules, top first) shows it stuck inside
/// OpenGL's driver, as the freeze reports of #847 show it: above the program's own code, Windows'
/// OpenGL (`opengl32.dll`, whose `wglSwapBuffers` shows a frame) or a card maker's OpenGL driver
/// (Intel's `ig*icd*.dll`, NVIDIA's `nvoglv*.dll`, AMD's `atio6axx.dll` / `atioglxx.dll`). Its
/// module's name, if so. Below the program's code, `opengl32.dll` is in every OpenGL window's
/// message handling: a freeze in MDM's own code isn't one. `testing` (`MDM_TEST_GL_STALL` in an
/// off-screen test copy): the program's own code counts, its stall standing for a driver's.
pub fn gl_driver_frame<'a>(
    modules: impl IntoIterator<Item = Option<&'a str>>,
    testing: bool,
) -> Option<String> {
    let driver = |m: &str| {
        let m = m.to_ascii_lowercase();
        m == "opengl32.dll"
            || (m.starts_with("ig") && m.contains("icd") && m.ends_with(".dll"))
            || m.starts_with("nvoglv")
            || m.starts_with("atio6axx")
            || m.starts_with("atioglxx")
    };
    for m in modules.into_iter().flatten() {
        if driver(m) {
            return Some(m.to_owned());
        }
        // The program's own code (its .exe): what is below it isn't where the window is stuck.
        if m.to_ascii_lowercase().ends_with(".exe") {
            return testing.then(|| "MDM_TEST_GL_STALL".to_owned());
        }
    }
    None
}

/// The window froze inside OpenGL's driver (the crash report's window read it from the freeze's
/// stack, [`gl_driver_frame`]): remembered, so the next starts draw with the system's API on its
/// software renderer when the card has none (#847). Windows only ([`WATCH_GL`]).
pub fn remember_gl_froze(module: &str, seconds: u64) {
    if !WATCH_GL {
        return;
    }
    let details = format!("the window froze for {seconds} s inside {module} as OpenGL drew");
    tracing::warn!("OpenGL can't draw MDM here: {details}; passed over from the next start");
    remember_failure(Way::OpenGl, Why::FrozeInDriver, &details);
}

/// `MDM_TEST_GL_STALL=1` (off-screen test copies): once, a few seconds after the first frame
/// drawn with OpenGL, the window stalls past the freeze watchdog's limit, as in a driver's
/// present (#847's freezes), and the crash report's window takes it for one.
fn stall_test() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !testing("MDM_TEST_GL_STALL")
        || ACTIVE.get() != Some(&Api::OpenGl)
        || !DRAWN.load(Ordering::Relaxed)
        || since_start() < 5000
        || DONE.swap(true, Ordering::Relaxed)
    {
        return;
    }
    tracing::info!("testing: the window stalls 12 s while OpenGL draws");
    std::thread::sleep(std::time::Duration::from_secs(12));
}

/// The version OpenGL says it is; `MDM_TEST_GL_VERSION=2.1` in off-screen test copies, as an old
/// card's driver says.
fn gl_version(gl: &eframe::glow::Context) -> GlVersion {
    use eframe::glow::HasContext as _;
    let v = gl.version();
    let real = GlVersion {
        major: v.major,
        minor: v.minor,
        embedded: v.is_embedded,
    };
    let test = std::env::var("MDM_TEST_GL_VERSION")
        .ok()
        .filter(|_| crate::offscreen())
        .and_then(|t| {
            let (major, minor) = t.trim().split_once('.')?;
            Some(GlVersion {
                major: major.parse().ok()?,
                minor: minor.parse().ok()?,
                embedded: false,
            })
        });
    if let Some(t) = test {
        tracing::info!(
            "testing: OpenGL {}.{} says it is {}.{}",
            real.major,
            real.minor,
            t.major,
            t.minor
        );
    }
    test.unwrap_or(real)
}

/// A start with OpenGL: when it can't draw MDM ([`gl_unfit`]) and that wasn't known yet, it is
/// remembered and MDM starts again without it (as after a start that drew nothing: the next
/// start's plan passes over it). Known already, OpenGL draws as the last way, the others having
/// failed, and goes on: the windows always open. `checked`: the plan's `gl_checked`.
fn check_gl(gl: &eframe::glow::Context, checked: bool, driver: &str) {
    let Some(why) = gl_unfit(gl_version(gl), cfg!(windows)) else {
        return;
    };
    let known = GL_FAILED_WHY.lock().is_ok_and(|u| u.is_some());
    if known || !checked || GPU.is_none() {
        tracing::warn!("{why}: it may draw MDM wrong, but nothing else could start");
        return;
    }
    let details = format!("{why} ({driver})");
    remember_failure(Way::OpenGl, Why::TooOld, &details);
    // Remembered as this reason, not as a start that ended before its first frame.
    if let Some((starting, _)) = Way::OpenGl.files() {
        let _ = std::fs::remove_file(starting);
    }
    #[cfg(gpu)]
    gpu::start_again(&format!("OpenGL can't draw MDM here: {details}"));
}

/// A test's failure asked for (`name`=1), honoured only by off-screen test copies.
fn testing(name: &str) -> bool {
    crate::offscreen() && std::env::var_os(name).is_some_and(|v| v == "1")
}

/// MDM_TEST_SLOW_START=<ms> (off-screen test copies): making the app takes that much longer, as
/// on a slow computer just after it starts (#280, the first frame's clock).
pub fn test_slow_start() {
    let ms = std::env::var("MDM_TEST_SLOW_START")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|_| crate::offscreen());
    if let Some(ms) = ms {
        tracing::info!("testing: making the app {ms} ms slower");
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// The start of every main-window pass: the system's API's device checked (see `gpu::poll`).
/// The pass itself is MDM's own work, not the API's (see [`FirstFrameClock`]).
pub fn frame_begin() {
    pace();
    own_work_begin();
    #[cfg(gpu)]
    gpu::poll();
    stall_test();
}

/// The frames are paced by MDM, not by the screen's refresh (see [`unpaced_swaps`]).
static PACED: AtomicBool = AtomicBool::new(false);

/// wgpu presents without waiting for the refresh once MDM paces the frames: its surface takes
/// the present mode from the app's frame, which the first pass has (none while the app is made).
pub fn present_unpaced(frame: &mut eframe::Frame) {
    #[cfg(gpu)]
    if PACED.load(Ordering::Relaxed)
        && let Some(mut config) = frame.wgpu_surface_config()
        && config.present_mode != eframe::wgpu::PresentMode::AutoNoVsync
    {
        config.present_mode = eframe::wgpu::PresentMode::AutoNoVsync;
        frame.set_wgpu_surface_config(config);
    }
    #[cfg(not(gpu))]
    let _ = frame;
}

/// The shortest time between two frames when MDM paces them: 60 a second, as the screen would.
const FRAME_PACE: std::time::Duration = std::time::Duration::from_micros(16_667);

/// Linux under XWayland (MDM's default in a Wayland session) with NVIDIA's own driver: a swap
/// that waits for the screen's refresh waited about a second for each window XWayland's
/// compositor wasn't showing at that moment (the main window behind a dialog, a dialog behind
/// another), and every window of MDM draws in one pass, so a key typed in Download video's File
/// name showed 1 to 2 seconds later, and every click the same. Its windows swap at once there,
/// and MDM spaces its frames itself ([`pace`]). Other drivers under XWayland (Mesa: Intel, AMD,
/// nouveau) keep the refresh's pacing. `vendor`: the driver's vendor, as it names itself.
fn unpaced_swaps(xwayland: bool, vendor: &str) -> bool {
    xwayland && vendor.to_ascii_lowercase().contains("nvidia")
}

/// MDM's windows are X11's in a Wayland session (XWayland).
fn on_xwayland() -> bool {
    #[cfg(target_os = "linux")]
    return crate::unix::on_xwayland();
    #[cfg(not(target_os = "linux"))]
    false
}

/// OpenGL's swap interval is chosen as its first window is made, before the app knows the
/// driver: under XWayland, the GLX vendor of the display is asked first (`unix::glx_vendor`).
fn gl_vendor_before_windows() -> Option<String> {
    #[cfg(target_os = "linux")]
    return on_xwayland().then(crate::unix::glx_vendor).flatten();
    #[cfg(not(target_os = "linux"))]
    None
}

/// Once the app is made and its driver is known: whether MDM paces the frames, and the log says
/// so. OpenGL's windows were made unpaced only if [`configure`] took the GLX vendor for NVIDIA's;
/// wgpu's adapter says its vendor now, and its present mode follows at the first pass
/// ([`present_unpaced`]).
fn pacing_known(cc: &eframe::CreationContext<'_>, swapped_at_once: bool) {
    let xwayland = on_xwayland();
    #[cfg(target_os = "linux")]
    let gl_vendor = cc.gl.as_deref().map(crate::unix::gl_vendor);
    #[cfg(not(target_os = "linux"))]
    let gl_vendor: Option<String> = None;
    #[cfg(gpu)]
    let gpu_vendor = cc
        .wgpu_render_state
        .as_ref()
        .map(|rs| gpu::vendor_name(&rs.adapter.get_info()));
    #[cfg(not(gpu))]
    let gpu_vendor: Option<String> = None;
    let vendor = gl_vendor.or(gpu_vendor).unwrap_or_default();
    let unpaced = unpaced_swaps(xwayland, &vendor);
    if cc.gl.is_some() && unpaced != swapped_at_once {
        // The GLX vendor and the context's disagree (an offloading setup): the windows keep the
        // swap they were made with, and MDM paces only when they don't wait for the refresh.
        tracing::warn!(
            "OpenGL's vendor is {vendor}, but its windows were made {}",
            if swapped_at_once {
                "without waiting for the screen's refresh"
            } else {
                "waiting for the screen's refresh"
            }
        );
    }
    let paced = if cc.gl.is_some() {
        swapped_at_once
    } else {
        unpaced
    };
    PACED.store(paced, Ordering::Relaxed);
    if paced {
        tracing::info!(
            "frame pacing: {vendor} under XWayland, so frames don't wait for the screen's refresh; MDM spaces them itself, {} a second at most",
            (1.0 / FRAME_PACE.as_secs_f64()).round()
        );
    }
}

/// When MDM paces the frames: the next waits until [`FRAME_PACE`] has passed since the last
/// began (a pass only runs when there is something to draw: an idle window waits as before).
fn pace() {
    use std::cell::Cell;
    use std::time::Instant;
    thread_local! {
        static LAST: Cell<Option<Instant>> = const { Cell::new(None) };
    }
    if !PACED.load(Ordering::Relaxed) {
        return;
    }
    LAST.with(|last| {
        if let Some(wait) = last.get().and_then(|t| FRAME_PACE.checked_sub(t.elapsed())) {
            std::thread::sleep(wait);
        }
        last.set(Some(Instant::now()));
    });
}

/// The first frame was drawn (presented).
static DRAWN: AtomicBool = AtomicBool::new(false);

/// The end of every main-window pass. Once a frame has been completed before this one (egui's
/// frame count, not its passes: a first frame can run two), the first was drawn: a watched
/// start made it.
pub fn frame_done(ctx: &egui::Context) {
    own_work_end();
    if DRAWN.load(Ordering::Relaxed) || ctx.cumulative_frame_nr() == 0 {
        return;
    }
    // MDM_TEST_NO_FIRST_FRAME=1 (off-screen test copies): a first frame that never comes (the
    // watchdog's check); `opengl`: only when OpenGL draws.
    let never = std::env::var("MDM_TEST_NO_FIRST_FRAME")
        .ok()
        .filter(|_| crate::offscreen());
    if never.as_deref() == Some("1")
        || (never.as_deref() == Some("opengl") && ACTIVE.get() == Some(&Api::OpenGl))
    {
        return;
    }
    DRAWN.store(true, Ordering::Relaxed);
    let api = clock_counted().map_or(String::new(), |d| {
        format!(" ({} ms of it counted for the API)", d.as_millis())
    });
    tracing::info!("first frame drawn, {} ms after start{api}", since_start());
    for (starting, _) in [Way::Card, Way::Software, Way::OpenGl]
        .into_iter()
        .filter_map(Way::files)
    {
        let _ = std::fs::remove_file(starting);
    }
}

// ---- The first frame's clock ----------------------------------------------------------------

/// How long a watched start has been the graphics API's to bring its first frame: the time from
/// the start of [`run`] to the first frame, less MDM's own work meanwhile (making the app:
/// loading the list, the fonts, the icons; and its passes' logic and layout), which isn't the
/// API's. A slow computer just after it starts can spend more than [`FIRST_FRAME_WITHIN`] there
/// alone, and that was counted as Direct3D drawing no first frame: MDM started again with OpenGL
/// and kept it (#280).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FirstFrameClock {
    /// Counted before the stretch now running.
    counted: std::time::Duration,
    /// Since when it runs; None while MDM's own work pauses it.
    since: Option<std::time::Instant>,
}

impl FirstFrameClock {
    fn started(now: std::time::Instant) -> Self {
        FirstFrameClock {
            counted: std::time::Duration::ZERO,
            since: Some(now),
        }
    }

    /// MDM's own work begins: not counted.
    fn pause(&mut self, now: std::time::Instant) {
        if let Some(since) = self.since.take() {
            self.counted += now.saturating_duration_since(since);
        }
    }

    /// MDM's own work is done: the API's turn again.
    fn resume(&mut self, now: std::time::Instant) {
        self.since.get_or_insert(now);
    }

    fn counted(&self, now: std::time::Instant) -> std::time::Duration {
        self.counted
            + self.since.map_or(std::time::Duration::ZERO, |s| {
                now.saturating_duration_since(s)
            })
    }
}

/// The watched start's clock (None when no start is watched).
static CLOCK: Mutex<Option<FirstFrameClock>> = Mutex::new(None);

fn with_clock<R>(f: impl FnOnce(&mut FirstFrameClock, std::time::Instant) -> R) -> Option<R> {
    let mut c = CLOCK.lock().ok()?;
    let now = std::time::Instant::now();
    c.as_mut().map(|c| f(c, now))
}

/// MDM's own work begins (the app being made, a pass's logic and layout): the first-frame clock
/// stops until [`own_work_end`].
pub fn own_work_begin() {
    if !DRAWN.load(Ordering::Relaxed) {
        with_clock(|c, now| c.pause(now));
    }
}

/// MDM's own work is done: the first-frame clock runs again.
pub fn own_work_end() {
    if !DRAWN.load(Ordering::Relaxed) {
        with_clock(|c, now| c.resume(now));
    }
}

/// The time counted for the API so far (a watched start only).
fn clock_counted() -> Option<std::time::Duration> {
    with_clock(|c, now| c.counted(now))
}

// ---- What draws now ---------------------------------------------------------------------------

static START: OnceLock<std::time::Instant> = OnceLock::new();

/// The program started (first thing in `main`).
pub fn mark_start() {
    let _ = START.set(std::time::Instant::now());
}

fn since_start() -> u128 {
    START.get().map_or(0, |s| s.elapsed().as_millis())
}

/// What draws now (set once the window started), for View → Draw with.
static ACTIVE: OnceLock<Api> = OnceLock::new();

#[cfg(gpu)]
pub fn active() -> Option<Api> {
    ACTIVE.get().copied()
}

/// Why a start with the system's API failed, from eframe's error: no adapter or surface for it
/// (no driver, `VK_ICD_FILENAMES` pointing at nothing, only a software renderer when a card was
/// wanted), or something else.
fn why_failed(e: &eframe::Error) -> Why {
    #[cfg(gpu)]
    {
        use eframe::egui_wgpu::WgpuError;
        if let eframe::Error::Wgpu(
            WgpuError::CustomNativeAdapterSelectionError(_)
            | WgpuError::RequestAdapterError(_)
            | WgpuError::CreateSurfaceError(_),
        ) = e
        {
            return Why::NoAdapter;
        }
    }
    let _ = e;
    Why::Other
}

/// What draws now is a software renderer (WARP, lavapipe), for View → Draw with.
#[cfg(gpu)]
static SOFTWARE: AtomicBool = AtomicBool::new(false);

#[cfg(gpu)]
pub fn software() -> bool {
    SOFTWARE.load(Ordering::Relaxed)
}

/// Run eframe's window with each way of the plan until one starts. `options` makes the window's
/// options (each try needs its own); `creator` makes the app, once (a try that fails before
/// that leaves it for the next). Returns the last try's result.
pub fn run<A: eframe::App + 'static>(
    name: &str,
    plan: &Plan,
    options: impl Fn() -> eframe::NativeOptions,
    creator: impl FnOnce(&eframe::CreationContext<'_>) -> A,
) -> eframe::Result {
    let creator = std::rc::Rc::new(std::cell::RefCell::new(Some(creator)));
    let mut result = Ok(());
    let mut instead: Option<String> = None;
    for &attempt in &plan.attempts {
        // A watched start (see `watchable`): marked until its first frame (one that ends before
        // is remembered at the next start), which must come in time, or MDM starts again the
        // next way.
        let watched = Way::of(attempt).filter(|_| plan.watch.contains(&attempt));
        if let Some((starting, _)) = watched.and_then(Way::files) {
            let _ = std::fs::write(starting, "");
        }
        #[cfg(gpu)]
        {
            let this_try = gpu::next_try();
            // Its own clock (none for a way not watched: a failed one's isn't carried over).
            if let Ok(mut c) = CLOCK.lock() {
                *c = watched.map(|_| FirstFrameClock::started(std::time::Instant::now()));
            }
            if let Some(way) = watched {
                gpu::watch_first_frame(way, this_try);
            }
        }
        let mut opts = options();
        configure(&mut opts, attempt);
        tracing::info!("drawing with {}", attempt.name());
        let make = creator.clone();
        let gl_checked = plan.gl_checked;
        let swapped_at_once = attempt == Attempt::OpenGl && !opts.glow_options.vsync;
        // Why this way: the plan's reason for its first, the last one's failure for another.
        let why_not = match &instead {
            Some(w) => w.clone(),
            None => plan.because.words(GPU.map_or("", |g| g.name())),
        };
        result = eframe::run_native(
            name,
            opts,
            Box::new(move |cc| {
                let make = make.borrow_mut().take().ok_or("the app was already made")?;
                started(cc, attempt, why_not, gl_checked);
                pacing_known(cc, swapped_at_once);
                // Making the app is MDM's own work, not the API's: off the first frame's clock.
                own_work_begin();
                let app = make(cc);
                own_work_end();
                Ok(Box::new(app))
            }),
        );
        // A failure after the app was made is the app's own (or a later one): no retry.
        let Err(e) = &result else { break };
        if creator.borrow().is_none() {
            break;
        }
        let why = format!("{} didn't start: {e}", attempt.name());
        tracing::warn!("{why}");
        let reason = why_failed(e);
        if attempt != Attempt::OpenGl {
            set_failed(None, reason, None);
        }
        if let Some(way) = watched {
            remember_failure(way, reason, &format!("{e}"));
            if let Some((starting, _)) = way.files() {
                let _ = std::fs::remove_file(starting);
            }
        }
        instead = Some(why);
    }
    result
}

/// The window's options for one way of drawing.
fn configure(opts: &mut eframe::NativeOptions, attempt: Attempt) {
    match attempt {
        Attempt::OpenGl => {
            opts.renderer = eframe::Renderer::Glow;
            // MDM_TEST_NO_OPENGL=1 (off-screen test copies): a pixel format no OpenGL has, as on
            // a computer whose only OpenGL is Windows' own 1.1 (Remote Desktop, a VM).
            if testing("MDM_TEST_NO_OPENGL") {
                opts.multisampling = 255;
                opts.depth_buffer = 255;
                opts.stencil_buffer = 255;
            }
        }
        #[cfg(gpu)]
        Attempt::Gpu(adapters) => {
            opts.renderer = eframe::Renderer::Wgpu;
            opts.wgpu_options = gpu::options(adapters);
        }
        // Not built here (plan() never asks for it).
        #[cfg(not(gpu))]
        Attempt::Gpu(_) => opts.renderer = eframe::Renderer::Glow,
    }
    PACED.store(false, Ordering::Relaxed);
    if attempt == Attempt::OpenGl {
        let vendor = gl_vendor_before_windows().unwrap_or_default();
        opts.glow_options.vsync = !unpaced_swaps(on_xwayland(), &vendor);
    }
}

/// The app is being made: what draws is known.
/// `why`: why this way (the plan's reason, or why the way before it didn't start).
/// `gl_checked`: the plan's (an OpenGL that can't draw MDM starts MDM again without it).
fn started(cc: &eframe::CreationContext<'_>, attempt: Attempt, why: String, gl_checked: bool) {
    #[cfg(gpu)]
    let gpu = cc
        .wgpu_render_state
        .as_ref()
        .map(|rs| gpu::describe(&rs.adapter.get_info()));
    #[cfg(not(gpu))]
    let gpu: Option<String> = None;
    // OpenGL names its card and driver too (#280: a freeze inside Intel's OpenGL driver came
    // with "OpenGL" alone).
    #[cfg(unix)]
    use crate::unix::gl_driver;
    #[cfg(windows)]
    use crate::win32::gl_driver;
    let detail = gpu
        .or_else(|| {
            cc.gl
                .as_ref()
                .map(|gl| format!("{} on {}", attempt.name(), gl_driver(gl)))
        })
        .unwrap_or_else(|| attempt.name().to_owned());
    tracing::info!(
        "drawing with {detail}, {} ms after start ({why})",
        since_start()
    );
    // An OpenGL that can't draw MDM: MDM starts again without it (#835).
    if let Some(gl) = &cc.gl {
        check_gl(gl, gl_checked, &gl_driver(gl));
    }
    // A software renderer (WARP, lavapipe), chosen or the only adapter: View → Draw with says so.
    #[cfg(gpu)]
    if let Some(rs) = &cc.wgpu_render_state {
        let cpu = rs.adapter.get_info().device_type == eframe::wgpu::DeviceType::Cpu;
        SOFTWARE.store(cpu, Ordering::Relaxed);
    }
    // The crash reports say it, and why ("OpenGL on Intel(R) UHD Graphics 620 (4.6.0 - Build …)
    // — Direct3D 12 failed here before: …").
    crate::crash::renderer(&format!("{detail} — {why}"));
    // Errors are logged, not panics, and a lost graphics card starts MDM again.
    #[cfg(gpu)]
    if let Some(rs) = &cc.wgpu_render_state {
        gpu::watch_device(&rs.device);
    }
    let _ = ACTIVE.set(attempt.api());
}

/// `/show renderer-lost` (off-screen test copies): the system's API's device lost on purpose
/// (MDM starts again).
#[cfg(gpu)]
pub fn test_device_lost() {
    gpu::lose_device();
}

/// eframe's wgpu set up for the system's API only: Direct3D 12, Vulkan or Metal.
#[cfg(gpu)]
mod gpu {
    use super::{
        Adapters, DRAWN, FIRST_FRAME_WITHIN, GPU, Way, Why, clock_counted, file, remember_failure,
        testing,
    };
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    use eframe::wgpu;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, SystemTime};

    /// The API's name in the log and the crash reports.
    fn api() -> &'static str {
        GPU.map_or("wgpu", |g| g.name())
    }

    /// The number of the try under way (a failed one goes on to the next way in the same
    /// process): a try's watch ends when the next begins.
    static TRY: AtomicUsize = AtomicUsize::new(0);

    /// A try begins: its number.
    pub fn next_try() -> usize {
        TRY.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// The first frame of this try (`way`) must come within [`FIRST_FRAME_WITHIN`]; if it doesn't
    /// (a driver stuck in its first present, a window left black), that way is remembered as
    /// failed and MDM starts again, drawing the next way.
    pub fn watch_first_frame(way: Way, this_try: usize) {
        let _ = std::thread::Builder::new()
            .name("first frame watch".into())
            .spawn(move || {
                let done =
                    || DRAWN.load(Ordering::Relaxed) || TRY.load(Ordering::Relaxed) != this_try;
                // The API's time only: MDM's own work meanwhile isn't counted (#280).
                let over = || clock_counted().is_some_and(|d| d >= FIRST_FRAME_WITHIN);
                while !over() {
                    if done() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if done() {
                    return;
                }
                let why = format!(
                    "{} drew no first frame within {} s ({} s after start)",
                    way.name(),
                    FIRST_FRAME_WITHIN.as_secs(),
                    super::since_start() / 1000
                );
                remember_failure(way, Why::NoFirstFrame, &why);
                // Remembered as this reason, not as a start that ended before its first frame.
                if let Some((starting, _)) = way.files() {
                    let _ = std::fs::remove_file(starting);
                }
                start_again(&why);
            });
    }

    /// When the graphics card is lost twice this close together, the API counts as failing.
    const LOST_AGAIN_WITHIN: Duration = Duration::from_secs(600);
    /// When the card was last lost (its time inside).
    const LOST: &str = "lost";

    /// wgpu's errors go to the log instead of ending MDM (its default handler panics), and a
    /// device lost while MDM runs (a driver update or reset: none of the three APIs recovers
    /// by itself) starts MDM again, with the same API, or the next way after a second loss
    /// within 10 minutes.
    pub fn watch_device(device: &wgpu::Device) {
        device.on_uncaptured_error(Arc::new(|e| {
            tracing::error!("{}: {e}", api());
        }));
        device.set_device_lost_callback(|reason, message| {
            let testing = TEST_LOSS.load(Ordering::Relaxed);
            if reason != wgpu::DeviceLostReason::Unknown && !testing {
                return;
            }
            let why = format!("the graphics card was lost ({reason:?}: {message})");
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let last = file(LOST)
                .and_then(|f| std::fs::read_to_string(f).ok())
                .and_then(|t| t.trim().parse::<u64>().ok());
            if last.is_some_and(|t| now.saturating_sub(t) < LOST_AGAIN_WITHIN.as_secs()) {
                // The card's, or its software renderer's when that is what drew.
                let way = if super::software() {
                    Way::Software
                } else {
                    Way::Card
                };
                let details = format!("{why}, twice within 10 minutes");
                remember_failure(way, Why::LostTwice, &details);
            } else if let Some(f) = file(LOST) {
                let _ = std::fs::write(f, now.to_string());
            }
            start_again(&why);
        });
        let _ = DEVICE.set(device.clone());
    }

    static DEVICE: std::sync::OnceLock<wgpu::Device> = std::sync::OnceLock::new();

    /// Each main-window pass, before its frame is painted: wgpu notices a lost device here and
    /// calls [`watch_device`]'s callback, before painting with it would fail.
    pub fn poll() {
        if let Some(d) = DEVICE.get() {
            let _ = d.poll(wgpu::PollType::Poll);
        }
    }
    static TEST_LOSS: AtomicBool = AtomicBool::new(false);

    /// `/show renderer-lost` (off-screen test copies): lose the device on purpose.
    pub fn lose_device() {
        if let Some(d) = DEVICE.get() {
            TEST_LOSS.store(true, Ordering::Relaxed);
            d.destroy();
            poll();
        }
    }

    /// End this copy and start MDM again: the new one waits for this one to be gone (as after
    /// an update), and draws as the failure markers now say.
    pub(super) fn start_again(why: &str) {
        tracing::warn!("{why}: starting MDM again");
        // The AppImage file itself on Linux (this program sits in its temporary mount).
        #[cfg(windows)]
        let program = std::env::current_exe().ok();
        #[cfg(unix)]
        let program = crate::unix::program();
        let started = program.and_then(|exe| {
            std::process::Command::new(exe)
                .arg(crate::update::UPDATED)
                // An override would bring the same failure back.
                .env_remove("MDM_RENDERER")
                .spawn()
                .ok()
        });
        if started.is_none() {
            tracing::error!("MDM couldn't start again");
        }
        crate::crash::clean_exit();
        // The window thread may be stuck in the driver: end at once, without unwinding.
        #[cfg(windows)]
        crate::win32::end_process(std::process::id());
        #[cfg(unix)]
        crate::unix::end_process(std::process::id());
        std::process::exit(1);
    }

    pub fn options(adapters: Adapters) -> WgpuConfiguration {
        let mut setup = WgpuSetupCreateNew::without_display_handle();
        let desc = &mut setup.instance_descriptor;
        // The system's API only, and nothing from WGPU_* variables: what draws is MDM's choice.
        desc.flags = wgpu::InstanceFlags::from_build_config();
        #[cfg(windows)]
        {
            desc.backends = wgpu::Backends::DX12;
            desc.backend_options = wgpu::BackendOptions {
                dx12: wgpu::Dx12BackendOptions {
                    // Windows' own compiler (d3dcompiler_47.dll): no dxcompiler.dll looked for
                    // on the PATH, where any program could have put one.
                    shader_compiler: wgpu::Dx12Compiler::Fxc,
                    ..Default::default()
                },
                ..Default::default()
            };
        }
        // Vulkan's loader still reads its own variables (VK_ICD_FILENAMES, VK_LOADER_*): the
        // system's way to pick a driver, and the lab's to take it away (#206).
        #[cfg(target_os = "linux")]
        {
            desc.backends = wgpu::Backends::VULKAN;
        }
        #[cfg(target_os = "macos")]
        {
            desc.backends = wgpu::Backends::METAL;
        }
        setup.power_preference = wgpu::PowerPreference::LowPower;
        setup.native_adapter_selector = Some(Arc::new(move |list, surface| {
            let kinds: Vec<(wgpu::DeviceType, bool)> = list
                .iter()
                .map(|a| {
                    let fits = surface.is_none_or(|s| a.is_surface_supported(s));
                    (a.get_info().device_type, fits)
                })
                .collect();
            match super::choose(&kinds, adapters, cfg!(target_os = "macos")) {
                Some(i) => Ok(list[i].clone()),
                None => Err(format!(
                    "no {} {} adapter among [{}]",
                    match adapters {
                        Adapters::Hardware => "hardware",
                        Adapters::Software => "software",
                        Adapters::Any => "usable",
                        Adapters::Nothing => "(none wanted)",
                    },
                    api(),
                    list.iter()
                        .map(|a| describe(&a.get_info()))
                        .collect::<Vec<_>>()
                        .join("; ")
                )),
            }
        }));
        setup.device_descriptor = Arc::new(|_adapter| wgpu::DeviceDescriptor {
            label: Some("MDM"),
            required_limits: wgpu::Limits {
                // As eframe asks: windows as large as 4K and more.
                max_texture_dimension_2d: 8192,
                // Direct3D 12 keeps one descriptor heap this large, in the graphics card's memory
                // and a copy in MDM's: wgpu's default (a million, 32 MB each) is for games. egui
                // uses one per texture (icons, fonts, file icons): a few hundred. (Vulkan and
                // Metal size their pools by use; the cap costs them nothing.)
                max_non_sampler_bindings: 65_536,
                ..wgpu::Limits::default()
            },
            // Small memory blocks: a download manager's windows need little, and it often sits
            // in the tray.
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        });
        // A software renderer as a card (MDM_TEST_SOFTWARE_CARD=1) is said in the log.
        if testing("MDM_TEST_SOFTWARE_CARD") {
            tracing::info!("testing: a software renderer counts as a graphics card");
        }
        WgpuConfiguration {
            wgpu_setup: WgpuSetup::CreateNew(setup),
            ..Default::default()
        }
    }

    /// The adapter's vendor and driver as words ("NVIDIA", "radv", "Intel open-source Mesa
    /// driver"), for [`super::unpaced_swaps`]: NVIDIA's PCI vendor id said by name too.
    pub fn vendor_name(info: &wgpu::AdapterInfo) -> String {
        const NVIDIA: u32 = 0x10de;
        let said = format!("{} {}", info.driver, info.driver_info);
        if info.vendor == NVIDIA && !said.to_ascii_lowercase().contains("nvidia") {
            format!("NVIDIA {said}")
        } else {
            said
        }
    }

    pub fn describe(info: &wgpu::AdapterInfo) -> String {
        let mut s = format!("{} on {}", api(), info.name);
        let driver = [&info.driver, &info.driver_info]
            .into_iter()
            .filter(|d| !d.is_empty())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        if !driver.is_empty() {
            s.push_str(&format!(" (driver {driver})"));
        }
        if info.device_type == wgpu::DeviceType::Cpu {
            s.push_str(" [software]");
        }
        s
    }
}

/// The adapter to draw with, among (kind, can draw to the window) in the system's order (the
/// one showing the main screen first on Windows; Mesa's device-select layer puts the default
/// one, or `DRI_PRIME`'s, first on Linux): the first fitting one of the kinds `adapters`
/// allows. `integrated_first` (macOS, whose order puts no card first): of the cards, a built-in
/// one before a separate one, as macOS itself picks for a program that asks for little, so a
/// Mac's separate card can sleep.
#[cfg(gpu)]
fn choose(
    list: &[(eframe::wgpu::DeviceType, bool)],
    adapters: Adapters,
    integrated_first: bool,
) -> Option<usize> {
    use eframe::wgpu::DeviceType;
    // MDM_TEST_NO_HARDWARE=1 (off-screen test copies): no graphics card's driver, only a
    // software renderer, as over Remote Desktop. MDM_TEST_SOFTWARE_CARD=1: the other way, a
    // software renderer taken for a card (lavapipe in the lab's virtual machines, which have no
    // graphics card, to check what a card's start does). MDM_TEST_NO_SOFTWARE=1: no software
    // renderer either (WARP failing, #847: the way on to OpenGL).
    let no_card = testing("MDM_TEST_NO_HARDWARE");
    let soft_card = testing("MDM_TEST_SOFTWARE_CARD");
    let no_soft = testing("MDM_TEST_NO_SOFTWARE");
    let hardware = |t: DeviceType| (t != DeviceType::Cpu || soft_card) && !no_card;
    let integrated = |t: DeviceType| t == DeviceType::IntegratedGpu && !no_card;
    let software = |t: DeviceType| t == DeviceType::Cpu && !no_soft;
    let first =
        |want: &dyn Fn(DeviceType) -> bool| list.iter().position(|&(t, fits)| fits && want(t));
    let card = || {
        integrated_first
            .then(|| first(&integrated))
            .flatten()
            .or_else(|| first(&hardware))
    };
    match adapters {
        Adapters::Hardware => card(),
        Adapters::Software => first(&software),
        Adapters::Any => card().or_else(|| first(&software)),
        Adapters::Nothing => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only NVIDIA's own driver under XWayland swaps at once (its vsynced swap waits about a
    /// second for a window the compositor isn't showing); Mesa's drivers keep the refresh's
    /// pacing, and so does every driver outside XWayland.
    #[test]
    fn only_nvidia_under_xwayland_swaps_at_once() {
        for nvidia in ["NVIDIA Corporation", "NVIDIA 595.99.02", "nvidia"] {
            assert!(unpaced_swaps(true, nvidia), "{nvidia}");
            assert!(!unpaced_swaps(false, nvidia), "{nvidia} outside XWayland");
        }
        for other in [
            "Mesa Project and SGI",
            "Mesa",
            "Intel",
            "AMD",
            "radv Mesa 25.2.8",
            "Intel open-source Mesa driver Mesa 25.2.8",
            "",
        ] {
            assert!(!unpaced_swaps(true, other), "{other}");
        }
    }

    #[test]
    fn the_setting_or_an_override_then_the_other_way() {
        let gpu = Attempt::Gpu(Adapters::Hardware);
        let any = Attempt::Gpu(Adapters::Any);
        let soft = Attempt::Gpu(Adapters::Software);
        let Some(api) = GPU else {
            // No wgpu here: OpenGL whatever was asked.
            for p in [
                plan(None, None, NONE),
                plan(Some(Api::Vulkan), None, NONE),
                plan(None, Some("gpu"), NONE),
            ] {
                assert_eq!(p.attempts, [Attempt::OpenGl]);
                assert!(!p.watched());
            }
            return;
        };
        // The system's API chosen: it, watched; OpenGL behind it, then a software renderer.
        let p = plan(Some(api), None, NONE);
        assert_eq!(p.attempts, [gpu, Attempt::OpenGl, soft]);
        assert!(p.watched());
        // ... unless it failed here before: OpenGL, watched on Windows (#847: its failure sends
        // the next start to the software renderer).
        let p = plan(Some(api), None, k(true, false, false));
        assert_eq!(p.attempts, [Attempt::OpenGl, any]);
        assert_eq!(p.watched(), WATCH_GL);
        // Nothing chosen: Direct3D on Windows (#89), OpenGL on Linux and macOS (#206).
        let p = plan(None, None, NONE);
        if cfg!(windows) {
            assert_eq!(p.attempts, [gpu, Attempt::OpenGl, soft]);
            assert!(p.watched());
            assert_eq!(
                plan(None, None, k(true, false, false)).attempts,
                [Attempt::OpenGl, any]
            );
        } else {
            assert_eq!(p.attempts, [Attempt::OpenGl, any]);
            assert!(!p.watched());
        }
        // The card's start is watched, and OpenGL's where its failure would send the next start
        // to the software renderer (Windows).
        let p = plan(Some(api), None, NONE);
        let gl_watched = if WATCH_GL {
            vec![gpu, Attempt::OpenGl]
        } else {
            vec![gpu]
        };
        assert_eq!(p.watch, gl_watched);
        // OpenGL chosen: kept, the system's API only if OpenGL can't start.
        let p = plan(Some(Api::OpenGl), None, NONE);
        assert_eq!(p.attempts, [Attempt::OpenGl, any]);
        assert_eq!(p.watched(), WATCH_GL);
        // Another system's API chosen (a settings file from another computer): the default.
        for other in [Api::Direct3D, Api::Vulkan, Api::Metal]
            .into_iter()
            .filter(|&a| a != api)
        {
            assert_eq!(plan(Some(other), None, NONE), plan(None, None, NONE));
            assert_eq!(chosen(Some(other)), DEFAULT);
        }
        // Overrides win, failure or not, and aren't watched: by the API's own name or `gpu`.
        for name in [api.key(), "gpu", "GPU"] {
            let p = plan(Some(Api::OpenGl), Some(name), k(true, false, false));
            assert_eq!(p.attempts, [gpu, Attempt::OpenGl]);
            assert!(!p.watched());
        }
        let p = plan(Some(api), Some("gl"), NONE);
        assert_eq!(p.attempts, [Attempt::OpenGl, any]);
        let p = plan(None, Some("software"), NONE);
        assert_eq!(p.attempts, [soft, Attempt::OpenGl]);
        // An unknown name, or another system's API, is ignored.
        let default = plan(None, None, NONE).attempts[0];
        assert_eq!(plan(None, Some("glide"), NONE).attempts[0], default);
        for other in ["direct3d", "vulkan", "metal"]
            .into_iter()
            .filter(|&n| n != api.key())
        {
            assert_eq!(plan(None, Some(other), NONE).attempts[0], default);
        }
        // Each system's own names.
        let names: &[(&str, Attempt)] = match api {
            Api::Direct3D => &[("d3d", gpu), ("dx12", gpu), ("warp", soft)],
            Api::Vulkan => &[("vk", gpu), ("lavapipe", soft), ("vulkan-software", soft)],
            _ => &[("metal", gpu)],
        };
        for &(name, want) in names {
            assert_eq!(plan(None, Some(name), NONE).attempts[0], want, "{name}");
        }
    }

    /// Nothing remembered.
    const NONE: Known = Known {
        card_failed: false,
        software_failed: false,
        gl_failed: false,
    };

    /// What the data folder remembers: the card failed, its software renderer failed, OpenGL
    /// can't draw MDM.
    fn k(card_failed: bool, software_failed: bool, gl_failed: bool) -> Known {
        Known {
            card_failed,
            software_failed,
            gl_failed,
        }
    }

    /// Windows, a card without Direct3D 12 (#847): OpenGL, watched; once it failed there (no
    /// start, too old, no first frame, a freeze inside its driver), Direct3D 12 on WARP, watched;
    /// WARP failing too, OpenGL again, no longer watched.
    #[cfg(windows)]
    #[test]
    fn windows_draws_with_warp_once_opengl_failed() {
        let (d3d, warp, gl, any) = (
            Attempt::Gpu(Adapters::Hardware),
            Attempt::Gpu(Adapters::Software),
            Attempt::OpenGl,
            Attempt::Gpu(Adapters::Any),
        );
        for setting in [None, Some(Api::Direct3D)] {
            // A card with Direct3D 12: as before; OpenGL's start after it is watched too.
            let p = plan(setting, None, NONE);
            assert_eq!(p.attempts, [d3d, gl, warp]);
            assert_eq!(p.watch, [d3d, gl]);
            // No Direct3D 12 on the card (also an older MDM's direct3d-failed.txt): OpenGL.
            let p = plan(setting, None, k(true, false, false));
            assert_eq!(p.attempts, [gl, any]);
            assert_eq!(p.watch, [gl]);
            assert_eq!(p.because, Because::FailedBefore);
            // ... and OpenGL failed there: WARP, watched, OpenGL only if it can't start.
            let p = plan(setting, None, k(true, false, true));
            assert_eq!(p.attempts, [warp, gl]);
            assert_eq!(p.watch, [warp]);
            assert_eq!(p.because, Because::OpenGlUnfit);
            // ... and WARP too: OpenGL, nothing else drew, watched no more.
            let p = plan(setting, None, k(true, true, true));
            assert_eq!(p.attempts, [gl, any]);
            assert!(!p.watched());
            // WARP failed, OpenGL not (yet): OpenGL, watched; its failure then changes nothing.
            let p = plan(setting, None, k(true, true, false));
            assert_eq!(p.attempts, [gl, any]);
            assert_eq!(p.watch, [gl]);
        }
        // OpenGL chosen: kept and watched; once it failed here, the card, else WARP.
        let p = plan(Some(Api::OpenGl), None, NONE);
        assert_eq!((p.attempts, p.watch), (vec![gl, any], vec![gl]));
        assert_eq!(p.because, Because::Chosen);
        let p = plan(Some(Api::OpenGl), None, k(false, false, true));
        assert_eq!(p.attempts, [any, gl]);
        assert_eq!(p.because, Because::OpenGlUnfit);
        let p = plan(Some(Api::OpenGl), None, k(true, false, true));
        assert_eq!((p.attempts, p.watch), (vec![warp, gl], vec![warp]));
        // Overrides: as asked, never watched.
        for (name, first) in [("gl", gl), ("d3d", d3d), ("warp", warp)] {
            let p = plan(None, Some(name), k(true, true, true));
            assert_eq!(p.attempts[0], first);
            assert!(!p.watched());
        }
        // The files' names (an older MDM's direct3d-failed.txt and #835's opengl-unfit.txt count).
        if let Some((starting, failed)) = Way::Card.files() {
            assert!(starting.ends_with("direct3d-starting"));
            assert!(failed.ends_with("direct3d-failed.txt"));
        }
        if let Some((_, failed)) = Way::Software.files() {
            assert!(failed.ends_with("direct3d-software-failed.txt"));
        }
        if let Some((starting, failed)) = Way::OpenGl.files() {
            assert!(starting.ends_with("opengl-starting"));
            assert!(failed.ends_with("opengl-unfit.txt"));
        }
        assert_eq!(warp.name(), "Direct3D 12 (WARP)");
    }

    /// Linux and macOS: no OpenGL start watched, no OpenGL failure remembered (#847 is Windows').
    #[cfg(all(gpu, not(windows)))]
    #[test]
    fn linux_and_macos_watch_no_opengl() {
        let api = GPU.unwrap_or(Api::OpenGl);
        let card = Attempt::Gpu(Adapters::Hardware);
        assert_eq!(plan(Some(api), None, NONE).watch, [card]);
        assert!(!plan(Some(Api::OpenGl), None, NONE).watched());
        assert!(!plan(Some(api), None, k(true, false, false)).watched());
    }

    /// A freeze's window thread inside OpenGL's driver, as #847's reports show it, and not
    /// anywhere else.
    #[test]
    fn a_freeze_inside_an_opengl_driver() {
        let at = |modules: &[&'static str]| modules.iter().map(|m| Some(*m)).collect::<Vec<_>>();
        let mdm = "misterdownloadmanager.exe";
        // Intel HD 5500 (#871), AMD (#860), Windows' own OpenGL showing a frame, NVIDIA: above
        // MDM's own code.
        for (stack, driver) in [
            (
                at(&["win32u.dll", "ig8icd64.dll", "opengl32.dll", mdm]),
                "ig8icd64.dll",
            ),
            (
                at(&["ntdll.dll", "atio6axx.dll", "opengl32.dll", mdm]),
                "atio6axx.dll",
            ),
            (
                at(&["ntdll.dll", "opengl32.dll", "gdi32full.dll", mdm]),
                "opengl32.dll",
            ),
            (at(&["ntdll.dll", "nvoglv64.dll", mdm]), "nvoglv64.dll"),
            (at(&["kernelbase.dll", "IG9ICD64.DLL", mdm]), "IG9ICD64.DLL"),
        ] {
            assert_eq!(gl_driver_frame(stack, false).as_deref(), Some(driver));
        }
        // MDM's own code stuck (opengl32.dll lower down, handling the window's messages, as in
        // every OpenGL window), Direct3D's drivers, unknown frames: not OpenGL's.
        for stack in [
            at(&[
                "ntdll.dll",
                "kernelbase.dll",
                mdm,
                "user32.dll",
                "opengl32.dll",
                mdm,
            ]),
            at(&["ntdll.dll", "nvwgf2umx.dll", "d3d12core.dll", mdm]),
            vec![None, None],
        ] {
            assert_eq!(gl_driver_frame(stack, false), None);
        }
        // A test copy's stall (MDM_TEST_GL_STALL): MDM's own code counts.
        let stall = at(&["ntdll.dll", "kernelbase.dll", mdm, "opengl32.dll"]);
        assert_eq!(
            gl_driver_frame(stall, true).as_deref(),
            Some("MDM_TEST_GL_STALL")
        );
    }

    /// Windows' plans are what they were before Vulkan and Metal (#206): the same attempts for
    /// every setting, failure and override.
    #[cfg(windows)]
    #[test]
    fn windows_plans_unchanged() {
        let d3d = Attempt::Gpu(Adapters::Hardware);
        let any = Attempt::Gpu(Adapters::Any);
        let warp = Attempt::Gpu(Adapters::Software);
        let gl = Attempt::OpenGl;
        for setting in [None, Some(Api::Direct3D)] {
            assert_eq!(plan(setting, None, NONE).attempts, [d3d, gl, warp]);
            assert_eq!(
                plan(setting, None, k(true, false, false)).attempts,
                [gl, any]
            );
        }
        assert_eq!(plan(Some(Api::OpenGl), None, NONE).attempts, [gl, any]);
        assert_eq!(
            plan(Some(Api::OpenGl), Some("D3D"), k(true, false, false)).attempts,
            [d3d, gl]
        );
        assert_eq!(plan(None, Some("warp"), NONE).attempts, [warp, gl]);
        assert_eq!(plan(None, Some("vulkan"), NONE).attempts[0], d3d);
        // Its files keep their names.
        if let Some(f) = file(FAILED) {
            assert!(f.ends_with("direct3d-failed.txt"));
        }
        assert_eq!(
            Attempt::Gpu(Adapters::Software).name(),
            "Direct3D 12 (WARP)"
        );
    }

    /// An OpenGL that can't draw MDM (#835) is the last way, after the system's API on the card
    /// and on a software renderer; the order for any other card is unchanged (above).
    #[test]
    fn an_opengl_that_cant_draw_mdm_comes_last() {
        let gl = Attempt::OpenGl;
        let Some(api) = GPU else {
            // Nothing else to draw with: OpenGL, as before.
            assert_eq!(plan(None, None, k(true, false, true)).attempts, [gl]);
            return;
        };
        let card = Attempt::Gpu(Adapters::Hardware);
        let any = Attempt::Gpu(Adapters::Any);
        let soft = Attempt::Gpu(Adapters::Software);
        // The system's API chosen, never failed: the card, watched, then its software renderer
        // before OpenGL.
        let p = plan(Some(api), None, k(false, false, true));
        assert_eq!(p.attempts, [card, soft, gl]);
        assert!(p.watched() && p.gl_checked);
        assert_eq!(p.because, Because::Chosen);
        // ... failed on the card here (#835: no Direct3D 12 driver): the software renderer
        // (WARP) first, watched, OpenGL only if that can't start.
        let p = plan(Some(api), None, k(true, false, true));
        assert_eq!(p.attempts, [soft, gl]);
        assert_eq!(p.watch, [soft]);
        assert_eq!(p.because, Because::OpenGlUnfit);
        // ... and on the software renderer too: OpenGL first, nothing else drew; not watched
        // again (its failure is known: no start again and again).
        let p = plan(Some(api), None, k(true, true, true));
        assert_eq!(p.attempts, [gl, any]);
        assert!(!p.watched());
        // OpenGL chosen: the system's API instead, on the card if there is one.
        let p = plan(Some(Api::OpenGl), None, k(false, false, true));
        assert_eq!(p.attempts, [any, gl]);
        assert_eq!(p.because, Because::OpenGlUnfit);
        assert_eq!(
            plan(Some(Api::OpenGl), None, k(true, false, true)).attempts,
            [soft, gl]
        );
        // Nothing chosen: as the default's own choice.
        assert_eq!(plan(None, None, k(true, false, true)).attempts, [soft, gl]);
        let fresh = if DEFAULT == api {
            vec![card, soft, gl]
        } else {
            vec![any, gl]
        };
        assert_eq!(plan(None, None, k(false, false, true)).attempts, fresh);
        // An override still has it, and isn't checked.
        let p = plan(None, Some("gl"), k(true, false, true));
        assert_eq!(p.attempts, [gl, any]);
        assert!(!p.gl_checked);
        assert!(plan(None, None, NONE).gl_checked);
        // Not known: the plans above, unchanged for every other card.
        assert_eq!(
            plan(Some(api), None, k(true, false, false)).attempts,
            [gl, any]
        );
    }

    /// The rule: on Windows, OpenGL older than 3.0 can't draw MDM (#835: 2.1 drew garbage);
    /// elsewhere every version is kept.
    #[test]
    fn opengl_older_than_3_cant_draw_mdm_on_windows() {
        let v = |major, minor| GlVersion {
            major,
            minor,
            embedded: false,
        };
        assert_eq!(
            gl_unfit(v(2, 1), true).as_deref(),
            Some("OpenGL 2.1 is older than 3.0")
        );
        assert!(gl_unfit(v(1, 1), true).is_some(), "Windows' own OpenGL 1.1");
        for ok in [v(3, 0), v(3, 3), v(4, 6)] {
            assert_eq!(gl_unfit(ok, true), None);
        }
        for any in [v(1, 1), v(2, 1), v(4, 6)] {
            assert_eq!(gl_unfit(any, false), None);
        }
    }

    #[test]
    fn override_from_the_command_line() {
        let a = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(overridden(&a("/renderer d3d")).as_deref(), Some("d3d"));
        assert_eq!(
            overridden(&a("--renderer warp /n")).as_deref(),
            Some("warp")
        );
    }

    #[test]
    fn settings_names() {
        for (api, name) in [
            (Api::OpenGl, "opengl"),
            (Api::Direct3D, "direct3d"),
            (Api::Vulkan, "vulkan"),
            (Api::Metal, "metal"),
        ] {
            let json = format!("\"{name}\"");
            assert_eq!(serde_json::to_string(&api).unwrap(), json);
            assert_eq!(serde_json::from_str::<Api>(&json).unwrap(), api);
        }
    }

    #[test]
    fn failure_reasons_in_their_file() {
        for why in Why::ALL {
            let text = format!("{}\nsome details", why.key());
            assert_eq!(Why::read(&text, Why::Other), (why, "some details"));
        }
        // The files of test builds before 0.5.0 on Windows: one line of details.
        let old = "Direct3D drew no first frame within 10 s";
        assert_eq!(Why::read(old, Why::Other), (Why::Other, old));
        assert_eq!(Why::read("  \n", Why::Other), (Why::Other, ""));
        // #835's opengl-unfit.txt: one line, the only reason it knew.
        let unfit = "OpenGL 2.1 is older than 3.0 (Intel HD Graphics)";
        assert_eq!(Why::read(unfit, Why::TooOld), (Why::TooOld, unfit));
    }

    #[cfg(gpu)]
    #[test]
    fn adapters_in_the_systems_order() {
        use eframe::wgpu::DeviceType::*;
        // The main screen's card first; one that can't draw to the window is passed over.
        let l = [(IntegratedGpu, false), (DiscreteGpu, true), (Cpu, true)];
        assert_eq!(choose(&l, Adapters::Hardware, false), Some(1));
        assert_eq!(choose(&l, Adapters::Software, false), Some(2));
        assert_eq!(choose(&l, Adapters::Any, false), Some(1));
        // No card: a software renderer only when allowed.
        let warp = [(Cpu, true)];
        assert_eq!(choose(&warp, Adapters::Hardware, false), None);
        assert_eq!(choose(&warp, Adapters::Any, false), Some(0));
        assert_eq!(choose(&l, Adapters::Nothing, false), None);
        // macOS: the built-in card before a separate one, whatever the order; the separate one
        // when it is the only one that fits.
        let mac = [(DiscreteGpu, true), (IntegratedGpu, true)];
        assert_eq!(choose(&mac, Adapters::Hardware, true), Some(1));
        assert_eq!(choose(&mac, Adapters::Hardware, false), Some(0));
        assert_eq!(choose(&l, Adapters::Hardware, true), Some(1));
        assert_eq!(choose(&[(VirtualGpu, true)], Adapters::Any, true), Some(0));
    }

    /// The first frame's clock counts the API's time only: MDM's own work (making the app, its
    /// passes) is left out, so a slow start of MDM's isn't taken for Direct3D failing (#280: 10.7
    /// s from OpenGL's context to the first frame on the reporter's computer, at the 10 s limit).
    #[test]
    fn the_first_frame_clock_leaves_out_mdms_own_work() {
        use std::time::{Duration, Instant};
        let s = |n: u64| Duration::from_secs(n);
        let t0 = Instant::now();
        let mut c = FirstFrameClock::started(t0);
        // The window and the device: 2 s, the API's.
        assert_eq!(c.counted(t0 + s(2)), s(2));
        // The app made in 12 s: not counted, however long.
        c.pause(t0 + s(2));
        assert_eq!(c.counted(t0 + s(14)), s(2));
        c.resume(t0 + s(14));
        // Its first pass, 3 s of MDM's, then 1 s of painting and presenting.
        c.pause(t0 + s(14));
        c.resume(t0 + s(17));
        assert_eq!(c.counted(t0 + s(18)), s(3));
        assert!(c.counted(t0 + s(18)) < FIRST_FRAME_WITHIN);
        // A driver stuck in its first present is still caught: only its time runs.
        assert!(c.counted(t0 + s(25)) >= FIRST_FRAME_WITHIN);
        // Pausing twice, or resuming a running clock, changes nothing.
        let mut d = FirstFrameClock::started(t0);
        d.pause(t0 + s(1));
        d.pause(t0 + s(5));
        d.resume(t0 + s(6));
        d.resume(t0 + s(8));
        assert_eq!(d.counted(t0 + s(9)), s(4));
    }

    /// Why the first way is tried, for the log and the crash reports (#280).
    #[test]
    fn a_plan_says_why() {
        let Some(api) = GPU else {
            assert_eq!(plan(Some(Api::OpenGl), None, NONE).because, Because::Chosen);
            return;
        };
        assert_eq!(plan(None, None, NONE).because, Because::Default);
        assert_eq!(plan(Some(api), None, NONE).because, Because::Chosen);
        assert_eq!(plan(Some(Api::OpenGl), None, NONE).because, Because::Chosen);
        assert_eq!(
            plan(Some(Api::OpenGl), None, k(true, false, false)).because,
            Because::Chosen
        );
        assert_eq!(
            plan(Some(api), None, k(true, false, false)).because,
            Because::FailedBefore
        );
        assert_eq!(
            plan(Some(api), Some("opengl"), k(true, false, false)).because,
            Because::Override
        );
        // The default fails like a choice where the default is the system's API (Windows).
        let failed_default = plan(None, None, k(true, false, false)).because;
        if DEFAULT == api {
            assert_eq!(failed_default, Because::FailedBefore);
        } else {
            assert_eq!(failed_default, Because::Default);
        }
        assert_eq!(
            Because::Chosen.words("Direct3D 12"),
            "chosen in View → Draw with"
        );
        assert!(
            Because::FailedBefore
                .words("Direct3D 12")
                .starts_with("Direct3D 12 failed here before: ")
        );
    }
}
