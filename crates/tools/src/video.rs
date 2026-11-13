//! A video download run by yt-dlp under supervision: our file name, progress read from a fixed
//! `--progress-template`, the final path from `--print after_move`. Stopping kills it; starting
//! again continues (`--continue`). Each run takes the page as looked up (`-J`), points links at
//! servers that answer ([`crate::servers`]) and downloads from that list (`--load-info-json`),
//! so a run reads the page once. A run looks the page up afresh, except the first run of a
//! download chosen in the browser's quality menu, which takes the look-up the menu was made from
//! ([`Preloaded`]): the page is then read once for the menu and the download together.
//!
//! The bytes come down through MDM's engine (#769, [`crate::streams`]): yt-dlp chooses the
//! formats, the engine downloads each stream into the file yt-dlp looks for, then yt-dlp merges
//! them, saves the subtitles and the metadata as after its own download. What the engine can't
//! download (a live stream, RTMP…) yt-dlp downloads itself, as before.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::proc::{self, ProcError};
use crate::servers;

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Running,
    Done(PathBuf),
    Failed(String),
    Stopped,
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub state: State,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Bytes/s.
    pub speed: f64,
    /// Notes for the download's log, oldest first (a server swapped for another).
    pub events: Vec<(std::time::SystemTime, String)>,
    /// Saved beside the video once it is done: its subtitle files (#68).
    pub side_files: Vec<PathBuf>,
    /// Subtitles that didn't come with the video, once it is done (#68): the video is saved all
    /// the same.
    pub sub_problems: Vec<SubProblem>,
    /// While MDM's engine downloads the streams (#769): its segments, connections and server.
    pub engine: Option<crate::streams::View>,
    /// Why yt-dlp downloads this one itself (#769), once a run has found out.
    pub helper: Option<crate::streams::Helper>,
    /// Once every stream is in: their file being joined into one (ffmpeg, run by yt-dlp; #736),
    /// with how far that is (0 to 1) when its file's size tells.
    pub merging: Option<Option<f64>>,
    /// It failed joining its streams: what of it is kept beside its file, the whole streams
    /// among them, for Resume to join again without downloading them (yt-dlp takes them as they
    /// are; the engine's `fetch` keeps a whole one). Deleting the download with its files
    /// removes them.
    pub merge_failed: Option<Vec<PathBuf>>,
}

/// A subtitle language the site wouldn't give (#68).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubProblem {
    /// yt-dlp's code (`en`) and the site's name for it (`English`; the code when none).
    pub code: String,
    pub name: String,
    pub reason: SubReason,
    /// They came on the second try, but beside the video, not in it (they were to be embedded).
    pub beside: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubReason {
    /// HTTP 429: the site limits how often subtitles may be fetched (YouTube does, most of all
    /// for its machine-translated captions).
    TooManyRequests,
    /// yt-dlp's own words.
    Other(String),
}

/// How long to wait before asking once more for the subtitles a site refused. A site that
/// answers "too many requests" wants a pause, not a quick retry. `MDM_SUBS_RETRY_SECS`
/// shortens it for the tests (tests/fake_ytdlp.rs).
fn subs_retry_delay() -> std::time::Duration {
    let secs = std::env::var("MDM_SUBS_RETRY_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    std::time::Duration::from_secs(secs)
}

/// The subtitle languages yt-dlp said it couldn't download (with `--ignore-errors` it goes on
/// with the video and only warns: `Unable to download video subtitles for 'en': HTTP Error 429:
/// Too Many Requests`), and why.
fn subtitle_failures(warnings: &[String]) -> Vec<(String, SubReason)> {
    const SAID: &str = "Unable to download video subtitles for '";
    let mut out: Vec<(String, SubReason)> = Vec::new();
    for w in warnings {
        let Some(rest) = w.split_once(SAID).map(|(_, r)| r) else {
            continue;
        };
        let Some((code, why)) = rest.split_once('\'') else {
            continue;
        };
        let why = why.trim_start_matches(':').trim();
        let reason =
            if why.contains("429") || why.to_ascii_lowercase().contains("too many requests") {
                SubReason::TooManyRequests
            } else {
                SubReason::Other(why.to_owned())
            };
        if !out.iter().any(|(c, _)| c == code) {
            out.push((code.to_owned(), reason));
        }
    }
    out
}

/// The site's name for a subtitle language, from the looked-up page (`code` when none).
fn sub_name(info: &serde_json::Value, code: &str) -> String {
    ["subtitles", "automatic_captions"]
        .iter()
        .find_map(|k| info[k][code][0]["name"].as_str())
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| code.to_owned())
}

/// What a second, subtitles-only run needs (#68): what the first asked for, and its own
/// arguments (the output name, the browser's context, where ffmpeg is).
#[derive(Clone, Debug)]
struct SubsRetry {
    automatic: bool,
    embed: bool,
    ffmpeg: bool,
    base: Vec<String>,
}

impl SubsRetry {
    /// yt-dlp's arguments to fetch `codes` again, beside the video (a second run can't put them
    /// into a video already saved).
    fn args(&self, codes: &[String]) -> Vec<String> {
        let again = Subs {
            langs: codes.to_vec(),
            automatic: self.automatic,
            embed: false,
        };
        let mut out = self.base.clone();
        out.push("--skip-download".into());
        out.extend(subtitle_args(&again, self.ffmpeg));
        out
    }
}

/// Which subtitles to save with a video (#68). `langs` empty: none.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Subs {
    /// yt-dlp's language codes, or [`ALL_UPLOADERS`] alone for every one the uploader gave.
    pub langs: Vec<String>,
    /// The site's automatic captions rather than the uploader's.
    pub automatic: bool,
    /// Put into the video file instead of beside it (needs ffmpeg; otherwise beside it).
    pub embed: bool,
}

/// `Subs::langs` for all of the uploader's subtitles.
pub const ALL_UPLOADERS: &str = "all";

/// The files subtitles are saved as.
const SUB_EXTS: [&str; 4] = ["srt", "vtt", "ass", "ttml"];

/// yt-dlp's arguments for `subs`: written beside the video as `<stem>.<lang>.srt` (converted to
/// SRT by ffmpeg, which every player reads; without ffmpeg SRT when the site has it, else its
/// own format, usually WebVTT), or embedded into the video with ffmpeg (yt-dlp removes the
/// separate files then).
fn subtitle_args(subs: &Subs, ffmpeg: bool) -> Vec<String> {
    if subs.langs.is_empty() {
        return Vec::new();
    }
    let langs = if subs.langs.len() == 1 && subs.langs[0] == ALL_UPLOADERS {
        // Not the chat replay of a live stream, which yt-dlp lists with them.
        "all,-live_chat".to_owned()
    } else {
        subs.langs.join(",")
    };
    let embed = subs.embed && ffmpeg;
    let mut out: Vec<String> = Vec::new();
    // `--embed-subs` fetches the uploader's itself, and keeps no files behind when it is the
    // only one asked (`--write-subs` too would keep them).
    match (subs.automatic, embed) {
        (true, _) => out.push("--write-auto-subs".into()),
        (false, false) => out.push("--write-subs".into()),
        (false, true) => {}
    }
    out.extend(["--sub-langs".into(), langs]);
    if ffmpeg {
        out.extend([
            "--sub-format".into(),
            "srt/best".into(),
            "--convert-subs".into(),
            "srt".into(),
        ]);
    } else {
        out.extend(["--sub-format".into(), "srt/vtt/best".into()]);
    }
    if embed {
        out.push("--embed-subs".into());
    }
    // A subtitle the site refuses must not cost the video: yt-dlp stops on it unless told to
    // go on, and then only warns (`subtitle_failures`). Whether the video was saved is checked
    // on its own (`attempt`).
    out.push("--ignore-errors".into());
    out
}

/// Download video's Embed chapters (#827): on unless the app says otherwise ([`set_chapters`]).
static CHAPTERS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether the videos started from now on get their chapters (the app's setting, shared as it
/// changes; the command line keeps the default, on).
pub fn set_chapters(on: bool) {
    CHAPTERS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The chapters a video's page lists, put into the file (#827): `--embed-chapters` on yt-dlp's
/// last run, the one that merges and fixes the file up (after MDM's engine too). Only with ffmpeg,
/// which writes them, and for a video or the sound as the site has it, not a conversion to MP3,
/// Opus or FLAC. A video without chapters is left as it is.
fn chapter_args(chapters: bool, ffmpeg: bool, converted: bool) -> Vec<String> {
    if chapters && ffmpeg && !converted {
        vec!["--embed-chapters".into()]
    } else {
        Vec::new()
    }
}

/// The subtitle files saved beside `video` (`<stem>.<lang>.<srt|vtt…>`, as yt-dlp names them).
fn side_files(video: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (video.parent(), video.file_stem()) else {
        return Vec::new();
    };
    let prefix = format!("{}.", stem.to_string_lossy());
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p != video && p.is_file())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
            let ext = p
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase());
            name.is_some_and(|n| n.starts_with(&prefix))
                && ext.is_some_and(|e| SUB_EXTS.contains(&e.as_str()))
        })
        .collect();
    out.sort();
    out
}

/// How long the browser's quality menu's look-up may take: a stream's playlists
/// (`crate::manifest`, 5 s), yt-dlp's look-up and its second one without the cookies
/// (`proc::look_up`, 90 s each), and a little more. The download chosen there waits for it that
/// long at most, and a second click shares a look-up still running that long (the app's
/// videolookup.rs).
pub const LOOKUP_WAIT: Duration = Duration::from_secs(
    crate::manifest::BUDGET.as_secs() + 2 * crate::resolve::LOOK_UP_TIMEOUT.as_secs() + 5,
);

/// The largest look-up kept for the download (a page's is a few hundred KB, YouTube's a few MB):
/// a bigger one isn't kept, and the download looks the page up itself.
pub const INFO_MAX: usize = 8 * 1024 * 1024;

/// All the look-ups kept at once, together: past it, the oldest are let go (their downloads look
/// the page up themselves). Twenty look-ups are kept for ten minutes (videolookup.rs), and
/// [`INFO_MAX`] each would be 160 MB.
pub const INFO_TOTAL: usize = 32 * 1024 * 1024;

/// The look-ups' answers kept, oldest first, with their sizes: what [`INFO_TOTAL`] counts. A
/// look-up no longer kept anywhere (its slot dropped) counts no more.
struct Kept {
    limit: usize,
    answers: Mutex<std::collections::VecDeque<(std::sync::Weak<SlotInner>, usize)>>,
}

impl Kept {
    fn new(limit: usize) -> Arc<Kept> {
        Arc::new(Kept {
            limit,
            answers: Mutex::default(),
        })
    }

    /// Every look-up's: the process's own pool.
    fn shared() -> Arc<Kept> {
        static POOL: std::sync::LazyLock<Arc<Kept>> =
            std::sync::LazyLock::new(|| Kept::new(INFO_TOTAL));
        POOL.clone()
    }

    /// `slot` keeps an answer of `len` bytes now: the oldest others are let go while all of
    /// them together are more than the limit.
    fn add(&self, slot: &Arc<SlotInner>, len: usize) {
        let mut answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        answers.retain(|(s, _)| s.strong_count() > 0);
        answers.push_back((Arc::downgrade(slot), len));
        let mut total: usize = answers.iter().map(|(_, n)| n).sum();
        while total > self.limit && answers.len() > 1 {
            let Some((oldest, n)) = answers.pop_front() else {
                break;
            };
            total -= n;
            if let Some(s) = oldest.upgrade() {
                s.let_go();
            }
        }
    }
}

/// yt-dlp's look-up (`-J`) of a page for the browser's quality menu, kept for the download
/// chosen from it: being looked up, done (the answer as yt-dlp gave it, and whether it took
/// looking the page up without cookies), or failed. In memory only, never written or logged
/// (its links are signed): its `Debug` shows nothing of it.
#[derive(Clone)]
pub struct InfoSlot(Arc<SlotInner>);

struct SlotInner {
    info: Mutex<Info>,
    changed: Condvar,
    born: Instant,
    /// Where its answer counts ([`INFO_TOTAL`]).
    pool: Arc<Kept>,
}

impl SlotInner {
    /// Its answer dropped to make room: as if the look-up had failed.
    fn let_go(&self) {
        let mut info = self.info.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(*info, Info::Ready(..)) {
            *info = Info::Failed;
            self.changed.notify_all();
        }
    }
}

enum Info {
    Pending,
    Ready(Arc<Vec<u8>>, bool),
    Failed,
}

impl std::fmt::Debug for InfoSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InfoSlot")
    }
}

impl InfoSlot {
    /// A look-up begun at `born`.
    pub fn new(born: Instant) -> Self {
        Self::counted_in(born, Kept::shared())
    }

    fn counted_in(born: Instant, pool: Arc<Kept>) -> Self {
        InfoSlot(Arc::new(SlotInner {
            info: Mutex::new(Info::Pending),
            changed: Condvar::new(),
            born,
            pool,
        }))
    }

    pub fn born(&self) -> Instant {
        self.0.born
    }

    fn info(&self) -> std::sync::MutexGuard<'_, Info> {
        self.0.info.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// yt-dlp answered with `json` (`without_cookies`: as `proc::LookedUp` says). Bigger than
    /// [`INFO_MAX`]: as if it had failed; the oldest others are let go past [`INFO_TOTAL`]. Only
    /// a look-up still pending takes it.
    pub fn fill(&self, json: Vec<u8>, without_cookies: bool) {
        let len = json.len();
        {
            let mut info = self.info();
            if !matches!(*info, Info::Pending) {
                return;
            }
            *info = if len > INFO_MAX {
                Info::Failed
            } else {
                Info::Ready(Arc::new(json), without_cookies)
            };
            self.0.changed.notify_all();
        }
        // Not under its own lock: letting another go locks that one.
        if len <= INFO_MAX {
            self.0.pool.add(&self.0, len);
        }
    }

