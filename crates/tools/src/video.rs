//! A video download run by yt-dlp under supervision: our file name, progress read from a fixed
//! `--progress-template`, the final path from `--print after_move`. Stopping kills it; starting
//! again continues (`--continue`). Each start looks the page up afresh (`-J`), points links at
//! servers that answer ([`crate::servers`]) and downloads from that list (`--load-info-json`),
//! so the page is read once.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::proc::{self, ProcError};
use crate::relay::Pace;
use crate::servers;

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Running,
    Done(PathBuf),
    Failed(String),
    Stopped,
}

#[derive(Clone, Debug, PartialEq)]
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

pub struct VideoJob {
    /// yt-dlp once the page is looked up (`None` before).
    child: Arc<Mutex<Option<proc::Spawned>>>,
    progress: Arc<Mutex<Progress>>,
    /// Its speed limits, followed as they change (#708; `crate::relay`).
    pace: Option<Arc<Pace>>,
}

/// A private file for the looked-up page (its links are signed; removed when the run ends).
fn info_file() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
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
    pace: Option<&Arc<Pace>>,
) {
    let mut ours = ours.to_vec();
    let mut dead: std::collections::BTreeSet<String> = Default::default();
    let mut looked_again = false;
    let mut relaxed_to: Option<String> = None;
    let mut reroute = true;
    let mut last = String::from("yt-dlp stopped");
    for _ in 0..5 {
        match attempt(
            exe, url, &ours, browser, progress, child, expected, retry, &dead, reroute, pace,
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

/// yt-dlp's progress lines (`MDMP <downloaded> <total> <estimate> <speed>`) made into one
/// steady figure. Each stream of a merged download (video, then audio) reports from 0 with its
/// own total, and a fragmented stream's total (HLS, DASH) is an estimate that moves with every
/// fragment: the streams are added up, the total never falls below the size the video window
/// showed (`expected`, every stream), and the fraction done goes back only when a stream's end
/// had looked like the whole (no `expected`, the next stream starts).
///
/// Pieces fetched several at once ([`fragment_args`]) report from each at its turn: the count can
/// step back a little, which isn't a new stream (#531: one taken for it added a 25 MB estimate,
/// and a 6 MB video read 66 MB). Then only a fall to under half of it, by 64 KB or more, is.
struct Meter {
    expected: Option<u64>,
    /// Pieces at once: the count may step back.
    at_once: bool,
    /// The streams already finished, and the current one's last total and bytes.
    finished: u64,
    last_total: u64,
    last_got: u64,
    /// The highest fraction shown so far.
    frac: f64,
}

impl Meter {
    fn new(expected: Option<u64>, at_once: bool) -> Self {
        Meter {
            expected: expected.filter(|e| *e > 0),
            at_once,
            finished: 0,
            last_total: 0,
            last_got: 0,
            frac: 0.0,
        }
    }

    fn line(&mut self, rest: &str, p: &mut Progress) {
        let f: Vec<Option<f64>> = rest.split_whitespace().map(|v| v.parse().ok()).collect();
        let got = f.first().copied().flatten().unwrap_or(0.0).max(0.0) as u64;
        let total = f
            .get(1)
            .copied()
            .flatten()
            .or(f.get(2).copied().flatten())
            .map(|t| t.max(0.0) as u64);
        // Back near 0: the next stream started; the one before counts in full.
        let next_stream = if self.at_once {
            got.saturating_mul(2) < self.last_got && self.last_got - got >= 64 * 1024
        } else {
            got < self.last_got
        };
        if next_stream && self.last_got > 0 {
            self.finished += self.last_total.max(self.last_got);
            self.last_total = 0;
            // Without the whole size from the start, the first stream's end looked like the
            // whole download: the next stream's total pulls the bar back (#344).
            if self.expected.is_none() {
                self.frac = 0.0;
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
        let mut whole = self.finished + self.last_total.max(got);
        if let Some(e) = self.expected {
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
    pace: Option<&Arc<Pace>>,
) -> Result<Option<Ran>, String> {
    let stdout = {
        // Under the child's lock: stop() either came first (nothing starts) or finds the child
        // to kill.
        let mut slot = child.lock().unwrap();
        if progress.lock().unwrap().state == State::Stopped {
            return Ok(None);
        }
        let spawned = proc::ytdlp_info_paced(exe, &[], ours, file, pace)
            .and_then(|cmd| proc::spawn(cmd, "yt-dlp"));
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
    // Every line to the end, read as bytes (#79): a line that isn't UTF-8 (a title in a legacy
    // code page) used to end the reading there, while yt-dlp went on; it is read leniently now,
    // and the saved file is also found by its name (`attempt`).
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
        } else if let Some(rest) = line.strip_prefix("MDMQ ") {
            quality = Some(rest.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("MDMP ")
            && let Some(m) = meter.as_deref_mut()
        {
            m.line(rest, &mut progress.lock().unwrap());
        }
    }
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
    }))
}

/// The media file yt-dlp saved under our output name (`<dir>/<stem>.%(ext)s`), when it didn't say
/// where, or said it in a form that names no file (#79). Its subtitles (`<stem>.<lang>.srt`) and
/// unfinished parts don't count; the newest one wins.
fn saved_by_name(ours: &[String]) -> Option<PathBuf> {
    const MEDIA: [&str; 15] = [
        "mp4", "mkv", "webm", "mov", "m4v", "m4a", "mp3", "opus", "ogg", "oga", "flac", "wav",
        "aac", "flv", "3gp",
    ];
    let template = ours
        .iter()
        .position(|a| a == "-o")
        .and_then(|i| ours.get(i + 1))?;
    let t = Path::new(template);
    let dir = t.parent()?;
    let stem = t
        .file_name()?
        .to_string_lossy()
        .strip_suffix(".%(ext)s")?
        .replace("%%", "%");
    MEDIA
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .filter(|p| p.is_file())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
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

/// One run: look the page up, move links off unreachable servers (and off `dead`, the ones an
/// earlier run couldn't reach), download from that list. Subtitles the site refused are asked
/// for once more after a pause (`retry`), beside the video; the ones still refused are reported
/// with it, never failing it.
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
    pace: Option<&Arc<Pace>>,
) -> Attempt {
    let failed = |error: String, rerouted: bool| Attempt::Failed { error, rerouted };
    let mut info = match look_up(exe, url, browser) {
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
    let changed = if reroute {
        let prober = servers::Prober::new();
        servers::reroute(
            &mut info,
            |h| !dead.contains(h) && servers::reachable(h),
            |l, h| prober.serves(l, h),
        )
    } else {
        servers::Rerouted::default()
    };
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
    let at_once = ours.iter().any(|a| a == "--concurrent-fragments");
    let mut meter = Meter::new(expected, at_once);
    let ran = match run(exe, ours, &file, progress, child, Some(&mut meter), pace) {
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
        match run(exe, &r.args(&codes), &file, progress, child, None, pace) {
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

/// A stream in pieces (HLS, DASH): `fragments` pieces at once, as a file comes down over several
/// connections (Options → Connection, #531). yt-dlp uses that for such streams only (a plain file
/// stays one connection). The speed limits hold for the pieces together: the relay paces the
/// whole download (#708, `crate::relay`; yt-dlp's own `--limit-rate` holds for each piece, and
/// only where the relay can't be used, `proc::fixed_pace_args`).
pub fn fragment_args(fragments: u32) -> Vec<String> {
    if fragments > 1 {
        vec!["--concurrent-fragments".into(), fragments.to_string()]
    } else {
        Vec::new()
    }
}

/// Our progress line: `MDMP <downloaded> <total> <estimate> <speed>` (`NA` when unknown).
const PROGRESS: &str = "download:MDMP %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s";
/// The final file: `MDMF <path>`.
const FINAL: &str = "after_move:MDMF %(filepath)s";
/// The quality yt-dlp chose, before it downloads: `MDMQ <format id> <resolution>` (the log names
/// it when it isn't the one asked for, #94).
const QUALITY: &str = "video:MDMQ %(format_id)s %(resolution)s";

impl VideoJob {
    /// Download `url` in format `selector` to `dir/<stem>.<ext>`, held to `pace` (MDM's speed
    /// limits as they change, #708: through the relay, `proc::ytdlp_info_paced`; None: no
    /// limit), a stream in pieces `fragments` pieces at once ([`fragment_args`]); `ffmpeg` (its
    /// exe) merges separate video and audio into an MP4. `expected`: the size the
    /// choice was shown with (every stream), so the progress starts from the whole. `browser`:
    /// what the browser's request for the page carried (proc::FromBrowser). `subs`: the
    /// subtitles to save with it (#68; none for a sound alone, which has no picture to put them
    /// on).
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        exe: &Path,
        url: &str,
        selector: &str,
        dir: &Path,
        stem: &str,
        pace: Option<Arc<Pace>>,
        fragments: u32,
        ffmpeg: Option<&Path>,
        expected: Option<u64>,
        browser: &proc::FromBrowser,
        subs: &Subs,
    ) -> Result<VideoJob, ProcError> {
        proc::safe(url)?;
        let output = dir.join(format!("{}.%(ext)s", template_text(stem)));
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
        ];
        ours.extend(fragment_args(fragments));
        // A subtitles-only second run, for the ones a site refuses (#68): the same output name.
        let mut retry_base: Vec<String> = vec![
            "-o".into(),
            output.to_string_lossy().into_owned(),
            "--no-playlist".into(),
        ];
        match convert {
            Some(a) => {
                ours.extend(["-x".into(), "--audio-format".into(), a.format.into()]);
                if let Some(q) = a.quality {
                    ours.extend(["--audio-quality".into(), q]);
                }
            }
            None => ours.extend(subtitle_args(subs, ffmpeg.is_some())),
        }
        let (theirs, cookies) = browser.args(url)?;
        retry_base.extend(theirs.iter().cloned());
        ours.extend(theirs.iter().cloned());
        if let Some(dir) = ffmpeg.and_then(Path::parent) {
            ours.extend([
                "--ffmpeg-location".into(),
                dir.to_string_lossy().into_owned(),
                "--merge-output-format".into(),
                "mp4".into(),
            ]);
            retry_base.extend([
                "--ffmpeg-location".into(),
                dir.to_string_lossy().into_owned(),
            ]);
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
        for a in &ours {
            proc::safe(a)?;
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
        }));
        let child: Child = Arc::new(Mutex::new(None));
        {
            let (progress, child, pace) = (progress.clone(), child.clone(), pace.clone());
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
                        pace.as_ref(),
                    )
                });
                if std::panic::catch_unwind(run).is_err() {
                    // Given up: nothing may go on downloading unseen (#79).
                    end(&child);
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
            pace,
        })
    }

    /// Its own speed limit, now (None: unlimited); the global cap holds as well.
    pub fn set_speed_limit(&self, rate: Option<u64>) {
        if let Some(p) = &self.pace {
            p.set(rate);
        }
    }

    /// Its own speed limit.
    pub fn speed_limit(&self) -> Option<u64> {
        self.pace.as_ref().and_then(|p| p.get())
    }

    pub fn progress(&self) -> Progress {
        self.progress.lock().unwrap().clone()
    }

    /// Stop (the partial file stays; starting again continues it).
    pub fn stop(&self) {
        self.progress.lock().unwrap().state = State::Stopped;
        if let Some(s) = self.child.lock().unwrap().as_mut() {
            s.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// #531: a stream in pieces comes down several at once, as many as a file's connections;
    /// the relay paces them together (#708). yt-dlp's own limit holds for each piece (measured
    /// against the lab server: `--limit-rate 512K -N 4` took a quarter of the time): where it
    /// is used, one piece at a time.
    #[test]
    fn fragments_at_once_and_the_fixed_limit_one_at_a_time() {
        let s = |v: Vec<String>| v.join(" ");
        assert_eq!(s(fragment_args(8)), "--concurrent-fragments 8");
        assert_eq!(s(fragment_args(1)), "");
        assert_eq!(s(fragment_args(0)), "");
        // yt-dlp's own limit only where the relay can't pace it (#708): one piece at a time.
        assert_eq!(
            s(proc::fixed_pace_args(Some(524_288))),
            "--limit-rate 524288 --concurrent-fragments 1"
        );
        assert_eq!(s(proc::fixed_pace_args(None)), "");
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
        let job = VideoJob::start(
            &exe,
            &page,
            &format,
            dir.path(),
            "a",
            None,
            1,
            ffmpeg.as_deref(),
            None,
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let p = loop {
            let p = job.progress();
            if p.state != State::Running || std::time::Instant::now() > end {
                break p;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        };
        for (_, e) in &p.events {
            println!("event: {e}");
        }
        println!("state: {:?}, {} bytes", p.state, p.downloaded);
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
            1,
            None,
            None,
            &Default::default(),
            &Default::default(),
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
            1,
            None,
            None,
            &Default::default(),
            &Default::default(),
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
        run_with(expected, false, lines)
    }

    fn run_with(expected: Option<u64>, at_once: bool, lines: &[&str]) -> Vec<(u64, Option<u64>)> {
        let mut m = Meter::new(expected, at_once);
        let mut p = Progress {
            state: State::Running,
            downloaded: 0,
            total: None,
            speed: 0.0,
            events: Vec::new(),
            side_files: Vec::new(),
            sub_problems: Vec::new(),
        };
        lines
            .iter()
            .map(|l| {
                m.line(l, &mut p);
                (p.downloaded, p.total)
            })
            .collect()
    }

    fn fractions(v: &[(u64, Option<u64>)]) -> Vec<f64> {
        v.iter()
            .map(|(d, t)| *d as f64 / t.unwrap() as f64)
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

    /// #531: yt-dlp's lines with 8 pieces at once (real ones, from the lab server's HLS stream of
    /// 6 MB), its count stepping back now and then: never more than the file, and the end is
    /// the file. A second stream (the sound after the picture) still adds up.
    #[test]
    fn pieces_at_once_step_back_without_a_new_stream() {
        let lines = [
            "30720 NA 6905856.0 5636",
            "26624 NA 6905856.0 5636",
            "1304576 NA 37576704.0 911582",
            "1961984 NA 25104384.0 911582",
            "1830912 NA 25104384.0 911582",
            "4061184 NA 8382464.0 1683967",
            "3929088 NA 8382464.0 1683967",
            "6289408 NA 7704576.0 1883540",
            "6158336 NA 7704576.0 1883540",
            "6291456 NA 6553600.0 1883540",
        ];
        let v = run_with(None, true, &lines);
        for (d, _) in &v {
            assert!(*d <= 6_291_456, "more than the file: {v:?}");
        }
        assert_eq!(v.last().unwrap().0, 6_291_456);
        // Before, the first step back counted the 6.9 MB estimate as a finished stream.
        assert!(run(None, &lines).iter().any(|(d, _)| *d > 6_291_456));
        let two = run_with(
            None,
            true,
            &[
                "4000000 4000000 NA 5",
                "1024 NA 900000 5",
                "900000 900000 NA 5",
            ],
        );
        assert_eq!(two.last().unwrap().0, 4_900_000, "{two:?}");
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
    /// never looks like the whole download.
    #[test]
    fn expected_size_holds_the_bar_back() {
        let v = run(Some(1000), &["400 400 NA 0"]);
        assert_eq!(v[0], (400, Some(1000)));
        let v = run(None, &["NA NA NA NA"]);
        assert_eq!(v[0], (0, None));
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

    #[test]
    fn percent_signs_in_titles_stay_text() {
        assert_eq!(template_text("100% fun"), "100%% fun");
    }
}