    /// The look-up failed (or ended without an answer): the download looks the page up itself.
    pub fn fail(&self) {
        let mut info = self.info();
        if matches!(*info, Info::Pending) {
            *info = Info::Failed;
            self.0.changed.notify_all();
        }
    }

    pub fn pending(&self) -> bool {
        matches!(*self.info(), Info::Pending)
    }

    /// How many hold this look-up (the menu's, the download chosen from it): a check that a
    /// choice let go of it.
    pub fn holders(&self) -> usize {
        Arc::strong_count(&self.0)
    }

    /// The answer once there, waiting until `until` at most, or until `stopped` says so.
    fn wait(&self, until: Instant, stopped: impl Fn() -> bool) -> Option<(Arc<Vec<u8>>, bool)> {
        let mut info = self.info();
        loop {
            match &*info {
                Info::Ready(json, without) => return Some((json.clone(), *without)),
                Info::Failed => return None,
                Info::Pending => {}
            }
            let now = Instant::now();
            if now >= until || stopped() {
                return None;
            }
            // A tenth of a second at a time: a Stop meanwhile is seen at once.
            let step = (until - now).min(Duration::from_millis(100));
            info = self
                .0
                .changed
                .wait_timeout(info, step)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// The browser's quality menu's look-up for the download chosen there (its first run only):
/// the look-up, and the choice as the menu offered it (MDM's selector, its label, untranslated,
/// and whether the menu's list had merged qualities). The page yt-dlp read must offer that very
/// choice, or the run looks the page up itself.
pub struct Preloaded {
    pub slot: InfoSlot,
    pub selector: String,
    pub label: String,
    pub merge: bool,
}

/// How a run tells whether a media server can be reached and serves a stream, in place of
/// `servers::reachable` and `servers::Prober` (which ask the servers): set by tests only
/// (tests/fake_ytdlp.rs), whose servers must never be asked.
type ServerChecks = (fn(&str) -> bool, fn(&str) -> bool);

static SERVER_CHECKS: std::sync::RwLock<Option<ServerChecks>> = std::sync::RwLock::new(None);

/// Tests only: media servers `alive` says are reachable, serving every stream `serves` says.
#[doc(hidden)]
pub fn check_servers_with(alive: fn(&str) -> bool, serves: fn(&str) -> bool) {
    *SERVER_CHECKS.write().unwrap_or_else(|e| e.into_inner()) = Some((alive, serves));
}

/// yt-dlp's look-up `json` as a run's page, when it offers `selector` under `label` among its
/// choices (`resolve::choices` with `merge`): the menu offered that very choice from it.
fn offered(json: &[u8], selector: &str, label: &str, merge: bool) -> Option<serde_json::Value> {
    let video = crate::resolve::parse(json)?;
    crate::resolve::choices(&video, merge)
        .iter()
        .any(|c| c.selector == selector && c.label == label)
        .then(|| serde_json::from_slice(json).ok())
        .flatten()
}

/// The containers and sounds yt-dlp saves (`%(ext)s`), merged, converted or as the site has them.
const MEDIA: [&str; 19] = [
    "mp4", "m4a", "m4v", "webm", "weba", "mkv", "mka", "mov", "flv", "3gp", "ts", "mp3", "opus",
    "ogg", "oga", "flac", "wav", "aac", "avi",
];
/// The subtitle files yt-dlp saves beside a video (#68).
const SUBTITLES: [&str; 10] = [
    "srt", "vtt", "ass", "ssa", "ttml", "srv1", "srv2", "srv3", "json3", "lrc",
];

/// Whether `rest`, a file name after `<stem>.` (lower case), is one yt-dlp writes for a download
/// saved as `<stem>.%(ext)s`: an unfinished run's (`<ext>.part`, `<ext>.ytdl`,
/// `<ext>.part-Frag<N>[.part]`, each also for one stream of a merged choice as `f<id>.<ext>…`,
/// that stream once whole, `f<id>.<ext>`, and a merge's `temp.<ext>`; MDM's engine's part file
/// and resume state for such a stream, `f<id>.<ext>.mdm.part` and `.mdm.json`, #769); with
/// `outputs`, also the finished file (`<ext>`) and its subtitles (`<language>.<srt|vtt…>`).
/// Nothing else: another download's files (MDM's own `.mdm.part`), a file merely named alike.
fn yt_dlp_leftover(rest: &str, outputs: bool) -> bool {
    let media = |s: &str| MEDIA.contains(&s);
    let word = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    // A format id as yt-dlp names a stream: `137`, `hls-720p`, `dash-1`, `http-480`.
    let format = |s: &str| {
        s.strip_prefix('f').is_some_and(|id| {
            word(id)
                && (id.starts_with(|c: char| c.is_ascii_digit())
                    || ["hls-", "dash-", "http-"].iter().any(|p| {
                        id.strip_prefix(p)
                            .is_some_and(|r| r.chars().any(|c| c.is_ascii_digit()))
                    }))
        })
    };
    let fragment = |s: &str| {
        s.strip_prefix("part-frag")
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    };
    let parts: Vec<&str> = rest.split('.').collect();
    // An unfinished file: `<ext>` then `part`, `ytdl`, `part-frag<N>` or `part-frag<N>.part`.
    let unfinished = |p: &[&str]| match p {
        [ext, "part" | "ytdl"] => media(ext),
        [ext, frag] => media(ext) && fragment(frag),
        [ext, frag, "part"] => media(ext) && fragment(frag),
        _ => false,
    };
    let working = match parts.as_slice() {
        ["temp", ext] => media(ext),
        [f, ext] if format(f) => media(ext),
        [f, ext, "mdm", "part" | "json"] if format(f) => media(ext),
        [f, tail @ ..] if format(f) => unfinished(tail),
        p => unfinished(p),
    };
    let finished = outputs
        && match parts.as_slice() {
            [ext] => media(ext),
            [language, sub] => word(language) && SUBTITLES.contains(sub),
            _ => false,
        };
    working || finished
}

/// What yt-dlp left in `dir` for a download saved as `<stem>.%(ext)s` (our template), as
/// [`yt_dlp_leftover`] tells them, letter case aside: what an unfinished run left, and with
/// `outputs` the finished file and its subtitles. A file not of that shape is left alone.
pub fn own_files(dir: &Path, stem: &str, outputs: bool) -> Vec<PathBuf> {
    let prefix = format!("{}.", stem.to_lowercase());
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name.strip_prefix(&prefix)
                .is_some_and(|rest| yt_dlp_leftover(rest, outputs))
        })
        .map(|e| e.path())
        .collect()
}

pub struct VideoJob {
    /// yt-dlp once the page is looked up (`None` before).
    child: Arc<Mutex<Option<proc::Spawned>>>,
    progress: Arc<Mutex<Progress>>,
    /// The engine's download of a stream, while one is under way (#769).
    current: Arc<crate::streams::Current>,
}

/// A private file for the looked-up page (its links are signed; removed when the run ends), in
/// the helpers' temporary folder ([`crate::workdir::temp_dir`]).
fn info_file() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    crate::workdir::temp_dir().join(format!(
        "mdm-video-{}-{}.json",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// The page's video information from yt-dlp (one video, not a playlist), asked as the browser
/// asked for the page (`browser`: its Referer, User-Agent and cookies, `FromBrowser::args`).
/// Without them (#229) a page that wants a sign-in, YouTube's "confirm you're not a bot", was
/// refused here although the video window's look-up, which had them, had listed its qualities.
fn look_up(exe: &Path, url: &str, browser: &[String]) -> Result<(serde_json::Value, bool), String> {
    let mut args: Vec<String> = ["-J", "--no-playlist", "--no-warnings"]
        .map(String::from)
        .to_vec();
    args.extend(browser.iter().cloned());
    let out = proc::look_up(exe, &args, url, std::time::Duration::from_secs(90))
        .map_err(|e| e.to_string())?;
    let info = serde_json::from_slice(&out.json)
        .map_err(|_| "yt-dlp gave no video information".to_owned())?;
    Ok((info, out.without_cookies))
}

type Child = Arc<Mutex<Option<proc::Spawned>>>;

/// How one run of yt-dlp ended.
enum Attempt {
    /// The video saved, the subtitles that didn't come with it, and the quality yt-dlp chose
    /// (`137+251 1920x1080`).
    Done(PathBuf, Vec<SubProblem>, Option<String>),
    Stopped,
    /// `rerouted`: links were moved to other servers for this run.
    Failed {
        error: String,
        rerouted: bool,
    },
    /// Every stream came, and joining them failed: no other server or look-up mends that. With
    /// the streams' format ids (`MDMQ`'s `1280+aud-English`).
    MergeFailed(String, Vec<String>),
}

/// End yt-dlp and everything it started (#79): whenever the download is given up, nothing may
/// go on downloading unseen (Windows: its job, the launcher's child and ffmpeg with it).
fn end(child: &Child) {
    let mut slot = child.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut s) = slot.take() {
        s.kill();
    }
}

/// The format selector in yt-dlp's arguments (`-f <selector>`).
fn selector_of(ours: &[String]) -> Option<&str> {
    ours.iter()
        .position(|a| a == "-f")
        .and_then(|i| ours.get(i + 1))
        .map(String::as_str)
}

/// `ours` with another format selector.
fn with_selector(ours: &[String], selector: &str) -> Vec<String> {
    let mut out = ours.to_vec();
    if let Some(i) = out.iter().position(|a| a == "-f")
        && let Some(s) = out.get_mut(i + 1)
    {
        *s = selector.to_owned();
    }
    out
}

/// The nearest quality to a choice from the window (`bv*[height=1080]+ba/b[height=1080]`) when
/// that one isn't offered (#94: the servers this PC reaches don't serve it): the best at or below
/// that height, else the best video there is, always a video (`b` alone would take a sound-only
/// format when no video is left; live 2026-11-05). None when the choice names no height.
fn relaxed(selector: &str) -> Option<String> {
    selector
        .contains("[height=")
        .then(|| format!("{}/bv*+ba/bv*", selector.replace("[height=", "[height<=")))
}

/// Runs the download, and again when a run fails in a way a new look-up can mend:
/// - a media server yt-dlp couldn't reach (it resolved when looked up, then didn't: #94) is
///   treated as dead, so its streams move to an alternative;
/// - a run whose links had to be moved to other servers gets one fresh look-up (new links, often
///   other servers: YouTube's alternatives refuse some streams some of the time);
/// - the chosen quality not offered at all (left out by the move, #94) falls back to the nearest
///   one below it, said in the log; never the whole download's failure while another quality
///   is there;
/// - no video left at all after the move: once more on the page's own servers, unmoved.
///
/// Whenever it is given up, yt-dlp's whole job is ended (#79).
#[allow(clippy::too_many_arguments)]
fn supervise(
    exe: &Path,
    url: &str,
    ours: &[String],
    browser: &[String],
    progress: &Mutex<Progress>,
    child: &Child,
    expected: Option<u64>,
    retry: Option<&SubsRetry>,
    preloaded: Option<Preloaded>,
    engine: Option<&ByEngine>,
) {
    // The first run only: a run after it looks the page up afresh (links expire).
    let mut preloaded = preloaded;
    let mut ours = ours.to_vec();
    let mut dead: std::collections::BTreeSet<String> = Default::default();
    let mut looked_again = false;
    let mut relaxed_to: Option<String> = None;
    let mut reroute = true;
    let mut last = String::from("yt-dlp stopped");
    for _ in 0..5 {
        let page = preloaded.take();
        match attempt(
            exe,
            url,
            &ours,
            browser,
            progress,
            child,
            expected,
            retry,
            &dead,
            reroute,
            page.as_ref(),
            engine,
        ) {
            Attempt::Stopped => return,
            Attempt::Done(f, problems, quality) => {
                end(child);
                // Named for what it holds (#569): HLS pieces joined without ffmpeg.
                let f = named_for_content(f);
                if relaxed_to.is_some()
                    && let Some(q) = quality
                {
                    note(progress, format!("Downloaded {q} instead"));
                }
                // The saved file's own size: a merged video or a converted MP3 isn't the size
                // of what came down.
                let saved = std::fs::metadata(&f).ok().map(|m| m.len());
                let side = side_files(&f);
                let mut p = progress.lock().unwrap();
                if p.state != State::Stopped {
                    p.speed = 0.0;
                    p.side_files = side;
                    p.sub_problems = problems;
                    if let Some(len) = saved {
                        p.total = Some(len);
                        p.downloaded = len;
                    }
                    p.state = State::Done(f);
                }
                return;
            }
            Attempt::MergeFailed(error, ids) => {
                end(child);
                // The streams stay for Resume (the merge's own unfinished file is gone).
                let kept = template_of(&ours)
                    .map(|(dir, stem)| kept_parts(&dir, &stem, &ids))
                    .unwrap_or_default();
                let mut p = progress.lock().unwrap();
                if p.state != State::Stopped {
                    p.speed = 0.0;
                    p.merging = None;
                    p.merge_failed = Some(kept);
                    p.state = State::Failed(error);
                }
                return;
            }
            Attempt::Failed { error, rerouted } => {
                end(child);
                let gone: Vec<String> = servers::unreachable_in(&error)
                    .into_iter()
                    .filter(|h| !dead.contains(h))
                    .collect();
                // On the page's own servers (no moving any more), an unreachable one is the end:
                // another look-up would go to it again (live 2026-11-05).
                if !gone.is_empty() && reroute {
                    for h in &gone {
                        note(
                            progress,
                            format!(
                                "The server {h} couldn't be reached; looking the page up again without it"
                            ),
                        );
                    }
                    dead.extend(gone);
                } else if servers::format_unavailable(&error) && rerouted && !looked_again {
                    looked_again = true;
                    note(progress, format!("{error}; looking the page up again"));
                } else if servers::format_unavailable(&error)
                    && relaxed_to.is_none()
                    && let Some(sel) = selector_of(&ours).and_then(relaxed)
                {
                    note(
                        progress,
                        "The chosen quality isn't offered by the servers this computer can reach; downloading the nearest quality below it".into(),
                    );
                    ours = with_selector(&ours, &sel);
                    relaxed_to = Some(sel);
                } else if servers::format_unavailable(&error) && rerouted && reroute {
                    note(
                        progress,
                        "No video the servers this computer can reach serve is left; trying the page's own servers".into(),
                    );
                    reroute = false;
                } else if rerouted && !looked_again {
                    looked_again = true;
                    note(progress, format!("{error}; looking the page up again"));
                } else {
                    last = error;
                    break;
                }
                last = error;
            }
        }
    }
    end(child);
    let mut p = progress.lock().unwrap();
    if p.state != State::Stopped {
        p.speed = 0.0;
        p.state = State::Failed(last);
    }
}

/// The most the bar shows while yt-dlp still works: all the bytes in is not the end, another
/// stream or the merge may follow (#344). The window's percentage takes it from here; the sizes
/// stay what they are.
pub const MAX_WHILE_RUNNING: f64 = 0.999;

/// yt-dlp's progress lines (`MDMP <downloaded> <total> <estimate> <speed> <format id>`) made
/// into one steady figure. Each stream of a merged download (video, then audio) reports from 0
/// with its own total, and a fragmented stream's total (HLS, DASH) is an estimate that moves with
/// every fragment: the streams are added up, and the fraction done goes back only when a stream's
/// end had looked like the whole (no `expected`, the next stream starts).
///
/// The size the video window showed (`expected`, every stream) holds the total up until the last
/// stream: a stream's end before it never looks like the whole download (#344). That size is
/// often an estimate (an HLS stream's bitrate, a peak, times its length) well above what comes:
/// in the last stream, once yt-dlp knows that stream's own total, the total is what came before
/// and that stream's, so the bar reaches the end. Which streams there are, yt-dlp says before it
/// downloads ([`Meter::quality`]), and each line names the one it is for ([`Meter::named`]).
///
/// A line named otherwise (no `MDMQ`, an id it didn't name, `NA`) is read as before: back near 0
/// is the next stream, never counted as one of `MDMQ`'s, and the window's size holds the total.
/// Subtitle files come before the video with lines of their own, `NA` (#68): what they brought
/// leaves the bar when the first stream's line comes.
struct Meter {
    expected: Option<u64>,
    /// `MDMQ`'s format id and resolution (`137+140 1920x1080`), once yt-dlp has said.
    quality: Option<String>,
    /// The stream the lines are for now (its format id), how many of `MDMQ`'s streams have
    /// started, and whether a line was for all of them at once (`137+140`).
    current: Option<String>,
    started: usize,
    all_at_once: bool,
    /// Every line so far said `NA` for its format id (subtitle files).
    only_na: bool,
    /// The streams already finished, and the current one's last total and bytes.
    finished: u64,
    last_total: u64,
    last_got: u64,
    /// The highest fraction shown so far.
    frac: f64,
}

impl Meter {
    fn new(expected: Option<u64>) -> Self {
        Meter {
            expected: expected.filter(|e| *e > 0),
            quality: None,
            current: None,
            started: 0,
            all_at_once: false,
            only_na: true,
            finished: 0,
            last_total: 0,
            last_got: 0,
            frac: 0.0,
        }
    }

    /// yt-dlp's `MDMQ <format id> <resolution>` ([`QUALITY`]): a merged choice's id names each
    /// stream (`137+140`).
    fn quality(&mut self, rest: &str) {
        let rest = rest.trim();
        self.quality = (!rest.is_empty()).then(|| rest.to_owned());
    }

    /// How many streams `MDMQ` named.
    fn streams(&self) -> Option<usize> {
        self.quality.as_ref().map(|q| q.split('+').count())
    }

    /// Whether a line's format `id` is one of the streams `MDMQ` named (`137` of `137+140`;
    /// false), or all of them at once (`137+140`, as ffmpeg downloads and merges them; true).
    /// None for any other: none named, `NA`, an id `MDMQ` didn't name, no `MDMQ`.
    fn named<'a>(&self, id: Option<&'a str>) -> Option<(&'a str, bool)> {
        let (q, id) = (self.quality.as_deref()?, id.filter(|id| *id != "NA")?);
        // The id, then the resolution after the last one (one word, or two: `audio only`).
        let is = |s: &str| {
            s.strip_prefix(id)
                .is_some_and(|r| r.is_empty() || r.starts_with(' '))
        };
        if q.split('+').any(is) {
            Some((id, false))
        } else {
            is(q).then_some((id, true))
        }
    }

    /// The next stream started: the one before counts in full.
    fn next_stream(&mut self) {
        self.finished += self.last_total.max(self.last_got);
        self.last_total = 0;
        self.last_got = 0;
        // Without the whole size from the start, the first stream's end looked like the
        // whole download: the next stream's total pulls the bar back (#344).
        if self.expected.is_none() {
            self.frac = 0.0;
        }
    }

    fn line(&mut self, rest: &str, p: &mut Progress) {
        let fields: Vec<&str> = rest.trim().splitn(5, ' ').collect();
        let f: Vec<Option<f64>> = fields.iter().take(4).map(|v| v.parse().ok()).collect();
        let mut got = f.first().copied().flatten().unwrap_or(0.0).max(0.0) as u64;
        let total = f
            .get(1)
            .copied()
            .flatten()
            .or(f.get(2).copied().flatten())
            .map(|t| t.max(0.0) as u64);
        // Only an estimate, and no more than what came: yt-dlp's first line for a stream in
        // pieces, before it knows how many there are (1 KB of 1 KB), or its last one. It says
        // nothing of how much is left, and the bar must not hold on to its end (#622).
        let blind = f.get(1).copied().flatten().is_none() && total.is_some_and(|t| t <= got);
        let id = fields
            .get(4)
            .map(|id| id.trim())
            .filter(|id| !id.is_empty());
        let named = self.named(id);
        match named {
            // Each stream by its own id: a count from 0 again within one (a retry from the
            // start) is not the next stream, and the bar waits there until it is back where it
            // was.
            Some((id, _)) if self.current.as_deref() == Some(id) => {
                got = got.max(self.last_got);
            }
            Some((id, all)) => {
                if self.only_na {
                    // Subtitle files came first (#68): theirs are not the video's bytes (the bar
                    // gives back their share of the whole, a sliver beside a video's).
                    self.finished = 0;
                    self.last_total = 0;
                    self.last_got = 0;
                    self.frac = 0.0;
                } else if self.current.is_some() || self.last_got > 0 {
                    self.next_stream();
                }
                if all {
                    self.all_at_once = true;
                } else {
                    self.started += 1;
                }
                self.current = Some(id.to_owned());
                self.only_na = false;
            }
            // As before: back near 0 is the next stream.
            None => {
                let back = got < self.last_got;
                if back && self.last_got > 0 {
                    self.next_stream();
                }
                self.only_na &= id == Some("NA");
            }
        }
        self.last_got = got;
        if let Some(t) = total {
            self.last_total = t;
        }
        let downloaded = self.finished + got;
        p.downloaded = downloaded;
        p.speed = f.get(3).copied().flatten().unwrap_or(0.0);
        // Neither yt-dlp nor the video window knows how much is coming (a live stream, some
        // HLS): no percentage rather than what came so far as the whole, 100 % (#344).
        if self.last_total == 0 && self.expected.is_none() {
            p.total = None;
            return;
        }
        // Such a line leaves the bar where it was (none yet: no percentage). The stream's bytes
        // still count in full when the next one starts.
        if blind {
            p.total = (self.frac > 0.0)
                .then(|| ((downloaded as f64 / self.frac).round() as u64).max(downloaded + 1));
            return;
        }
        let mut whole = self.finished + self.last_total.max(got);
        // The window's size stands for the streams still to come (less what is known so far);
        // in the last stream, once its own total is known, nothing is left to come. When that
        // size was an estimate above what came, the bar lurches forward there (a merged
        // download's audio, after its video: from about the video's share of the estimate to
        // its share of what came); never back, and it glides there in the progress window.
        let last = named.is_some()
            && (self.all_at_once || self.streams().is_some_and(|n| self.started >= n));
        if let Some(e) = self.expected
            && !(last && self.last_total > 0)
        {
            whole = whole.max(e);
        }
        if whole == 0 {
            p.total = None;
            return;
        }
        let frac = (downloaded as f64 / whole as f64).min(MAX_WHILE_RUNNING);
        // When the estimate grew, the bar stays where it was rather than move back. Otherwise
        // the total is the whole, as big as it is: at the end it isn't inflated to read 99.9 %.
        p.total = Some(if frac >= self.frac {
            self.frac = frac;
            whole
        } else {
            ((downloaded as f64 / self.frac)
                .round()
                .max(whole.min(downloaded + 1) as f64)) as u64
        });
    }
}

/// How one yt-dlp run on the looked-up page ended.
struct Ran {
    status: std::io::Result<std::process::ExitStatus>,
    /// Where it said it saved the file (`MDMF`).
    saved: Option<PathBuf>,
    /// The quality it chose (`MDMQ`).
    quality: Option<String>,
    warnings: Vec<String>,
    /// What it said when it failed.
    failure: String,
    /// The merge's file when yt-dlp said it merges next and never that it was done.
    merging: Option<PathBuf>,
}

/// Run yt-dlp with `ours` on the page looked up into `file`, following its progress with
/// `meter`. None: stopped before or while it ran.
fn run(
    exe: &Path,
    ours: &[String],
    file: &Path,
    progress: &Mutex<Progress>,
    child: &Child,
    mut meter: Option<&mut Meter>,
) -> Result<Option<Ran>, String> {
    let stdout = {
        // Under the child's lock: stop() either came first (nothing starts) or finds the child
        // to kill.
        let mut slot = child.lock().unwrap();
        if progress.lock().unwrap().state == State::Stopped {
            return Ok(None);
        }
        let spawned =
            proc::ytdlp_info(exe, &[], ours, file).and_then(|cmd| proc::spawn(cmd, "yt-dlp"));
        match spawned {
            Ok(mut s) => {
                let out = s.child.stdout.take().expect("piped");
                *slot = Some(s);
                out
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let mut saved = None;
    let mut quality = None;
    progress.lock().unwrap().merging = None;
    // The merge's file, once yt-dlp says it's next; watched until the reading ends.
    let merged: Mutex<Option<PathBuf>> = Mutex::new(None);
    let read_all = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| watch_merge(&merged, &read_all, progress));
        // Every line to the end, read as bytes (#79): a line that isn't UTF-8 (a title in a
        // legacy code page) used to end the reading there, while yt-dlp went on; it is read
        // leniently now, and the saved file is also found by its name (`attempt`).
        let mut reader = BufReader::new(stdout);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&buf);
            let line = text.trim_end_matches(['\r', '\n']);
            if let Some(rest) = line.strip_prefix("MDMF ") {
                saved = Some(PathBuf::from(rest.trim()));
                // Merged and moved: done joining.
                *merged.lock().unwrap() = None;
                progress.lock().unwrap().merging = None;
            } else if let Some(rest) = line.strip_prefix("MDMQ ") {
                if let Some(m) = meter.as_deref_mut() {
                    m.quality(rest);
                }
                quality = Some(rest.trim().to_owned());
            } else if let Some(rest) = line.strip_prefix("MDMP ")
                && let Some(m) = meter.as_deref_mut()
            {
                m.line(rest, &mut progress.lock().unwrap());
            } else if let Some(rest) = line.strip_prefix("MDMM ")
                && quality.as_deref().is_some_and(several_streams)
            {
                // Several streams: ffmpeg joins them now (#736). Told by this run's `MDMQ`, not
                // by the meter: when MDM's engine downloaded the streams (#769) there is none,
                // and that is how most videos come down.
                *merged.lock().unwrap() = Some(merge_file(Path::new(rest.trim())));
                progress.lock().unwrap().merging = Some(None);
            }
        }
        read_all.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let merging = merged.into_inner().unwrap_or_else(|e| e.into_inner());
    // Poll (the lock is free in between, so stop() can still kill it).
    let status = loop {
        let mut slot = child.lock().unwrap();
        let Some(s) = slot.as_mut() else {
            break Err(std::io::Error::other("yt-dlp is gone"));
        };
        match s.child.try_wait() {
            Ok(Some(s)) => break Ok(s),
            Ok(None) => {}
            Err(e) => break Err(e),
        }
        drop(slot);
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    std::thread::sleep(std::time::Duration::from_millis(50));
    if progress.lock().unwrap().state == State::Stopped {
        return Ok(None);
    }
    let slot = child.lock().unwrap();
    let (warnings, failure) = slot.as_ref().map_or_else(
        || (Vec::new(), "yt-dlp stopped".to_owned()),
        |s| (s.warnings(), s.failure().to_string()),
    );
    Ok(Some(Ran {
        status,
        saved,
        quality,
        warnings,
        failure,
        merging,
    }))
}

/// The file yt-dlp's merge writes for `path` before it renames it there: `<stem>.temp.<ext>`
/// (yt-dlp's `prepend_extension`).
fn merge_file(path: &Path) -> PathBuf {
    match (path.file_stem(), path.extension()) {
        (Some(stem), Some(ext)) => {
            let mut name = stem.to_os_string();
            name.push(".temp.");
            name.push(ext);
            path.with_file_name(name)
        }
        _ => {
            let mut name = path.as_os_str().to_os_string();
            name.push(".temp");
            PathBuf::from(name)
        }
    }
}

/// How far a merge is (#736): the joined file's size against the streams' (ffmpeg copies them
/// as they are, so it ends about as big), read every quarter second while it is `merged`; at
/// most 99 %, the rename is still to come, and never back ([`merge_step`]). Ends when the
/// reading does.
fn watch_merge(
    merged: &Mutex<Option<PathBuf>>,
    read_all: &std::sync::atomic::AtomicBool,
    progress: &Mutex<Progress>,
) {
    while !read_all.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let Some(file) = merged.lock().unwrap().clone() else {
            continue;
        };
        let Ok(len) = std::fs::metadata(&file).map(|m| m.len()) else {
            continue;
        };
        let mut p = progress.lock().unwrap();
        if let Some(was) = p.merging
            && p.downloaded > 0
        {
            p.merging = Some(Some(merge_step(was, merge_fraction(len, p.downloaded))));
        }
    }
}

/// Whether `MDMQ`'s quality (`137+140 1920x1080`) is a choice of several streams, which
/// yt-dlp's ffmpeg merges.
fn several_streams(quality: &str) -> bool {
    quality
        .split_whitespace()
        .next()
        .is_some_and(|ids| ids.contains('+'))
}

/// A merge's share done: `written` of about `streams` bytes, held short of the end.
fn merge_fraction(written: u64, streams: u64) -> f64 {
    (written as f64 / streams.max(1) as f64).min(0.99)
}

/// The merge's share after a reading of `now`, from `was` (none yet): never back. The file can
/// read smaller than before (empty a moment while it is written again, made again by a second
/// pass, read between ffmpeg's truncating and writing it): the bar holds where it was.
fn merge_step(was: Option<f64>, now: f64) -> f64 {
    was.map_or(now, |was| was.max(now))
}

/// The media file yt-dlp saved under our output name (`<dir>/<stem>.%(ext)s`), when it didn't say
/// where, or said it in a form that names no file (#79). Its subtitles (`<stem>.<lang>.srt`) and
/// unfinished parts don't count; the newest one wins.
fn saved_by_name(ours: &[String]) -> Option<PathBuf> {
    const MEDIA: [&str; 15] = [
        "mp4", "mkv", "webm", "mov", "m4v", "m4a", "mp3", "opus", "ogg", "oga", "flac", "wav",
        "aac", "flv", "3gp",
    ];
    let (dir, stem) = template_of(ours)?;
    MEDIA
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .filter(|p| p.is_file())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

/// What a failed merge left in `dir` for `<stem>.%(ext)s`: what [`own_files`] tells (unfinished
/// files, streams named by a numbered or `hls-` id), and each stream of format `ids` by its name,
/// `<stem>.f<id>.<ext>`, however yt-dlp named it (`Talk.faud-English.mp4`, an HLS sound's).
fn kept_parts(dir: &Path, stem: &str, ids: &[String]) -> Vec<PathBuf> {
    let mut kept = own_files(dir, stem, false);
    let named: Vec<String> = ids
        .iter()
        .map(|id| format!("{}.f{}.", stem, id).to_lowercase())
        .collect();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let whole = named.iter().any(|n| {
                name.strip_prefix(n.as_str())
                    .is_some_and(|ext| MEDIA.contains(&ext))
            });
            if whole && e.file_type().is_ok_and(|t| t.is_file()) && !kept.contains(&e.path()) {
                kept.push(e.path());
            }
        }
    }
    kept.sort();
    kept
}

/// The folder and the file stem of our output template (`-o <dir>/<stem>.%(ext)s`).
fn template_of(ours: &[String]) -> Option<(PathBuf, String)> {
    let template = ours
        .iter()
        .position(|a| a == "-o")
        .and_then(|i| ours.get(i + 1))?;
    let t = Path::new(template);
    let stem = t
        .file_name()?
        .to_string_lossy()
        .strip_suffix(".%(ext)s")?
        .replace("%%", "%");
    Some((t.parent()?.to_path_buf(), stem))
}

/// What a saved file holds when its name says otherwise: an HLS stream's pieces joined as they
/// came, which only ffmpeg makes into the MP4 they're named for (#569). YouTube's HLS sound is
/// raw AAC (ADTS frames, each piece led by an ID3 tag with its time): `aac`, which players open.
/// A video's pieces are an MPEG transport stream: `ts`. A real MP4 (ffmpeg was there, or the
/// format came as a file) is none of these.
fn content_ext(head: &[u8]) -> Option<&'static str> {
    // ID3v2 tags first: a 10-byte header, then the tag's size in 7-bit bytes.
    let mut at = 0usize;
    while head.len() >= at + 10 && &head[at..at + 3] == b"ID3" {
        let size = head[at + 6..at + 10]
            .iter()
            .fold(0usize, |n, b| (n << 7) | usize::from(b & 0x7f));
        let footer = if head[at + 5] & 0x10 != 0 { 10 } else { 0 };
        at += 10 + size + footer;
    }
    let rest = head.get(at..)?;
    // ADTS: a 12-bit sync word, then MPEG layer 0.
    if rest.len() >= 7 && rest[0] == 0xff && rest[1] & 0xf6 == 0xf0 {
        return Some("aac");
    }
    // A transport stream: a sync byte every 188 bytes.
    if at == 0 && head.len() > 376 && [0, 188, 376].iter().all(|&i| head[i] == 0x47) {
        return Some("ts");
    }
    None
}

/// `f`, renamed for what it holds when its MP4 name is wrong ([`content_ext`]); as it was
/// otherwise, or when the rename fails (the file is kept).
fn named_for_content(f: PathBuf) -> PathBuf {
    let named = f
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    if !matches!(named.as_deref(), Some("mp4" | "m4a")) {
        return f;
    }
    let mut head = Vec::with_capacity(8192);
    let read = std::fs::File::open(&f).and_then(|file| {
        use std::io::Read;
        file.take(8192).read_to_end(&mut head)
    });
    let Some(ext) = read.ok().and_then(|_| content_ext(&head)) else {
        return f;
    };
    let stem = f
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let free = (1..100)
        .map(|n| match n {
            1 => f.with_file_name(format!("{stem}.{ext}")),
            n => f.with_file_name(format!("{stem} ({n}).{ext}")),
        })
        .find(|p| !p.exists());
    match free {
        Some(to) if std::fs::rename(&f, &to).is_ok() => to,
        _ => f,
    }
}

/// Wait `d`, or less if the download is stopped meanwhile (false then).
fn pause(d: std::time::Duration, progress: &Mutex<Progress>) -> bool {
    let end = std::time::Instant::now() + d;
    while std::time::Instant::now() < end {
        if progress.lock().unwrap().state == State::Stopped {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    progress.lock().unwrap().state != State::Stopped
}

/// The page as the quality menu's look-up read it (`preloaded`), once yt-dlp has answered there
/// (waiting for it as long as that look-up may take, [`LOOKUP_WAIT`]) and if it offers the choice
/// the menu showed; else none, and the run looks the page up itself.
fn preloaded_page(
    preloaded: &Preloaded,
    progress: &Mutex<Progress>,
) -> Option<(serde_json::Value, bool)> {
    let until = preloaded.slot.born() + LOOKUP_WAIT;
    let stopped = || progress.lock().unwrap().state == State::Stopped;
    let (json, without_cookies) = preloaded.slot.wait(until, stopped)?;
    let info = offered(
        &json,
        &preloaded.selector,
        &preloaded.label,
        preloaded.merge,
    )?;
    Some((info, without_cookies))
}

/// One run: look the page up (or take the quality menu's look-up, `preloaded`), move links off
/// unreachable servers (and off `dead`, the ones an earlier run couldn't reach), download from
/// that list. Subtitles the site refused are asked for once more after a pause (`retry`), beside
/// the video; the ones still refused are reported with it, never failing it.
#[allow(clippy::too_many_arguments)]
fn attempt(
    exe: &Path,
    url: &str,
    ours: &[String],
    browser: &[String],
    progress: &Mutex<Progress>,
    child: &Child,
    expected: Option<u64>,
    retry: Option<&SubsRetry>,
    dead: &std::collections::BTreeSet<String>,
    reroute: bool,
    preloaded: Option<&Preloaded>,
    engine: Option<&ByEngine>,
) -> Attempt {
    let failed = |error: String, rerouted: bool| Attempt::Failed { error, rerouted };
    let page = match preloaded.and_then(|p| preloaded_page(p, progress)) {
        Some(page) => Ok(page),
        // Stopped while waiting for it: nothing more to do.
        None if progress.lock().unwrap().state == State::Stopped => return Attempt::Stopped,
        None => look_up(exe, url, browser),
    };
    let mut info = match page {
        Ok((v, without_cookies)) => {
            if without_cookies {
                note(
                    progress,
                    "The site offered no format with the browser's cookies; looked the page up without them".into(),
                );
            }
            v
        }
        Err(e) => return failed(e, false),
    };
    let changed = moved(&mut info, dead, reroute);
    for (gone, alt) in &changed.swaps {
        note(
            progress,
            format!("The server {gone} can't be reached; downloading from {alt}"),
        );
    }
    if !changed.left_out.is_empty() {
        note(
            progress,
            format!(
                "Not served by the servers this computer can reach, so left out: {}",
                changed.left_out.join(", ")
            ),
        );
    }
    let rerouted = !changed.is_empty();
    let file = info_file();
    if let Err(e) = std::fs::write(&file, info.to_string()) {
        return failed(format!("Cannot write the video information: {e}"), rerouted);
    }
    let _cleanup = Remove(file.clone());
    // MDM's engine downloads the streams (#769); yt-dlp then finds them in place.
    let mut by_engine = false;
    if let Some(e) = engine {
        match e.fetch(exe, url, browser, &file, progress, dead, reroute) {
            Engined::Ready => by_engine = true,
            Engined::Helper(why) => {
                note(
                    progress,
                    format!("yt-dlp downloads this video itself: {}", why.english()),
                );
                progress.lock().unwrap().helper = Some(why);
            }
            Engined::Stopped => return Attempt::Stopped,
            Engined::Failed(error) => return failed(error, rerouted),
        }
    }
    let mut meter = Meter::new(expected);
    // The streams are in place: what yt-dlp says of them isn't the download's progress.
    let meter = (!by_engine).then_some(&mut meter);
    let ran = match run(exe, ours, &file, progress, child, meter) {
        Ok(Some(r)) => r,
        Ok(None) => return Attempt::Stopped,
        Err(e) => return failed(e, rerouted),
    };
    let quality = ran.quality.clone();
    let video = match (&ran.status, ran.saved) {
        // `--ignore-errors` (subtitles asked) lets yt-dlp end well with no file: the file
        // itself is the proof.
        (Ok(s), Some(f)) if s.success() && f.is_file() => f,
        // It said where, in a form that names no file (#79), or didn't say: our own name for it.
        (Ok(s), _) if s.success() && saved_by_name(ours).is_some() => {
            saved_by_name(ours).expect("just found")
        }
        (Ok(s), Some(_)) if s.success() => {
            return failed(
                "yt-dlp said it saved the file, but it isn't there".into(),
                rerouted,
            );
        }
        (Ok(s), None) if s.success() => {
            return failed("yt-dlp didn't say where it saved the file".into(), rerouted);
        }
        // Every stream in, and joining them failed (yt-dlp said it merges next, never that it
        // was done): what ffmpeg left half written goes.
        _ if let Some(temp) = &ran.merging => {
            let _ = std::fs::remove_file(temp);
            let ids = ran
                .quality
                .as_deref()
                .and_then(|q| q.split_whitespace().next())
                .map(|ids| ids.split('+').map(str::to_owned).collect())
                .unwrap_or_default();
            return Attempt::MergeFailed(ran.failure, ids);
        }
        _ => return failed(ran.failure, rerouted),
    };
    let refused = subtitle_failures(&ran.warnings);
    if refused.is_empty() {
        return Attempt::Done(video, Vec::new(), quality);
    }
    for (code, why) in &refused {
        note(
            progress,
            format!(
                "Subtitles in {code} couldn't be downloaded: {}",
                english(why)
            ),
        );
    }
    // Once more after a pause, for the ones refused (a site limiting requests often gives them
    // a little later).
    let mut still = refused.clone();
    if let Some(r) = retry {
        let delay = subs_retry_delay();
        note(
            progress,
            format!("Asking for the subtitles again in {} s", delay.as_secs()),
        );
        if !pause(delay, progress) {
            return Attempt::Done(video, Vec::new(), quality);
        }
        let codes: Vec<String> = refused.iter().map(|(c, _)| c.clone()).collect();
        match run(exe, &r.args(&codes), &file, progress, child, None) {
            Ok(Some(again)) if again.status.as_ref().is_ok_and(|s| s.success()) => {
                still = subtitle_failures(&again.warnings);
            }
            Ok(None) => return Attempt::Done(video, Vec::new(), quality.clone()),
            _ => {}
        }
    }
    let mut problems = Vec::new();
    for (code, why) in &refused {
        let gone = still.iter().find(|(c, _)| c == code);
        match gone {
            Some((_, why)) => problems.push(SubProblem {
                code: code.clone(),
                name: sub_name(&info, code),
                reason: why.clone(),
                beside: false,
            }),
            None => {
                note(
                    progress,
                    format!("Subtitles in {code} came on the second try"),
                );
                // Asked into the video, they're beside it now.
                if retry.is_some_and(|r| r.embed) {
                    problems.push(SubProblem {
                        code: code.clone(),
                        name: sub_name(&info, code),
                        reason: why.clone(),
                        beside: true,
                    });
                }
            }
        }
    }
    Attempt::Done(video, problems, quality)
}

/// Move `info`'s links off servers that can't be reached (and off `dead`, the ones an earlier run
/// couldn't reach), when `reroute`.
fn moved(
    info: &mut serde_json::Value,
    dead: &std::collections::BTreeSet<String>,
    reroute: bool,
) -> servers::Rerouted {
    let checks = *SERVER_CHECKS.read().unwrap_or_else(|e| e.into_inner());
    if let (true, Some((alive, serves))) = (reroute, checks) {
        servers::reroute(info, |h| !dead.contains(h) && alive(h), |l, _| serves(l))
    } else if reroute {
        let prober = servers::Prober::new();
        servers::reroute(
            info,
            |h| !dead.contains(h) && servers::reachable(h),
            |l, h| prober.serves(l, h),
        )
    } else {
        servers::Rerouted::default()
    }
}

/// yt-dlp's choice (`-O "%()j"` after its format selection) on the page looked up into `file`,
/// with `args` (the choice's `-f`, `-o`, merge and conversion, the browser's context).
fn choose(exe: &Path, args: &[String], file: &Path) -> Result<serde_json::Value, String> {
    let mut ours = args.to_vec();
    ours.extend(["-O".into(), CHOICE.into()]);
    let cmd = proc::ytdlp_info(exe, &[], &ours, file).map_err(|e| e.to_string())?;
    let out = proc::output(cmd, "yt-dlp", Duration::from_secs(90)).map_err(|e| e.to_string())?;
    String::from_utf8_lossy(&out)
        .lines()
        .find_map(|l| l.strip_prefix("MDMJ "))
        .and_then(|j| serde_json::from_str(j).ok())
        .ok_or_else(|| "yt-dlp gave no choice of formats".to_owned())
}

/// yt-dlp's choice as a line of JSON: `MDMJ {…}` (`filename` and the chosen formats in it).
const CHOICE: &str = "MDMJ %()j";

/// MDM's engine for a video's streams (#769): the engine itself, the arguments yt-dlp chooses
/// the formats with (`choose`), and the download's engine part under way.
pub(crate) struct ByEngine {
    engine: crate::streams::Engine,
    choose: Vec<String>,
    current: Arc<crate::streams::Current>,
}

/// How the engine's part of a run ended.
enum Engined {
    /// Every stream is in place for yt-dlp.
    Ready,
    /// yt-dlp downloads this choice itself.
    Helper(crate::streams::Helper),
    Stopped,
    Failed(String),
}

/// The engine's progress into the download's (`Progress`).
struct Reporter<'a>(&'a Mutex<Progress>);

impl crate::streams::Report for Reporter<'_> {
    fn progress(
        &self,
        downloaded: u64,
        total: Option<u64>,
        speed: f64,
        view: crate::streams::View,
    ) {
        let mut p = self.0.lock().unwrap();
        p.downloaded = downloaded;
        p.total = total;
        p.speed = speed;
        p.engine = Some(view);
    }

    fn note(&self, text: String) {
        note(self.0, text);
    }

    fn stopped(&self) -> bool {
        self.0.lock().unwrap().state == State::Stopped
    }
}

impl ByEngine {
    /// Choose the formats on the page looked up into `file` and download their streams with the
    /// engine; a stream's address that dies is asked for again with a new look-up (`url`, as
    /// the browser asked, `browser`; servers moved as this run moved them).
    #[allow(clippy::too_many_arguments)]
    fn fetch(
        &self,
        exe: &Path,
        url: &str,
        browser: &[String],
        file: &Path,
        progress: &Mutex<Progress>,
        dead: &std::collections::BTreeSet<String>,
        reroute: bool,
    ) -> Engined {
        let chosen = match choose(exe, &self.choose, file) {
            Ok(c) => c,
            Err(e) => return Engined::Failed(e),
        };
        if progress.lock().unwrap().state == State::Stopped {
            return Engined::Stopped;
        }
        let plan = match crate::streams::plan(&chosen) {
            Ok(p) => p,
            Err(why) => return Engined::Helper(why),
        };
        let Ok(page) = url::Url::parse(url) else {
            return Engined::Failed("not a web address".into());
        };
        {
            let mut p = progress.lock().unwrap();
            p.helper = None;
            p.total = plan.total().or(p.total);
        }
        let relink: crate::streams::Relink = {
            let (exe, url, browser) = (exe.to_owned(), url.to_owned(), browser.to_vec());
            let (choose, dead) = (self.choose.clone(), dead.clone());
            Arc::new(move |id: &str| relink(&exe, &url, &browser, &choose, &dead, reroute, id))
        };
        let out = crate::streams::fetch(
            &plan,
            &self.engine,
            &page,
            &relink,
            &self.current,
            &Reporter(progress),
        );
        // The engine's part is over: no connection receives while yt-dlp joins the streams
        // (#736); the last view taken of a stream in pieces still had them under way.
        let mut p = progress.lock().unwrap();
        p.speed = 0.0;
        if let (crate::streams::Fetched::Done, Some(view)) = (&out, &mut p.engine) {
            view.connections
                .iter_mut()
                .for_each(mdm_engine::download::ConnView::finish);
        }
        drop(p);
        match out {
            crate::streams::Fetched::Done => Engined::Ready,
            crate::streams::Fetched::Stopped => Engined::Stopped,
            crate::streams::Fetched::Failed(e) => Engined::Failed(e),
            crate::streams::Fetched::Helper(why) => Engined::Helper(why),
        }
    }
}

/// Format `id` on the page as yt-dlp gives it today (its address, headers, pieces): looked up
/// again (as the browser asked, `browser`), its servers moved as the run moved them, and chosen
/// with `-f <id>`.
fn relink(
    exe: &Path,
    url: &str,
    browser: &[String],
    choose_args: &[String],
    dead: &std::collections::BTreeSet<String>,
    reroute: bool,
    id: &str,
) -> Result<serde_json::Value, String> {
    let (mut info, _) = look_up(exe, url, browser)?;
    moved(&mut info, dead, reroute);
    let file = info_file();
    std::fs::write(&file, info.to_string())
        .map_err(|e| format!("Cannot write the video information: {e}"))?;
    let _cleanup = Remove(file.clone());
    let chosen = choose(exe, &with_selector(choose_args, id), &file)?;
    crate::streams::format_of(&chosen, id)
        .cloned()
        .ok_or_else(|| format!("the page no longer offers format {id}"))
}

/// A line for the download's log (English, as yt-dlp's own).
fn note(progress: &Mutex<Progress>, text: String) {
    progress
        .lock()
        .unwrap()
        .events
        .push((std::time::SystemTime::now(), text));
}

/// A subtitle refusal in the log's words.
fn english(r: &SubReason) -> String {
    match r {
        SubReason::TooManyRequests => "the site refused (too many requests)".into(),
        SubReason::Other(why) => why.clone(),
    }
}

/// Removes a file when dropped.
struct Remove(PathBuf);

impl Drop for Remove {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `%` would be read as a template field by yt-dlp.
fn template_text(s: &str) -> String {
    s.replace('%', "%%")
}

/// yt-dlp's speed when it downloads by itself (#708; MDM's engine downloads every other video,
/// #769): at most `rate` bytes/s.
pub fn limit_args(rate: Option<u64>) -> Vec<String> {
    rate.map(|r| vec!["--limit-rate".into(), r.to_string()])
        .unwrap_or_default()
}

/// Our progress line: `MDMP <downloaded> <total> <estimate> <speed> <format id>` (`NA` when
/// unknown; the format id is the stream's, `NA` for a subtitle file).
const PROGRESS: &str = "download:MDMP %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s %(info.format_id)s";
/// The final file: `MDMF <path>`.
const FINAL: &str = "after_move:MDMF %(filepath)s";
/// Every stream is in, the file they make is next (`MDMM <path>`): printed before yt-dlp's
/// post-processing, its merge first (#736).
const MERGE: &str = "post_process:MDMM %(filepath)s";
/// The quality yt-dlp chose, before it downloads: `MDMQ <format id> <resolution>` (the log names
/// it when it isn't the one asked for, #94).
const QUALITY: &str = "video:MDMQ %(format_id)s %(resolution)s";

impl VideoJob {
    /// Download `url` in format `selector` to `dir/<stem>.<ext>`; when yt-dlp downloads it by
    /// itself, at most `rate` bytes/s ([`limit_args`]). `ffmpeg` (its exe) merges separate video and audio into an MP4. `expected`: the size the
    /// choice was shown with (every stream), so the progress starts from the whole. `browser`:
    /// what the browser's request for the page carried (proc::FromBrowser). `subs`: the
    /// subtitles to save with it (#68; none for a sound alone, which has no picture to put them
    /// on). `engine`: MDM's engine, which downloads the streams yt-dlp finds (#769; its options'
    /// speed limit is the download's); without it, or for what it can't download, yt-dlp does.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        exe: &Path,
        url: &str,
        selector: &str,
        dir: &Path,
        stem: &str,
        rate: Option<u64>,
        ffmpeg: Option<&Path>,
        expected: Option<u64>,
        browser: &proc::FromBrowser,
        subs: &Subs,
        engine: Option<crate::streams::Engine>,
    ) -> Result<VideoJob, ProcError> {
        Self::start_with(
            exe, url, selector, dir, stem, rate, ffmpeg, expected, browser, subs, None, engine,
        )
    }

    /// [`VideoJob::start`], the first run taking the quality menu's look-up (`preloaded`) when
    /// it is for this very choice.
    #[allow(clippy::too_many_arguments)]
    pub fn start_with(
        exe: &Path,
        url: &str,
        selector: &str,
        dir: &Path,
        stem: &str,
        rate: Option<u64>,
        ffmpeg: Option<&Path>,
        expected: Option<u64>,
        browser: &proc::FromBrowser,
        subs: &Subs,
        preloaded: Option<Preloaded>,
        engine: Option<crate::streams::Engine>,
    ) -> Result<VideoJob, ProcError> {
        proc::safe(url)?;
        let preloaded = preloaded.filter(|p| p.selector == selector);
        // A full path: yt-dlp runs in a folder of its own (#834).
        let output = crate::workdir::absolute(dir).join(format!("{}.%(ext)s", template_text(stem)));
        // An MP3, Opus or FLAC choice (#32, #70): the best sound, converted by ffmpeg.
        let convert = crate::resolve::audio_conversion(selector);
        let format = if convert.is_some() {
            "ba/b".to_owned()
        } else if ffmpeg.is_none() {
            // A format that comes as a file first (#569): without ffmpeg an HLS stream stays
            // its raw pieces.
            crate::resolve::without_remuxing(selector)
        } else {
            selector.to_owned()
        };
        // What comes down is the sound as the site has it, not the file the choice's size
        // estimated: the progress follows yt-dlp's own total.
        let expected = if convert.is_some() { None } else { expected };
        // Subtitles go with a picture only (none for a sound).
        let with_subs = convert.is_none() && !subs.langs.is_empty();
        // How yt-dlp chooses the formats for the engine (#769): the same choice, merge and
        // conversion as the download's run, without downloading.
        let mut choose: Vec<String> = vec![
            "-f".into(),
            format.clone(),
            "-o".into(),
            output.to_string_lossy().into_owned(),
            "--no-playlist".into(),
        ];
        let mut ours: Vec<String> = vec![
            "-f".into(),
            format,
            "-o".into(),
            output.to_string_lossy().into_owned(),
            "--no-playlist".into(),
            "--newline".into(),
            // `--print` makes yt-dlp quiet; this keeps the progress lines.
            "--progress".into(),
            "--continue".into(),
            "--no-mtime".into(),
            "--progress-template".into(),
            PROGRESS.into(),
            "--print".into(),
            FINAL.into(),
            "--print".into(),
            QUALITY.into(),
            "--print".into(),
            MERGE.into(),
        ];
        ours.extend(limit_args(rate));
        // A subtitles-only second run, for the ones a site refuses (#68): the same output name.
        let mut retry_base: Vec<String> = vec![
            "-o".into(),
            output.to_string_lossy().into_owned(),
            "--no-playlist".into(),
        ];
        let converted = convert.is_some();
        match convert {
            Some(a) => {
                let x = ["-x".into(), "--audio-format".into(), a.format.into()];
                ours.extend(x.clone());
                choose.extend(x);
                if let Some(q) = a.quality {
                    ours.extend(["--audio-quality".into(), q]);
                }
            }
            None => ours.extend(subtitle_args(subs, ffmpeg.is_some())),
        }
        ours.extend(chapter_args(
            CHAPTERS.load(std::sync::atomic::Ordering::Relaxed),
            ffmpeg.is_some(),
            converted,
        ));
        let (theirs, cookies) = browser.args(url)?;
        retry_base.extend(theirs.iter().cloned());
        ours.extend(theirs.iter().cloned());
        choose.extend(theirs.iter().cloned());
        if let Some(dir) = ffmpeg.and_then(Path::parent) {
            let merge = [
                "--ffmpeg-location".into(),
                dir.to_string_lossy().into_owned(),
                "--merge-output-format".into(),
                "mp4".into(),
            ];
            ours.extend(merge.clone());
            choose.extend(merge);
            retry_base.extend([
                "--ffmpeg-location".into(),
                dir.to_string_lossy().into_owned(),
            ]);
        }
        // With the engine, every HLS stream is yt-dlp's hlsnative's (`--hls-prefer-native`): the
        // engine joins its pieces as hlsnative does, and yt-dlp's fixups after it then follow
        // (a live one hlsnative hands to ffmpeg, as before).
        if engine.is_some() {
            ours.push("--hls-prefer-native".into());
            choose.push("--hls-prefer-native".into());
        }
        let retry = with_subs.then(|| SubsRetry {
            automatic: subs.automatic,
            embed: subs.embed && ffmpeg.is_some(),
            ffmpeg: ffmpeg.is_some(),
            base: retry_base,
        });
        for a in retry.iter().flat_map(|r| r.base.iter()) {
            proc::safe(a)?;
        }
        for a in ours.iter().chain(&choose) {
            proc::safe(a)?;
        }
        // The engine's streams are there before yt-dlp's run: it fixes them up all the same
        // (`--fixup force`; by default only what it downloaded itself), as after its own
        // download: a DASH M4A sound, HLS pieces' MPEG-TS in an MP4.
        if engine.is_some() {
            ours.extend(["--fixup".into(), "force".into()]);
        }
        // Whose cookies yt-dlp has, for the log (#229): the browser's name only, never a
        // cookie or the profile (a folder may name the person).
        let events = match proc::browser_cookies() {
            Some(b) if cookies.is_none() => vec![(
                std::time::SystemTime::now(),
                format!(
                    "Cookies read from {} on this computer (Options → Sites Logins)",
                    b.browser
                ),
            )],
            _ => Vec::new(),
        };
        let progress = Arc::new(Mutex::new(Progress {
            state: State::Running,
            downloaded: 0,
            total: None,
            speed: 0.0,
            events,
            side_files: Vec::new(),
            sub_problems: Vec::new(),
            engine: None,
            helper: None,
            merging: None,
            merge_failed: None,
        }));
        let child: Child = Arc::new(Mutex::new(None));
        let current = Arc::new(crate::streams::Current::new(
            engine.as_ref().and_then(|e| e.opts.speed_limit),
        ));
        let by_engine = engine.map(|engine| ByEngine {
            engine,
            choose,
            current: current.clone(),
        });
        {
            let (progress, child) = (progress.clone(), child.clone());
            let (exe, url) = (exe.to_owned(), url.to_owned());
            std::thread::spawn(move || {
                // The cookie file lives as long as the run.
                let _cookies = cookies;
                // A panic in here must not leave the download "running" forever.
                let run = std::panic::AssertUnwindSafe(|| {
                    supervise(
                        &exe,
                        &url,
                        &ours,
                        &theirs,
                        &progress,
                        &child,
                        expected,
                        retry.as_ref(),
                        preloaded,
                        by_engine.as_ref(),
                    )
                });
                if std::panic::catch_unwind(run).is_err() {
                    // Given up: nothing may go on downloading unseen (#79).
                    end(&child);
                    if let Some(b) = &by_engine {
                        b.current.cancel();
                    }
                    let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                    if p.state == State::Running {
                        p.speed = 0.0;
                        p.state =
                            State::Failed("the video download stopped on an internal error".into());
                    }
                }
            });
        }
        Ok(VideoJob {
            child,
            progress,
            current,
        })
    }

    /// Its progress. Stopped only once the engine's download has saved where it stopped: a
    /// start again before then would find the stream still held.
    pub fn progress(&self) -> Progress {
        let mut p = self.progress.lock().unwrap().clone();
        if p.state == State::Stopped && self.current.busy() {
            p.state = State::Running;
            p.speed = 0.0;
        }
        p
    }

    /// Stop (the partial file stays; starting again continues it).
    pub fn stop(&self) {
        self.progress.lock().unwrap().state = State::Stopped;
        self.current.cancel();
        if let Some(s) = self.child.lock().unwrap().as_mut() {
            s.kill();
        }
    }

    /// A new speed limit (bytes/s): the engine's download takes it at once (#769). yt-dlp
    /// downloading by itself took its limit as it started.
    pub fn set_speed_limit(&self, rate: Option<u64>) {
        self.current.set_limit(rate);
    }

    pub fn speed_limit(&self) -> Option<u64> {
        self.current.limit()
    }

    /// Why yt-dlp downloads this one itself (#769: a live stream, RTMP…), as far as known: a new
    /// speed limit then takes starting it again.
    pub fn helper(&self) -> Option<crate::streams::Helper> {
        self.progress.lock().unwrap().helper.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A video download's files that "overwrite" (a duplicate added again) removes: exactly what
    /// yt-dlp leaves for its stem, the finished file and subtitles only with `outputs`; never a
    /// file merely named alike, another stem's, or MDM's own part files.
    #[test]
    fn a_videos_own_files_are_exactly_yt_dlps() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let working = [
            "Clip.mp4.part",
            "Clip.mp4.ytdl",
            "Clip.mp4.part-Frag4",
            "Clip.mp4.part-Frag4.part",
            "Clip.f137.mp4",
            "Clip.f137.mp4.part",
            "Clip.f137.mp4.ytdl",
            "Clip.f137.mp4.part-Frag12",
            "Clip.f137.mp4.part-Frag12.part",
            "Clip.f251.webm.part",
            "Clip.fhls-720p.mp4",
            "Clip.fdash-1.m4a.part",
            "Clip.fhttp-480p.mp4",
            "clip.temp.mp4",
            "Clip.f137.mp4.mdm.part",
            "Clip.f137.mp4.mdm.json",
            "Clip.f251.webm.mdm.part",
        ];
        let finished = [
            "Clip.mp4",
            "Clip.en.srt",
            "Clip.pt-BR.vtt",
            "Clip.fr.srt",
            "Clip.fi.vtt",
        ];
        let others = [
            "Clip.final.mp4",
            "Clip.full.mkv",
            "Clip.f1.jpg",
            "Clip.fhls-video.mp4",
            "Clip.v2.mp4.part",
            "Clip.pdf.mdm.part",
            "Clip.v2.zip.mdm.part",
            "Clip.mp4.mdm.part",
            "Clip.f137.mp4.mdm.zip",
            "Clip.f1.jpg.mdm.part",
            "Clip.zip",
            "Clip.notes.txt",
            "Clip.final cut.mp4",
            "Clip.mp4.part-Fragment",
            "Clip (1).f137.mp4",
            "Clip (1).f137.mp4.part",
            "Clip (1).en.srt",
            "Clipper.mp4.part",
            "Other.en.srt",
        ];
        for f in working.iter().chain(&finished).chain(&others) {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let names = |outputs| {
            let mut n: Vec<String> = own_files(d, "Clip", outputs)
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            n.sort();
            n
        };
        let mut want: Vec<String> = working.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(
            names(false),
            want,
            "subtitles (fr.srt, fi.vtt) are never working files"
        );
        want.extend(finished.iter().map(|s| s.to_string()));
        want.sort();
        assert_eq!(names(true), want);
        assert!(own_files(&d.join("missing"), "Clip", true).is_empty());
    }
    /// #736: the merge's own file, which yt-dlp renames when it's done, and its share done.
    #[test]
    fn a_merge_is_followed_by_its_file() {
        assert_eq!(
            merge_file(Path::new("/v/Big Buck Bunny.mp4")),
            PathBuf::from("/v/Big Buck Bunny.temp.mp4")
        );
        assert_eq!(
            merge_file(Path::new("/v/clip")),
            PathBuf::from("/v/clip.temp")
        );
        assert_eq!(merge_fraction(45, 100), 0.45);
        // About as big as the streams, never "done" before the rename.
        assert_eq!(merge_fraction(120, 100), 0.99);
        // A smaller file read later (truncated and written again): the share holds, then goes on.
        assert_eq!(merge_step(None, 0.4), 0.4);
        assert_eq!(merge_step(Some(0.4), 0.0), 0.4);
        assert_eq!(merge_step(Some(0.4), 0.45), 0.45);
        // A merge is a choice of several streams, as `MDMQ` names them.
        assert!(several_streams("401+140 3840x2160"));
        assert!(!several_streams("18 640x360"));
        assert!(!several_streams("140 audio only"));
    }

    /// #569: what a file joined from HLS pieces holds, from its first bytes. YouTube's sound:
    /// pieces each led by an ID3 tag (Apple's timestamp), then ADTS frames.
    #[test]
    fn joined_hls_pieces_are_named_for_what_they_hold() {
        let mut aac = b"ID3     ?".to_vec();
        aac.extend(std::iter::repeat_n(0u8, 0x3f));
        aac.extend([0xff, 0xf1, 0x50, 0x80, 0x2e, 0x7f, 0xfc]);
        assert_eq!(content_ext(&aac), Some("aac"));
        // ADTS with no tag, and MPEG-1 layer 3 (an MP3: not ours to rename).
        assert_eq!(content_ext(&[0xff, 0xf9, 0x50, 0x80, 0, 0, 0]), Some("aac"));
        assert_eq!(content_ext(&[0xff, 0xfb, 0x90, 0x64, 0, 0, 0]), None);
        let mut ts = vec![0u8; 400];
        for i in [0, 188, 376] {
            ts[i] = 0x47;
        }
        assert_eq!(content_ext(&ts), Some("ts"));
        // A real MP4 (ffmpeg was there): as it is.
        let mut mp4 = vec![0, 0, 0, 0x20];
        mp4.extend(b"ftypisom");
        mp4.resize(400, 0);
        assert_eq!(content_ext(&mp4), None);

        let dir = tempfile::tempdir().unwrap();
        let joined = dir.path().join("Talk.mp4");
        std::fs::write(&joined, &aac).unwrap();
        assert_eq!(named_for_content(joined), dir.path().join("Talk.aac"));
        assert!(dir.path().join("Talk.aac").is_file());
        // Another Talk.aac there already: not overwritten.
        std::fs::write(dir.path().join("Talk.mp4"), &aac).unwrap();
        let second = named_for_content(dir.path().join("Talk.mp4"));
        assert_eq!(second, dir.path().join("Talk (2).aac"));
        // A real MP4 keeps its name; so does a sound already named .aac.
        let real = dir.path().join("Real.mp4");
        std::fs::write(&real, &mp4).unwrap();
        assert_eq!(named_for_content(real.clone()), real);
        let named = dir.path().join("Talk.aac");
        assert_eq!(named_for_content(named.clone()), named);
    }

    #[test]
    fn a_limit_for_yt_dlp_downloading_by_itself() {
        let s = |v: Vec<String>| v.join(" ");
        assert_eq!(s(limit_args(None)), "");
        assert_eq!(s(limit_args(Some(524_288))), "--limit-rate 524288");
    }

    /// Live (network, yt-dlp): `MDM_LIVE_YTDLP=<yt-dlp.exe> MDM_LIVE_PAGE=<video page>
    /// cargo test -p mdm-tools live_download -- --ignored --nocapture`. Downloads the smallest
    /// audio and prints the log (a swapped server shows there).
    #[test]
    #[ignore]
    fn live_download() {
        let exe = PathBuf::from(std::env::var("MDM_LIVE_YTDLP").expect("MDM_LIVE_YTDLP"));
        let page = std::env::var("MDM_LIVE_PAGE").expect("MDM_LIVE_PAGE");
        let dir = tempfile::tempdir().unwrap();
        let format = std::env::var("MDM_LIVE_FORMAT").unwrap_or_else(|_| "worstaudio".into());
        let ffmpeg = std::env::var("MDM_LIVE_FFMPEG").ok().map(PathBuf::from);
        // MDM's engine downloads the streams (#769), unless MDM_LIVE_YTDLP_ALONE is set.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let engine =
            std::env::var_os("MDM_LIVE_YTDLP_ALONE")
                .is_none()
                .then(|| crate::streams::Engine {
                    handle: rt.handle().clone(),
                    opts: mdm_engine::Options::default(),
                });
        let job = VideoJob::start(
            &exe,
            &page,
            &format,
            dir.path(),
            "a",
            None,
            ffmpeg.as_deref(),
            None,
            &Default::default(),
            &Default::default(),
            engine,
        )
        .unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(180);
        // When the bytes started and stopped coming: the transfer's own speed.
        let (mut first, mut last, mut most) = (None, None, 0u64);
        let p = loop {
            let p = job.progress();
            if p.downloaded > most {
                most = p.downloaded;
                first.get_or_insert_with(std::time::Instant::now);
                last = Some(std::time::Instant::now());
            }
            if p.state != State::Running || std::time::Instant::now() > end {
                break p;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        };
        for (_, e) in &p.events {
            println!("event: {e}");
        }
        println!("state: {:?}, {} bytes", p.state, p.downloaded);
        if let (Some(a), Some(b)) = (first, last) {
            let secs = (b - a).as_secs_f64().max(0.001);
            println!(
                "transfer: {most} bytes in {secs:.1} s, {:.2} MB/s",
                most as f64 / secs / 1e6
            );
        }
        assert!(
            matches!(p.state, State::Done(ref f) if f.exists()),
            "{:?}",
            p.state
        );
    }

    /// Live (#61): `MDM_LIVE_YTDLP=<yt-dlp.exe> MDM_LIVE_SLOW=<a slow file's address>
    /// cargo test -p mdm-tools live_stop -- --ignored --nocapture` (the lab server:
    /// `/gen/20M/clip.mp4?rate=50K`). Stopping a video download ends every yt-dlp process it
    /// started (Windows: the launcher and its child), and the part file stops growing.
    #[test]
    #[ignore]
    fn live_stop_ends_every_process() {
        let exe = PathBuf::from(std::env::var("MDM_LIVE_YTDLP").expect("MDM_LIVE_YTDLP"));
        let url = std::env::var("MDM_LIVE_SLOW").expect("MDM_LIVE_SLOW");
        let dir = tempfile::tempdir().unwrap();
        let job = VideoJob::start(
            &exe,
            &url,
            "b",
            dir.path(),
            "clip",
            None,
            None,
            None,
            &Default::default(),
            &Default::default(),
            None,
        )
        .unwrap();
        let size = || {
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .map(|e| e.metadata().map_or(0, |m| m.len()))
                .sum::<u64>()
        };
        std::thread::sleep(std::time::Duration::from_secs(8));
        println!("before stop: {} bytes, {:?}", size(), job.progress().state);
        job.stop();
        std::thread::sleep(std::time::Duration::from_secs(3));
        let stopped = size();
        std::thread::sleep(std::time::Duration::from_secs(4));
        println!("after stop: {stopped} then {} bytes", size());
        assert_eq!(size(), stopped, "nothing downloads after Stop");
        // Started again: it continues the same part file (nothing else holds it now).
        let again = VideoJob::start(
            &exe,
            &url,
            "b",
            dir.path(),
            "clip",
            None,
            None,
            None,
            &Default::default(),
            &Default::default(),
            None,
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_secs(8));
        // yt-dlp's own count (a file open in another process shows a stale size on Windows).
        let p = again.progress();
        println!("resumed: yt-dlp at {} bytes, {:?}", p.downloaded, p.state);
        assert!(p.downloaded > stopped, "it went on from where it stopped");
        assert!(
            !matches!(p.state, State::Failed(_)),
            "resumed without an error: {:?}",
            p.state
        );
        again.stop();
    }

    fn run(expected: Option<u64>, lines: &[&str]) -> Vec<(u64, Option<u64>)> {
        let mut m = Meter::new(expected);
        let mut p = Progress {
            state: State::Running,
            downloaded: 0,
            total: None,
            speed: 0.0,
            events: Vec::new(),
            side_files: Vec::new(),
            sub_problems: Vec::new(),
            engine: None,
            helper: None,
            merging: None,
            merge_failed: None,
        };
        // `MDMQ …` as yt-dlp prints it before it downloads; every other line is an `MDMP` one's.
        lines
            .iter()
            .filter_map(|l| match l.strip_prefix("MDMQ ") {
                Some(q) => {
                    m.quality(q);
                    None
                }
                None => {
                    m.line(l, &mut p);
                    Some((p.downloaded, p.total))
                }
            })
            .collect()
    }

    /// The bar as the progress window draws it: held short of the end while yt-dlp works
    /// (`Item::fraction` takes [`MAX_WHILE_RUNNING`]; the total itself is the real size).
    fn fractions(v: &[(u64, Option<u64>)]) -> Vec<f64> {
        v.iter()
            .map(|(d, t)| (*d as f64 / t.unwrap() as f64).min(MAX_WHILE_RUNNING))
            .collect()
    }

    /// A fragmented 1080p stream whose estimate grows, then the separate audio from 0: the bar
    /// only moves forward, and ends at the whole.
    #[test]
    fn video_then_audio_never_moves_back() {
        let lines = [
            "100 NA 1000 5",
            "300 NA 1300 5",
            "600 NA 1500 5",
            "1400 NA 1600 5",
            "1600 1600 NA 5",
            "50 200 NA 5",
            "150 200 NA 5",
            "200 200 NA 5",
        ];
        let f = fractions(&run(Some(1800), &lines));
        for w in f.windows(2) {
            assert!(w[1] >= w[0] - 1e-9, "went back: {f:?}");
        }
        let last = run(Some(1800), &lines).pop().unwrap();
        assert_eq!(last, (1800, Some(1800)));
        assert!(f[f.len() - 1] > 0.99, "{f:?}");
    }

    /// The file size stays the file's size at the end: holding the bar short of 100 % is the
    /// window's (`Item::fraction`), not a total made 0.1 % too big (100.100 MB for 100 MB).
    #[test]
    fn the_size_is_not_inflated_at_the_end() {
        let m = 1 << 20;
        let line = format!("{} {} NA 900000", 100 * m, 100 * m);
        let v = run(Some(100 * m), &[line.as_str()]);
        assert_eq!(v, vec![(100 * m, Some(100 * m))]);
    }

    /// #344: with no size known from the start, the second stream brings the bar back from
    /// where the first one's end had taken it, and it never says 100 % before the end.
    #[test]
    fn a_stream_ending_is_not_the_whole_download() {
        let lines = [
            "100 1000 NA 5",
            "1000 1000 NA 5",
            "50 500 NA 5",
            "400 500 NA 5",
        ];
        let f = fractions(&run(None, &lines));
        assert!(f[2] < 0.75, "the first stream's 100 % stayed: {f:?}");
        assert!(f[3] > f[2], "{f:?}");
    }

    /// #344: no total and no estimate for a stream, nothing said by the video window: no
    /// percentage at all, and certainly not 100 %.
    #[test]
    fn nothing_known_is_no_percentage() {
        let v = run(None, &["100 NA NA 5", "9000 NA NA 5"]);
        assert_eq!(v, vec![(100, None), (9000, None)]);
    }

    /// The size the window showed counts both streams from the start: the first stream alone
    /// never looks like the whole download, whether or not yt-dlp has named the streams.
    #[test]
    fn expected_size_holds_the_bar_back() {
        let v = run(Some(1000), &["400 400 NA 0"]);
        assert_eq!(v[0], (400, Some(1000)));
        let v = run(Some(1000), &["MDMQ 137+140 1920x1080", "400 400 NA 0 137"]);
        assert_eq!(v[0], (400, Some(1000)));
    }

    /// A line with no figure at all: no percentage.
    #[test]
    fn a_line_of_nothing_is_no_percentage() {
        let v = run(None, &["NA NA NA NA"]);
        assert_eq!(v[0], (0, None));
    }

    /// Never back, but for the total's rounding to a whole byte (these sizes are tiny).
    fn assert_forward(f: &[f64]) {
        for w in f.windows(2) {
            assert!(w[1] >= w[0] - 1e-3, "went back: {f:?}");
        }
    }

    /// The bar at its end (`v`, the meter's lines). Drawn, it is held short of the end
    /// ([`fractions`]); the meter's own figures reach the whole at the last line only, and never
    /// go past it: 100 % is never said before the end.
    fn assert_ends(v: &[(u64, Option<u64>)]) {
        let f = fractions(v);
        assert!(f[f.len() - 1] >= MAX_WHILE_RUNNING - 1e-9, "{f:?}");
        let raw: Vec<f64> = v
            .iter()
            .map(|(d, t)| *d as f64 / t.unwrap() as f64)
            .collect();
        assert!(raw.iter().all(|x| *x <= 1.0), "past 100 %: {raw:?}");
        assert!(
            raw[..raw.len() - 1].iter().all(|x| *x < 1.0),
            "100 % before the end: {raw:?}"
        );
    }

    /// An HLS stream's lines, as yt-dlp prints them for format `id` (none: lines without one,
    /// as before it was asked for): from nothing, then its estimate after each of `frags`
    /// fragments of `frag` bytes, then its own total at its end.
    fn hls_stream(id: &str, frags: u64, frag: u64) -> Vec<String> {
        let whole = frags * frag;
        let mut out = vec!["0 NA NA 0".to_owned()];
        out.extend((1..frags).map(|k| format!("{} NA {whole} 5", k * frag)));
        out.push(format!("{whole} {whole} NA 5"));
        out.into_iter()
            .map(|l| format!("{l} {id}").trim_end().to_owned())
            .collect()
    }

    fn lines<'a>(quality: Option<&'a str>, streams: &'a [Vec<String>]) -> Vec<&'a str> {
        quality
            .into_iter()
            .chain(streams.iter().flatten().map(String::as_str))
            .collect()
    }

    /// The figures as before yt-dlp named its streams: the same lines without `MDMQ` and without
    /// their format ids.
    fn as_before(expected: Option<u64>, streams: &[Vec<String>]) -> Vec<(u64, Option<u64>)> {
        let bare: Vec<Vec<String>> = streams
            .iter()
            .map(|s| {
                s.iter()
                    .map(|l| l.splitn(5, ' ').take(4).collect::<Vec<_>>().join(" "))
                    .collect()
            })
            .collect();
        run(expected, &lines(None, &bare))
    }

    /// The video window's size an estimate twice what comes (an HLS stream's BANDWIDTH is its
    /// peak): with yt-dlp's word that the choice is one stream, the bar ends at the end, not
    /// halfway; never back. Without that word, as before: the window's size holds it.
    #[test]
    fn an_estimate_above_the_size_still_ends_at_the_end() {
        let stream = [hls_stream("hls-6400", 10, 100)];
        let v = run(Some(2000), &lines(Some("MDMQ hls-6400 1280x720"), &stream));
        assert_forward(&fractions(&v));
        assert_ends(&v);
        let f = fractions(&as_before(Some(2000), &stream));
        assert!((f[f.len() - 1] - 0.5).abs() < 1e-9, "{f:?}");
    }

    /// The same for a merged choice (video, then audio): the bar ends at the end, and the
    /// video's end is still not the whole download, nor does the bar go back (#344).
    #[test]
    fn a_merged_estimate_above_the_size_still_ends_at_the_end() {
        let streams = [
            hls_stream("hls-7040", 16, 100),
            hls_stream("hls-aud-English", 4, 100),
        ];
        let v = run(
            Some(4000),
            &lines(Some("MDMQ hls-7040+hls-aud-English 1280x720"), &streams),
        );
        let f = fractions(&v);
        assert_forward(&f);
        let video_end = streams[0].len() - 1;
        assert!(f[video_end] < 0.75, "the video's end looked whole: {f:?}");
        assert_ends(&v);
        assert_eq!(v[v.len() - 1].0, 2000);
    }

    /// One line for all the streams at once (`137+140`: yt-dlp's ffmpeg downloader fetches and
    /// merges them together, and says so once, at the end): that is the whole download, and its
    /// own total its end.
    #[test]
    fn a_line_for_every_stream_at_once_is_the_whole_download() {
        let q = "MDMQ 7040+aud-English 1280x720";
        let all = [hls_stream("7040+aud-English", 20, 100)];
        let v = run(Some(4000), &lines(Some(q), &all));
        assert_forward(&fractions(&v));
        assert_ends(&v);
        let v = run(Some(4000), &[q, "2000 2000 NA 2129866.9 7040+aud-English"]);
        assert_ends(&v);
    }

    /// Lines whose format id `MDMQ` didn't name, or `NA` on every line (a yt-dlp that names no
    /// stream): read exactly as before, never left out (the bar never stands still at 0).
    #[test]
    fn lines_not_named_by_mdmq_are_read_as_before() {
        for (expected, ids) in [
            (Some(4000), ["999", "998"]),
            (Some(4000), ["NA", "NA"]),
            (Some(2000), ["NA", "NA"]),
            (None, ["NA", "NA"]),
            (None, ["999", "998"]),
        ] {
            let streams = [hls_stream(ids[0], 16, 100), hls_stream(ids[1], 4, 100)];
            let v = run(expected, &lines(Some("MDMQ 137+140 1920x1080"), &streams));
            assert_eq!(v, as_before(expected, &streams), "{expected:?} {ids:?}");
            assert_eq!(v[v.len() - 1].0, 2000);
            if expected != Some(4000) {
                assert_ends(&v[v.len() - 1..]);
            }
        }
    }

    /// Subtitles asked for (#68): yt-dlp downloads their file first, with lines of its own that
    /// name no format, `NA` (as yt-dlp 2026.10.26 prints them for an HLS subtitle rendition), each
    /// file counting from 0 again. They are no stream: the video is not taken for the last one,
    /// its end is still not the whole download (#344), and their bytes leave the bar when the
    /// video's first line comes.
    #[test]
    fn subtitles_before_the_video_are_no_stream() {
        let streams = [
            hls_stream("NA", 3, 10),
            hls_stream("NA", 3, 10),
            hls_stream("137", 16, 100),
            hls_stream("140", 4, 100),
        ];
        let v = run(Some(4000), &lines(Some("MDMQ 137+140 1920x1080"), &streams));
        let subs = streams[0].len() + streams[1].len();
        assert!(fractions(&v[..subs]).iter().all(|x| *x < 0.02), "{v:?}");
        assert_eq!(v[subs - 1].0, 60);
        assert_eq!(v[subs].0, 0);
        let f = fractions(&v[subs..]);
        assert_forward(&f);
        let video_end = streams[2].len() - 1;
        assert!(f[video_end] < 0.75, "the video's end looked whole: {f:?}");
        assert_ends(&v[subs..]);
        assert_eq!(v[v.len() - 1].0, 2000);
    }

    /// A stream's count from 0 again (a retry from the start) is the same stream, not the next
    /// one: the video is not taken for the last stream, its bytes count once, and the bar waits
    /// where it was until the retry is back there.
    #[test]
    fn a_retry_from_the_start_is_not_the_next_stream() {
        let mut video = hls_stream("137", 16, 100);
        video.splice(
            9..9,
            ["300 NA 1600 5 137".to_owned(), "0 NA NA 0 137".to_owned()],
        );
        let streams = [video, hls_stream("140", 4, 100)];
        let v = run(Some(4000), &lines(Some("MDMQ 137+140 1920x1080"), &streams));
        assert_eq!([v[8].0, v[9].0, v[10].0, v[11].0], [800, 800, 800, 900]);
        let f = fractions(&v);
        assert_forward(&f);
        let video_end = streams[0].len() - 1;
        assert!(f[video_end] < 0.75, "the video's end looked whole: {f:?}");
        assert_ends(&v);
        assert_eq!(v[video_end].0, 1600);
        assert_eq!(v[v.len() - 1].0, 2000);
    }

    /// A variable bitrate's estimates: low at first, then up and down around the size as the
    /// fragments come. The bar never goes back and ends at the end.
    #[test]
    fn noisy_estimates_never_pull_the_bar_back() {
        let mut l = vec![
            "MDMQ hls-1 1280x720".to_owned(),
            "0 NA NA 0 hls-1".to_owned(),
        ];
        for (got, estimate) in [
            (100, 400),
            (200, 900),
            (300, 1300),
            (400, 1100),
            (500, 1000),
            (600, 1200),
            (700, 1000),
            (800, 1050),
            (900, 1000),
        ] {
            l.push(format!("{got} NA {estimate} 5 hls-1"));
        }
        l.push("1000 1000 NA 5 hls-1".to_owned());
        let l: Vec<&str> = l.iter().map(String::as_str).collect();
        let v = run(Some(2000), &l);
        assert_forward(&fractions(&v));
        assert_ends(&v);
    }

    /// The video window's size below what comes: the same figures as before yt-dlp named its
    /// streams (a stream's end never pulls the bar back).
    #[test]
    fn an_estimate_below_the_size_is_as_before() {
        for (expected, streams, quality) in [
            (1000, vec![hls_stream("22", 15, 100)], "MDMQ 22 1280x720"),
            (
                1200,
                vec![hls_stream("137", 16, 100), hls_stream("140", 4, 100)],
                "MDMQ 137+140 1920x1080",
            ),
            (
                1800,
                vec![hls_stream("137", 16, 100), hls_stream("140", 4, 100)],
                "MDMQ 137+140 1920x1080",
            ),
        ] {
            let with = run(Some(expected), &lines(Some(quality), &streams));
            assert_eq!(with, as_before(Some(expected), &streams));
            assert_forward(&fractions(&with));
        }
    }

    /// The streams `MDMQ` names, and which format ids are theirs: the resolution after the last
    /// one (two words for a sound) is not part of its id; the merged id is all of them at once;
    /// nothing is named before yt-dlp has said, nor `NA`.
    #[test]
    fn the_streams_are_named_by_their_format_ids() {
        let mut m = Meter::new(None);
        assert_eq!(m.named(Some("137")), None);
        let cases: [(&str, &[&str], &[&str]); 4] = [
            (
                "137+140 1920x1080",
                &["137", "140"],
                &["13", "14", "251", "NA"],
            ),
            ("18 640x360", &["18"], &["1", "640x360"]),
            (
                "hls-720p+hls-audio-English 1280x720",
                &["hls-720p", "hls-audio-English"],
                &["hls-audio", "hls-720p+hls-audio"],
            ),
            (
                "aud-English (US) audio only",
                &["aud-English (US)"],
                &["aud"],
            ),
        ];
        for (q, ids, others) in cases {
            m.quality(q);
            assert_eq!(m.streams(), Some(ids.len()), "{q}");
            for id in ids {
                assert_eq!(m.named(Some(*id)), Some((*id, false)), "{q}");
            }
            for id in others {
                assert_eq!(m.named(Some(*id)), None, "{q}: {id}");
            }
        }
        m.quality("137+140 1920x1080");
        assert_eq!(m.named(Some("137+140")), Some(("137+140", true)));
        assert_eq!(m.named(None), None);
        m.quality("");
        assert_eq!(m.streams(), None);
    }

    /// The subtitle arguments (#68): none asked, none passed; beside the video as SRT with
    /// ffmpeg, the site's own format without; embedded only with ffmpeg; every uploader's
    /// language but a live stream's chat.
    #[test]
    fn subtitle_arguments() {
        let subs = |langs: &[&str], automatic: bool, embed: bool| Subs {
            langs: langs.iter().map(|l| l.to_string()).collect(),
            automatic,
            embed,
        };
        let s = |v: Vec<String>| v.join(" ");
        assert!(subtitle_args(&Subs::default(), true).is_empty());
        assert_eq!(
            s(subtitle_args(&subs(&["fr"], false, false), true)),
            "--write-subs --sub-langs fr --sub-format srt/best --convert-subs srt --ignore-errors"
        );
        assert_eq!(
            s(subtitle_args(&subs(&["fr"], false, false), false)),
            "--write-subs --sub-langs fr --sub-format srt/vtt/best --ignore-errors"
        );
        assert_eq!(
            s(subtitle_args(&subs(&["en-orig"], true, true), true)),
            "--write-auto-subs --sub-langs en-orig --sub-format srt/best --convert-subs srt --embed-subs --ignore-errors"
        );
        assert_eq!(
            s(subtitle_args(&subs(&[ALL_UPLOADERS], false, true), true)),
            "--sub-langs all,-live_chat --sub-format srt/best --convert-subs srt --embed-subs --ignore-errors"
        );
        // Embedding needs ffmpeg: without it they're saved beside the video.
        assert_eq!(
            s(subtitle_args(&subs(&["de", "fr"], false, true), false)),
            "--write-subs --sub-langs de,fr --sub-format srt/vtt/best --ignore-errors"
        );
    }

    /// Embed chapters (#827): `--embed-chapters` for a video or the sound as the site has it,
    /// with ffmpeg; nothing when the option is off, without ffmpeg (which writes them), or for a
    /// conversion to MP3, Opus or FLAC.
    #[test]
    fn chapter_arguments() {
        assert_eq!(chapter_args(true, true, false), ["--embed-chapters"]);
        assert!(chapter_args(false, true, false).is_empty());
        assert!(chapter_args(true, false, false).is_empty());
        assert!(chapter_args(true, true, true).is_empty());
    }

    /// yt-dlp's warnings for subtitles it couldn't get (#68): each language once, a site's
    /// "too many requests" told apart from other reasons, other warnings ignored.
    #[test]
    fn refused_subtitles_from_the_warnings() {
        let w: Vec<String> = [
            "Unable to download video subtitles for 'en': HTTP Error 429: Too Many Requests",
            "[youtube] abc: Some formats are missing",
            "Unable to download video subtitles for 'de': HTTP Error 404: Not Found",
            "Unable to download video subtitles for 'en': HTTP Error 429: Too Many Requests",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            subtitle_failures(&w),
            [
                ("en".to_string(), SubReason::TooManyRequests),
                (
                    "de".to_string(),
                    SubReason::Other("HTTP Error 404: Not Found".into())
                ),
            ]
        );
        assert!(subtitle_failures(&[]).is_empty());
        // The retry fetches only those, beside the video, never into it.
        let r = SubsRetry {
            automatic: true,
            embed: true,
            ffmpeg: true,
            base: vec!["-o".into(), "x.%(ext)s".into()],
        };
        assert_eq!(
            r.args(&["en".into()]).join(" "),
            "-o x.%(ext)s --skip-download --write-auto-subs --sub-langs en --sub-format srt/best --convert-subs srt --ignore-errors"
        );
        let info = serde_json::json!({"automatic_captions": {"en": [{"name": "English"}]}});
        assert_eq!(sub_name(&info, "en"), "English");
        assert_eq!(sub_name(&info, "ru"), "ru");
    }

    /// The files found beside a finished video are its own subtitles only.
    #[test]
    fn side_files_are_the_videos_subtitles() {
        let dir = tempfile::tempdir().unwrap();
        for f in [
            "Talk.mp4",
            "Talk.fr.srt",
            "Talk.en-orig.vtt",
            "Talk.part",
            "Talk 2.en.srt",
            "Other.fr.srt",
            "Talk.jpg",
        ] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        let names: Vec<String> = side_files(&dir.path().join("Talk.mp4"))
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Talk.en-orig.vtt", "Talk.fr.srt"]);
    }

    /// #94: a window's choice falls back to the nearest quality below it, then to the best;
    /// choices without a height (audio, "b") have no fallback; the selector is swapped in place.
    #[test]
    fn a_missing_quality_falls_back_to_the_nearest_below() {
        assert_eq!(
            relaxed("bv*[height=1080]+ba/b[height=1080]").as_deref(),
            Some("bv*[height<=1080]+ba/b[height<=1080]/bv*+ba/bv*")
        );
        assert_eq!(relaxed("ba/b"), None);
        assert_eq!(relaxed("b"), None);
        let ours: Vec<String> = ["-f", "bv*[height=720]+ba/b[height=720]", "-o", "x"]
            .map(String::from)
            .to_vec();
        assert_eq!(selector_of(&ours), Some("bv*[height=720]+ba/b[height=720]"));
        assert_eq!(
            with_selector(&ours, "b"),
            ["-f", "b", "-o", "x"].map(String::from)
        );
    }

    /// #79: the saved file found by our own output name: the media file, not its subtitles,
    /// its unfinished part or another video's file; `%` in a title as it was written.
    #[test]
    fn the_saved_file_is_found_by_its_name() {
        let dir = tempfile::tempdir().unwrap();
        for f in [
            "Şirin 100% ğüş.mp4",
            "Şirin 100% ğüş.ru.srt",
            "Şirin 100% ğüş.f137.mp4.part",
            "Other.mp4",
        ] {
            std::fs::write(dir.path().join(f), b"x").unwrap();
        }
        let template = dir
            .path()
            .join(format!("{}.%(ext)s", template_text("Şirin 100% ğüş")));
        let ours: Vec<String> = vec!["-o".into(), template.to_string_lossy().into_owned()];
        assert_eq!(
            saved_by_name(&ours)
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "Şirin 100% ğüş.mp4"
        );
        let none: Vec<String> = vec![
            "-o".into(),
            dir.path()
                .join("Missing.%(ext)s")
                .to_string_lossy()
                .into_owned(),
        ];
        assert_eq!(saved_by_name(&none), None);
    }

    /// A2: the quality menu's look-up is taken for the very choice it offered only (its selector
    /// under the same label, merged or not as the menu had it), and waited for while it's being
    /// made, never past its time or a Stop. Too big to keep: none. Nothing of it in `Debug`.
    #[test]
    fn the_menus_look_up_serves_its_own_choice_only() {
        const PAGE: &str = r#"{"title": "T", "duration": 10, "formats": [
            {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a", "tbr": 500}]}"#;
        let page = PAGE.as_bytes();
        let label = "360p · H.264";
        assert!(offered(page, "18/b[height=360]", label, false).is_some());
        assert!(offered(page, "18/b[height=720]", "720p · H.264", false).is_none());
        assert!(offered(page, "18/b[height=360]", "360p · WEBM · H.264", false).is_none());
        assert!(offered(page, "18/b[height=360]", label, true).is_none());
        assert!(offered(page, "18/bv*[height=360]+ba/b[height=360]", label, true).is_some());
        assert!(offered(b"not json", "b", "b", false).is_none());

        let slot = InfoSlot::new(Instant::now());
        let filler = slot.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            filler.fill(page.to_vec(), true);
        });
        let (json, without) = slot
            .wait(Instant::now() + Duration::from_secs(10), || false)
            .unwrap();
        assert_eq!((json.as_slice(), without), (page, true));
        slot.fail();
        assert!(slot.wait(Instant::now(), || false).is_some(), "kept");
        assert_eq!(format!("{slot:?}"), "InfoSlot");

        let pending = InfoSlot::new(Instant::now());
        let at = Instant::now();
        assert!(
            pending
                .wait(at + Duration::from_millis(200), || false)
                .is_none()
        );
        assert!(at.elapsed() >= Duration::from_millis(200));
        assert!(
            pending
                .wait(at + Duration::from_secs(60), || true)
                .is_none(),
            "stopped"
        );
        assert!(pending.pending());
        pending.fail();
        assert!(!pending.pending());

        let big = InfoSlot::new(Instant::now());
        big.fill(vec![b' '; INFO_MAX + 1], false);
        assert!(!big.pending());
        assert!(
            big.wait(Instant::now() + Duration::from_secs(1), || false)
                .is_none()
        );
    }

    /// All the look-ups kept together stay within their budget: the oldest answers are let go
    /// (their downloads look the page up afresh); one no longer kept anywhere counts no more.
    #[test]
    fn look_ups_kept_have_a_budget_together() {
        let pool = Kept::new(100);
        let now = Instant::now();
        let kept = |n: usize| {
            let slot = InfoSlot::counted_in(now, pool.clone());
            slot.fill(vec![b' '; n], false);
            slot
        };
        let has = |s: &InfoSlot| s.wait(Instant::now(), || false).is_some();
        let (a, b) = (kept(40), kept(40));
        assert!(has(&a) && has(&b));
        let c = kept(40);
        assert!(!has(&a), "the oldest let go");
        assert!(has(&b) && has(&c));
        assert!(!a.pending(), "not waited for either");
        // Dropped: its 40 bytes are free.
        drop(b);
        let d = kept(40);
        assert!(has(&c) && has(&d));
        // One bigger than the rest together: the others go, it stays.
        let e = kept(90);
        assert!(!has(&c) && !has(&d) && has(&e));
        // The process's own pool: INFO_TOTAL.
        assert_eq!(Kept::shared().limit, INFO_TOTAL);
    }

    #[test]
    fn percent_signs_in_titles_stay_text() {
        assert_eq!(template_text("100% fun"), "100%% fun");
    }

    /// #622: a Dropbox video (an HLS stream whose size nobody knows beforehand) read 99.9 % from
    /// its first second to its end, the size growing with what came and 0 seconds left. yt-dlp's
    /// first line for a stream gives an estimate as big as what came (1 KB of 1 KB: it knows
    /// nothing yet), and the bar held on to that 99.9 % ever after. These are the lines yt-dlp
    /// printed for a Dropbox video, its 1080p stream then its sound.
    #[test]
    fn a_first_estimate_as_big_as_what_came_is_no_end() {
        let lines = [
            "MDMQ 1080p+aud192k-Track_1 1920x1080",
            "1024 NA 1024 491.2 1080p",
            "3072 NA 20480.0 491.2 1080p",
            "7168 NA 61440.0 491.2 1080p",
            "1047552 NA 10465280.0 452673.0 1080p",
            "1206648 NA 12046000.0 502170.4 1080p",
            "9775640 NA 37005408.0 1553343.6 1080p",
            "22221428 NA 39449345.4 1594996.9 1080p",
            "36687072 NA 42130818.8 1439490.0 1080p",
            "39940760 NA 39940760 1343764.3 1080p",
            "64512 NA 253952.0 19016.1 aud192k-Track_1",
            "1122012 NA 2178488.0 258367.6 aud192k-Track_1",
            "2185904 NA 2460726.8 369230.2 aud192k-Track_1",
        ];
        let got = run(None, &lines);
        // Nothing known at the first line: no percentage, rather than the end.
        assert_eq!(got[0], (1024, None));
        let f: Vec<f64> = got[1..]
            .iter()
            .map(|(d, t)| (*d as f64 / t.unwrap() as f64).min(MAX_WHILE_RUNNING))
            .collect();
        // A sliver at first, then what the estimate says: about a quarter at 9.8 MB of ~37 MB.
        assert!(f[0] < 0.2, "{f:?}");
        assert!(f[4] > 0.2 && f[4] < 0.3, "{f:?}");
        assert!(f[6] > 0.85 && f[6] < 0.9, "{f:?}");
        // The time left counts down from a size that is not just what came.
        assert!(got[5].1.unwrap() > got[5].0 * 3, "{got:?}");
    }
}
