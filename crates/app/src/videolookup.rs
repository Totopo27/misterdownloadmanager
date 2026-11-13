//! The browser's quality menu over a video (the extension's button, crates/nmhost): MDM looks
//! the video up with yt-dlp on a thread of its own and answers at once with a token
//! (`video_lookup`); the browser asks how it goes (`video_poll`) and sends the choice made there
//! (`video_pick`), a place in the list MDM offered, which MDM downloads with its own selector for
//! it and the look-up's own browser context.
//!
//! Faster than a look-up and a download each running yt-dlp on the page:
//! - an HLS stream's playlist MDM can read exactly as yt-dlp would is read by MDM itself
//!   (`mdm_tools::manifest`), and its qualities offered a second and more sooner; yt-dlp looks
//!   it up at the same time, for the download;
//! - yt-dlp's look-up is kept as it answered (`InfoSlot`), and the download chosen from it
//!   starts from it instead of reading the page again (`mdm_tools::video::Preloaded`);
//! - a second click on the same video, with exactly the same browser context, shares the first
//!   one's look-up while it runs or once it found the video ([`Lookups::share`]).
//!
//! Nothing else is shared between look-ups (a private window's never meets another's, a failure
//! is never reused), and none is kept past 10 minutes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mdm_ipc::api::{VideoChoice, VideoLookupAnswer, VideoNotice, lookup};
use mdm_tools::resolve::{Choice, Looked, Video};
use mdm_tools::video::{InfoSlot, LOOKUP_WAIT, Preloaded, Subs};

use crate::videodlg::VideoAdd;

/// How long a look-up's token is kept.
pub const TTL: Duration = Duration::from_secs(10 * 60);
/// Look-ups kept at once; a new one makes room by forgetting the oldest.
pub const KEPT: usize = 20;
/// yt-dlp look-ups running at once; one more is answered `busy`.
pub const RUNNING: usize = 2;
/// Choices offered at most (a page has a handful; the browser's messages are capped at 1 MB).
const CHOICES: usize = 64;
/// The longest title and message sent to the browser.
const TITLE_MAX: usize = 300;
const MESSAGE_MAX: usize = 2000;

/// Where a look-up is.
#[derive(Debug, Default)]
pub enum Found {
    #[default]
    Looking,
    /// A video, with the choices offered for it (none: nothing this can download).
    Video(Video, Vec<Choice>),
    /// A list of videos (a playlist, a channel): chosen from in MDM's window.
    List,
    /// yt-dlp's error.
    Failed(String),
}

impl Found {
    /// What yt-dlp's look-up found, with the choices the Download video window offers for it
    /// (merged qualities when `merge`: ffmpeg is there).
    pub fn of(res: Result<Looked, String>, merge: bool) -> Found {
        match res {
            Ok(Looked::Video(v)) => {
                let choices = mdm_tools::resolve::choices(&v, merge);
                Found::Video(v, choices)
            }
            Ok(Looked::List(_)) => Found::List,
            Err(e) => Found::Failed(e),
        }
    }
}

/// A look-up's state: written by its thread, read by the questions about it.
pub type Shared = Arc<Mutex<Found>>;

/// The state, even when a thread panicked holding it (it only ever writes a whole value).
fn lock(found: &Shared) -> MutexGuard<'_, Found> {
    found.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One look-up's token.
struct Search {
    /// What yt-dlp looks up.
    address: String,
    /// The browser's request context it came with ([`context_of`]): yt-dlp's, then its
    /// download's.
    context: mdm_ipc::Command,
    /// The browser's cookie store it came from.
    store_id: Option<String>,
    created: Instant,
    found: Shared,
    /// yt-dlp's look-up as it answered, for the download chosen from it.
    info: InfoSlot,
    /// Its choices are merged qualities (ffmpeg was there).
    merge: bool,
}

/// What makes two clicks one look-up ([`Lookups::share`]): the address looked up and everything
/// of the browser's context yt-dlp asks with, exactly, and merged choices or not. Not the page's
/// title: each click keeps its own (the file is named after its page, #345).
#[derive(PartialEq)]
struct Key {
    address: String,
    private: bool,
    store_id: Option<String>,
    cookies: Option<String>,
    jar: Vec<mdm_ipc::BrowserCookie>,
    referer: Option<String>,
    origin: Option<String>,
    user_agent: Option<String>,
    merge: bool,
}

impl Key {
    fn of(context: &mdm_ipc::Command, store_id: &Option<String>, merge: bool) -> Key {
        Key {
            address: context.video_page.clone().unwrap_or_default(),
            private: context.private,
            store_id: store_id.clone(),
            cookies: context.cookies.clone(),
            jar: context.cookie_jar.clone(),
            referer: context.referer.clone(),
            origin: context.origin.clone(),
            user_agent: context.user_agent.clone(),
            merge,
        }
    }
}

/// A look-up another click may share ([`Lookups::share`]), kept apart from the tokens: a choice
/// taken from one of them leaves it here.
struct Recent {
    key: Key,
    started: Instant,
    found: Shared,
    info: InfoSlot,
}

/// A running look-up's place among the [`RUNNING`]: given back when dropped, however its
/// thread ends.
pub struct Running(Arc<AtomicUsize>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A look-up to run on a thread of its own ([`run`]).
pub struct Job {
    pub token: String,
    pub address: String,
    /// yt-dlp's view of the look-up's browser context.
    pub browser: mdm_tools::proc::FromBrowser,
    pub found: Shared,
    pub info: InfoSlot,
    /// The qualities that need merging are offered (ffmpeg is there).
    pub merge: bool,
    pub running: Running,
}

/// A choice made in the browser that fits its look-up.
#[derive(Debug)]
pub struct Picked {
    pub address: String,
    pub context: mdm_ipc::Command,
    pub video: Video,
    pub choice: Choice,
    /// yt-dlp's look-up, for the download (its `Debug` shows nothing of it).
    pub info: InfoSlot,
    pub merge: bool,
}

/// The look-ups the browser's quality menu asked for (`App::lookups`).
#[derive(Default)]
pub struct Lookups {
    searches: HashMap<String, Search>,
    /// The look-ups a click may share, at most [`KEPT`], for [`TTL`] after they began.
    recent: Vec<Recent>,
    running: Arc<AtomicUsize>,
}

impl Lookups {
    /// Look `ask` up with yt-dlp `exe` and the browser's context of `cmd` (`rt`: for a plain
    /// file's server): answered at once, `pending` with its token (`busy` while [`RUNNING`] run
    /// already).
    pub fn start(
        &mut self,
        ask: &mdm_ipc::VideoLookup,
        cmd: &mdm_ipc::Command,
        exe: PathBuf,
        rt: tokio::runtime::Handle,
        now: Instant,
    ) -> VideoLookupAnswer {
        // The qualities that need merging, when ffmpeg is there (as the window offers them).
        let merge = mdm_tools::tools_dir()
            .as_deref()
            .and_then(mdm_tools::ffmpeg::installed)
            .is_some();
        if let Some(token) = self.share(ask, cmd, merge, now) {
            return self.poll(&token, now);
        }
        let Some(job) = self.begin(ask, cmd, merge, now) else {
            return VideoLookupAnswer::of(lookup::BUSY);
        };
        let (token, found, info) = (job.token.clone(), job.found.clone(), job.info.clone());
        match run(job, exe, rt) {
            Ok(()) => VideoLookupAnswer::pending(&token),
            // No thread for it (its place came back with the job): said as yt-dlp's error is.
            Err(e) => {
                *lock(&found) = Found::Failed(e.to_string());
                info.fail();
                self.poll(&token, now)
            }
        }
    }

    /// A token of its own for an earlier look-up of the same thing as `ask` with the browser's
    /// context of `cmd` (a second click on the same video): the same address, the same context
    /// for yt-dlp (kind of window, cookie store, cookies, cookie jar, referrer, User-Agent, all
    /// exactly) and the same kind of choices (`merge`). Shared while it runs, if it began less
    /// than [`LOOKUP_WAIT`] ago (no second yt-dlp, no place among the [`RUNNING`] taken), or once
    /// it found the video, within [`TTL`] of its start, never longer. A failure or a list is
    /// never shared: the click looks it up again. None: nothing to share.
    pub fn share(
        &mut self,
        ask: &mdm_ipc::VideoLookup,
        cmd: &mdm_ipc::Command,
        merge: bool,
        now: Instant,
    ) -> Option<String> {
        self.prune(now);
        let context = context_of(ask, cmd);
        let store_id = store(&ask.store_id);
        let key = Key::of(&context, &store_id, merge);
        let earlier = self.recent.iter().find(|r| {
            let age = now.saturating_duration_since(r.started);
            r.key == key
                && match &*lock(&r.found) {
                    Found::Looking => age < LOOKUP_WAIT,
                    Found::Video(..) => age < TTL,
                    Found::List | Found::Failed(_) => false,
                }
        })?;
        let (found, info) = (earlier.found.clone(), earlier.info.clone());
        self.make_room();
        let token = mdm_ipc::random_token();
        self.searches.insert(
            token.clone(),
            Search {
                address: key.address,
                context,
                store_id,
                created: now,
                found,
                info,
                merge,
            },
        );
        Some(token)
    }

    /// A new look-up of `ask` with the browser's context of `cmd` (`merge`: offering merged
    /// qualities), for the caller to run ([`run`]); none (`busy`) while [`RUNNING`] run already.
    pub fn begin(
        &mut self,
        ask: &mdm_ipc::VideoLookup,
        cmd: &mdm_ipc::Command,
        merge: bool,
        now: Instant,
    ) -> Option<Job> {
        self.prune(now);
        // Only this (the window's) thread takes places; the look-ups' threads only give them
        // back, so the count can't pass RUNNING between the check and the taking.
        if self.running.load(Ordering::SeqCst) >= RUNNING {
            return None;
        }
        self.running.fetch_add(1, Ordering::SeqCst);
        let running = Running(self.running.clone());
        self.make_room();
        let token = mdm_ipc::random_token();
        let context = context_of(ask, cmd);
        let address = context.video_page.clone().unwrap_or_default();
        let store_id = store(&ask.store_id);
        let found = Shared::default();
        let info = InfoSlot::new(now);
        let job = Job {
            token: token.clone(),
            address: address.clone(),
            browser: crate::videodlg::from_browser(&context),
            found: found.clone(),
            info: info.clone(),
            merge,
            running,
        };
        // The one a later click shares, in place of any earlier one of the same thing.
        let key = Key::of(&context, &store_id, merge);
        self.recent.retain(|r| r.key != key);
        while self.recent.len() >= KEPT {
            self.recent.remove(0);
        }
        self.recent.push(Recent {
            key,
            started: now,
            found: found.clone(),
            info: info.clone(),
        });
        self.searches.insert(
            token,
            Search {
                address,
                context,
                store_id,
                created: now,
                found,
                info,
                merge,
            },
        );
        Some(job)
    }

    /// Room for one more token: the oldest forgotten while [`KEPT`] are there.
    fn make_room(&mut self) {
        while self.searches.len() >= KEPT {
            let oldest = self
                .searches
                .iter()
                .min_by_key(|(_, s)| s.created)
                .map(|(t, _)| t.clone());
            match oldest {
                Some(t) => self.searches.remove(&t),
                None => break,
            };
        }
    }

    /// How look-up `token` goes (`gone`: never given, or forgotten).
    pub fn poll(&mut self, token: &str, now: Instant) -> VideoLookupAnswer {
        self.prune(now);
        match self.searches.get(token) {
            Some(s) => answer(token, s),
            None => VideoLookupAnswer::of(lookup::GONE),
        }
    }

    /// The choice `pick` made in the browser (from a private window: `private`), if it fits
    /// its look-up: found, one of the choices offered, from the same kind of window and the
    /// same cookie store. Taken once: the look-up is forgotten then.
    pub fn pick(
        &mut self,
        pick: &mdm_ipc::VideoPick,
        private: bool,
        now: Instant,
    ) -> Option<Picked> {
        self.prune(now);
        let s = self.searches.get(&pick.token)?;
        if s.context.private != private || s.store_id != store(&pick.store_id) {
            return None;
        }
        if pick.choice >= CHOICES {
            return None;
        }
        let (video, choice) = match &*lock(&s.found) {
            Found::Video(v, choices) => (v.clone(), choices.get(pick.choice)?.clone()),
            _ => return None,
        };
        let s = self.searches.remove(&pick.token)?;
        Some(Picked {
            address: s.address,
            context: s.context,
            video,
            choice,
            info: s.info,
            merge: s.merge,
        })
    }

    /// Forgets the look-ups past [`TTL`], which keep a page's cookies: on the app's clock too, so
    /// with no other look-up coming along.
    pub fn prune(&mut self, now: Instant) {
        self.searches
            .retain(|_, s| now.saturating_duration_since(s.created) < TTL);
        self.recent
            .retain(|r| now.saturating_duration_since(r.started) < TTL);
    }
}

/// A cookie store's name, none when blank.
fn store(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// `s` cut to `max` characters.
fn cut(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The browser's request context a look-up keeps, for yt-dlp and then its download (as the
/// Download video window keeps the click's): the address looked up (as the window looks it up,
/// `pages::video_address`), the page's referrer, cookies and User-Agent, whether from a private
/// window, and the page's title (a video yt-dlp knows only by its address is named by it, #345).
pub fn context_of(ask: &mdm_ipc::VideoLookup, cmd: &mdm_ipc::Command) -> mdm_ipc::Command {
    mdm_ipc::Command {
        video_page: Some(mdm_tools::pages::video_address(&ask.address)),
        video_title: ask
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| cut(t, TITLE_MAX)),
        referer: cmd.referer.clone(),
        origin: cmd.origin.clone(),
        cookies: cmd.cookies.clone(),
        cookie_jar: cmd.cookie_jar.clone(),
        user_agent: cmd.user_agent.clone(),
        private: cmd.private,
        ..Default::default()
    }
}

/// The title video `v`, looked up at `address` with the browser's `context`, is offered and
/// saved under, as the Download video window names it: the page's when yt-dlp knows it only by
/// that address (#345), else yt-dlp's.
fn title_of<'a>(v: &'a Video, address: &str, context: &'a mdm_ipc::Command) -> &'a str {
    crate::videodlg::page_title(context, v, address).unwrap_or(&v.title)
}

/// Choice `c` as the browser's quality menu shows it: its label in MDM's language, and what the
/// menu's line says beside it (#755: height, codec, frame rate, range; #801: an about size).
fn menu_choice(c: &Choice) -> VideoChoice {
    VideoChoice {
        label: crate::videodlg::choice_label(&c.label),
        ext: c.ext.clone(),
        size: c.size,
        audio: mdm_tools::resolve::is_audio(c),
        height: c.height,
        fps: c.fps,
        kbps: c.kbps,
        original: mdm_tools::resolve::original_sound(&c.selector),
        codec: c.codec.clone(),
        range: c.range.clone(),
        approx: c.approx && c.size.is_some(),
    }
}

/// What the menu says with video `v`'s choices, as the Download video window says it (#852:
/// YouTube looked up without a JavaScript runtime), with its help page in MDM's language.
fn notice(v: &Video) -> Option<VideoNotice> {
    v.needs_js_runtime.then(|| VideoNotice {
        text: crate::videodlg::js_runtime_note().to_owned(),
        // i18n: link in the browser's video menu, after the note that YouTube needs a JavaScript runtime: opens the help page that says how to install Node.js or Deno
        link: crate::i18n::t("How to install one").to_owned(),
        url: crate::help::page_url(crate::videodlg::JS_RUNTIME_HELP, crate::i18n::code()),
    })
}

/// The answer for look-up `token` as it stands: the choices labelled in MDM's language, an
/// error in its words (as the Download video window says them).
fn answer(token: &str, s: &Search) -> VideoLookupAnswer {
    match &*lock(&s.found) {
        Found::Looking => VideoLookupAnswer::pending(token),
        Found::Video(v, choices) if choices.is_empty() => VideoLookupAnswer {
            notice: notice(v),
            ..VideoLookupAnswer::failed(lookup::NO_CHOICES, None)
        },
        Found::Video(v, choices) => VideoLookupAnswer {
            token: Some(token.to_owned()),
            title: Some(cut(title_of(v, &s.address, &s.context), TITLE_MAX)),
            choices: choices.iter().take(CHOICES).map(menu_choice).collect(),
            notice: notice(v),
            ..VideoLookupAnswer::of(lookup::READY)
        },
        Found::List => VideoLookupAnswer::of(lookup::LIST),
        Found::Failed(e) => {
            // Whose cookies the look-up had (#229): the extension's, or (none came) those of
            // the browser Options names.
            let sent = s
                .context
                .cookies
                .as_deref()
                .is_some_and(|c| !c.trim().is_empty());
            let store = (!sent).then(mdm_tools::proc::browser_cookies).flatten();
            let message = crate::videodlg::lookup_error(e, sent, store.as_ref());
            VideoLookupAnswer::failed(lookup::ERROR, Some(cut(&message, MESSAGE_MAX)))
        }
    }
}

/// The download choice `p` adds, into `folder` (the Video category's), or `music` (the Music
/// category's, None when the user deleted it) for a sound-only choice, listed under that category
/// as the Download video window does (#611): MDM's own selector for it, the file named as the
/// Download video window names it, the subtitles that window would preselect from `pref`
/// (settings.video_subs, not changed by it), started now unless `start_now` is false (Options:
/// only add to the queue), from the look-up's own yt-dlp answer (its first run). None for an
/// address MDM can't take.
pub fn video_add(
    p: Picked,
    folder: &Path,
    music: Option<&Path>,
    start_now: bool,
    pref: &Subs,
) -> Option<VideoAdd> {
    let page = url::Url::parse(&p.address)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"))?;
    let name = crate::videodlg::file_name(title_of(&p.video, &p.address, &p.context), &p.choice);
    let (category, folder) = match music {
        Some(m) if mdm_tools::resolve::is_audio(&p.choice) => (crate::model::Category::Music, m),
        _ => (crate::model::Category::Video, folder),
    };
    Some(VideoAdd {
        page,
        selector: p.choice.selector.clone(),
        dest: folder.join(mdm_engine::naming::sanitize(name.trim())),
        category,
        size: p.choice.size,
        start_now,
        subs: crate::videodlg::default_subs(&p.video, &p.choice, pref),
        context: p.context,
        remember: None,
        preloaded: Some(Preloaded {
            slot: p.info,
            selector: p.choice.selector,
            label: p.choice.label,
            merge: p.merge,
        }),
    })
}

/// Writes the look-up's end when its thread ends without one (a panic on it): nothing stays
/// "looking" (a second click would share it) nor pending (its download would wait for it).
struct Ended {
    found: Shared,
    info: InfoSlot,
}

impl Drop for Ended {
    fn drop(&mut self) {
        let mut found = lock(&self.found);
        if matches!(*found, Found::Looking) {
            // yt-dlp's words for an end without an answer.
            *found = Found::Failed(
                mdm_tools::proc::ProcError::Failed {
                    tool: "yt-dlp".into(),
                    message: "it stopped without saying why".into(),
                }
                .to_string(),
            );
        }
        self.info.fail();
    }
}

/// yt-dlp's look-up as `videodlg::look_up_kept` makes it: what it found, and its answer as it
/// gave it.
type Kept = (Result<Looked, String>, Option<mdm_tools::proc::LookedUp>);

/// A look-up's two readings at once, the first answer taking the menu: MDM's own of a stream's
/// playlists (`fast`, on the runtime `rt`, 5 s at most; preferred, and much the quicker), and
/// yt-dlp's (`slow`, on this thread), started at once whatever the other does (a slow playlist
/// server never holds it up) and kept for the download either way (`info`). An answer once
/// written stays (the browser may be showing its choices, and picks by place in them); a
/// failure of yt-dlp's waits for MDM's reading, which may still bring the video.
fn settle(
    found: &Shared,
    info: &InfoSlot,
    merge: bool,
    rt: &tokio::runtime::Handle,
    fast: impl std::future::Future<Output = Option<Video>> + Send + 'static,
    slow: impl FnOnce() -> Kept,
) {
    let mine = found.clone();
    let reading = rt.spawn(async move {
        if let Some(v) = fast.await {
            let mut found = lock(&mine);
            if matches!(*found, Found::Looking) {
                *found = Found::of(Ok(Looked::Video(v)), merge);
            }
        }
    });
    let (res, kept) = slow();
    match (&res, kept) {
        (Ok(Looked::Video(_)), Some(out)) => info.fill(out.json, out.without_cookies),
        _ => info.fail(),
    }
    if res.is_err() {
        let _ = rt.block_on(reading);
    }
    let mut found = lock(found);
    if matches!(*found, Found::Looking) {
        *found = Found::of(res, merge);
    }
}

/// Run `job` with yt-dlp `exe` on a thread of its own, as the Download video window looks a
/// video up (`rt`: a plain file's size from its server, and an HLS stream's playlists): its
/// state written when yt-dlp answers (within its 90 s, twice that without the cookies), or
/// sooner when MDM reads the stream itself ([`settle`]), its place given back as the thread
/// ends. A look-up forgotten meanwhile (10 minutes, or 20 newer ones) ends all the same, with
/// nothing to read it.
pub fn run(job: Job, exe: PathBuf, rt: tokio::runtime::Handle) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("mdm-video-lookup".into())
        .spawn(move || {
            let Job {
                address,
                browser,
                found,
                info,
                merge,
                running,
                ..
            } = job;
            let _ended = Ended {
                found: found.clone(),
                info: info.clone(),
            };
            // A stream's playlist MDM reads exactly as yt-dlp would (as Options' proxy and the
            // browser's request for it); none for anything else.
            let fast = {
                let (address, browser) = (address.clone(), browser.clone());
                let opts = crate::settings::own_net();
                async move { mdm_tools::manifest::fetch(&address, &browser, &opts).await }
            };
            // yt-dlp's look-up all the same (the click asked for the video): kept for the
            // download, and the menu's answer when MDM can't read the stream.
            settle(&found, &info, merge, &rt, fast, || {
                crate::videodlg::look_up_kept(&exe, &address, &browser, &rt)
            });
            drop(running);
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdm_ipc::{Command, VideoLookup, VideoPick};

    /// A stream's manifest as yt-dlp sees it (named after its address) with two variants and
    /// German subtitles.
    const STREAM: &str = r#"{"title": "master", "duration": 60, "formats": [
        {"format_id": "hls-2500", "ext": "mp4", "height": 720, "vcodec": "avc1", "acodec": "mp4a", "tbr": 2500},
        {"format_id": "hls-800", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a", "tbr": 800}],
        "subtitles": {"de": [{"name": "German"}]}}"#;
    const SRC: &str = "https://cdn.test/v/hls/master.m3u8";
    const PAGE: &str = "https://videos.test/watch?v=1";

    fn look(address: &str, title: Option<&str>, store: Option<&str>) -> VideoLookup {
        VideoLookup {
            address: address.into(),
            title: title.map(str::to_owned),
            store_id: store.map(str::to_owned),
        }
    }

    /// The host's command: the browser's context with the look-up.
    fn sent(private: bool, cookies: &str) -> Command {
        Command {
            referer: Some(PAGE.into()),
            cookies: Some(cookies.into()),
            user_agent: Some("UA".into()),
            private,
            startup: true,
            ..Default::default()
        }
    }

    fn pick(token: &str, choice: usize, store: Option<&str>) -> VideoPick {
        VideoPick {
            token: token.into(),
            choice,
            store_id: store.map(str::to_owned),
        }
    }

    /// The look-up's thread, as yt-dlp answers it.
    fn answered(job: &Job, json: &str) {
        let v = mdm_tools::resolve::parse_looked(json.as_bytes()).unwrap();
        *lock(&job.found) = Found::of(Ok(v), false);
    }

    #[test]
    fn each_look_up_keeps_its_own_context() {
        let mut l = Lookups::default();
        let now = Instant::now();
        let a = l
            .begin(
                &look(
                    SRC,
                    Some(" A talk: in Lisbon "),
                    Some("firefox-container-1"),
                ),
                &sent(false, "a=1"),
                false,
                now,
            )
            .unwrap();
        let b = l
            .begin(&look(PAGE, None, None), &sent(false, "b=2"), false, now)
            .unwrap();
        assert_ne!(a.token, b.token);
        assert_eq!(a.token.len(), 64);
        // yt-dlp asks with each one's own.
        assert_eq!(
            (a.address.as_str(), a.browser.cookies.as_deref()),
            (SRC, Some("a=1"))
        );
        assert_eq!(
            (b.address.as_str(), b.browser.cookies.as_deref()),
            (PAGE, Some("b=2"))
        );
        assert_eq!(a.browser.referer.as_deref(), Some(PAGE));
        assert_eq!(l.poll(&a.token, now), VideoLookupAnswer::pending(&a.token));
        answered(&a, STREAM);
        let ready = l.poll(&a.token, now);
        assert_eq!(ready.state, lookup::READY);
        assert_eq!(ready.token.as_deref(), Some(a.token.as_str()));
        assert_eq!(
            ready.title.as_deref(),
            Some("A talk: in Lisbon"),
            "the page's, not yt-dlp's `master`"
        );
        assert_eq!(ready.choices.len(), 2);
        assert!(
            ready.choices[0].label.contains("720p"),
            "{:?}",
            ready.choices
        );
        assert_eq!(ready.choices[0].ext, "mp4");
        assert_eq!(ready.choices[0].size, Some(60 * 2500 * 125), "estimated");
        assert!(ready.choices.iter().all(|c| !c.audio), "videos");
        assert_eq!(ready.notice, None, "nothing lacked");
        assert_eq!(l.poll(&b.token, now).state, lookup::PENDING, "its own");
        // The choice takes the look-up's context and MDM's selector for that place.
        let p = l
            .pick(&pick(&a.token, 1, Some("firefox-container-1")), false, now)
            .unwrap();
        assert_eq!(p.address, SRC);
        assert_eq!(p.context.cookies.as_deref(), Some("a=1"));
        assert_eq!(p.context.referer.as_deref(), Some(PAGE));
        assert_eq!(p.context.user_agent.as_deref(), Some("UA"));
        assert_eq!(p.context.video_title.as_deref(), Some("A talk: in Lisbon"));
        assert!(!p.context.private && !p.context.startup);
        assert_eq!(p.choice.selector, "hls-800/b[height=360]");
        // Once.
        assert!(
            l.pick(&pick(&a.token, 1, Some("firefox-container-1")), false, now)
                .is_none()
        );
        assert_eq!(l.poll(&a.token, now).state, lookup::GONE);
        assert_eq!(l.poll("nonsense", now).state, lookup::GONE);
    }

    /// A private window's look-up is taken from a private window only, and from the same
    /// cookie store; a normal one from a normal window only.
    #[test]
    fn a_private_windows_look_up_is_its_own() {
        let mut l = Lookups::default();
        let now = Instant::now();
        let private = l
            .begin(
                &look(PAGE, None, Some("firefox-private")),
                &sent(true, "p=1"),
                false,
                now,
            )
            .unwrap();
        let normal = l
            .begin(&look(PAGE, None, None), &sent(false, "n=1"), false, now)
            .unwrap();
        answered(&private, STREAM);
        answered(&normal, STREAM);
        let t = private.token.clone();
        assert!(
            l.pick(&pick(&t, 0, Some("firefox-private")), false, now)
                .is_none()
        );
        assert!(l.pick(&pick(&t, 0, None), true, now).is_none());
        assert!(
            l.pick(&pick(&t, 0, Some("firefox-default")), true, now)
                .is_none()
        );
        assert!(l.pick(&pick(&normal.token, 0, None), true, now).is_none());
        assert!(
            l.pick(&pick(&normal.token, 0, Some("firefox-private")), false, now)
                .is_none()
        );
        // The refusals forgot nothing.
        let p = l
            .pick(&pick(&t, 0, Some("firefox-private")), true, now)
            .unwrap();
        assert!(p.context.private);
        assert_eq!(p.context.cookies.as_deref(), Some("p=1"));
        // A blank store is none.
        assert!(
            l.pick(&pick(&normal.token, 0, Some(" ")), false, now)
                .is_some()
        );
    }

    /// A second click on the same video, with the same browser context, gets a token of its own
    /// for the first click's look-up: no second yt-dlp, no place among the two taken, the same
    /// answer when it comes, yt-dlp's kept answer for its download; its own page title (each
    /// click names its file after its page). Shared after a choice was taken from it, too.
    #[test]
    fn a_second_click_shares_the_first_look_up() {
        let mut l = Lookups::default();
        let now = Instant::now();
        let first = l
            .begin(
                &look(SRC, Some("Talk A"), None),
                &sent(false, "a=1"),
                false,
                now,
            )
            .unwrap();
        let other = l
            .begin(&look(PAGE, None, None), &sent(false, "b=2"), false, now)
            .unwrap();
        // Both places taken: still shared.
        let later = now + Duration::from_secs(30);
        let second = l
            .share(
                &look(SRC, Some("Talk B"), None),
                &sent(false, "a=1"),
                false,
                later,
            )
            .expect("shared");
        assert_ne!(second, first.token);
        assert_eq!(l.running.load(Ordering::SeqCst), RUNNING, "no place taken");
        assert_eq!(l.poll(&second, later), VideoLookupAnswer::pending(&second));
        answered(&first, STREAM);
        first.info.fill(STREAM.as_bytes().to_vec(), false);
        let (a, b) = (l.poll(&first.token, later), l.poll(&second, later));
        assert_eq!(
            (a.state.as_str(), b.state.as_str()),
            (lookup::READY, lookup::READY)
        );
        assert_eq!(a.choices, b.choices);
        assert_eq!(
            (a.title.as_deref(), b.title.as_deref()),
            (Some("Talk A"), Some("Talk B")),
            "each its own page's"
        );
        let p = l.pick(&pick(&second, 0, None), false, later).unwrap();
        assert!(!p.info.pending(), "the first click's yt-dlp answer");
        assert_eq!(p.context.video_title.as_deref(), Some("Talk B"));
        // A choice taken leaves it to share.
        assert!(
            l.share(&look(SRC, None, None), &sent(false, "a=1"), false, later)
                .is_some()
        );
        drop(other);
    }

    /// The context a second click must share exactly, one part of it changed.
    fn shares_with(change: impl Fn(&mut VideoLookup, &mut Command, &mut bool)) -> bool {
        let mut l = Lookups::default();
        let now = Instant::now();
        let base = || {
            let mut cmd = sent(false, "a=1");
            cmd.cookie_jar = vec![mdm_ipc::BrowserCookie {
                name: "sid".into(),
                value: "1".into(),
                domain: ".cdn.test".into(),
                ..Default::default()
            }];
            (
                look(SRC, Some("A talk"), Some("firefox-container-1")),
                cmd,
                false,
            )
        };
        let (ask, cmd, merge) = base();
        let job = l.begin(&ask, &cmd, merge, now).unwrap();
        let (mut ask, mut cmd, mut merge) = base();
        change(&mut ask, &mut cmd, &mut merge);
        let shared = l.share(&ask, &cmd, merge, now).is_some();
        drop(job);
        shared
    }

    #[test]
    fn the_same_context_shares() {
        assert!(shares_with(|_, _, _| {}));
        // The page's title isn't part of it.
        assert!(shares_with(
            |a, _, _| a.title = Some("Another tab's title".into())
        ));
    }

    #[test]
    fn another_address_is_its_own_look_up() {
        assert!(!shares_with(|a, _, _| a.address = PAGE.into()));
    }

    #[test]
    fn another_kind_of_window_is_its_own_look_up() {
        assert!(!shares_with(|_, c, _| c.private = true));
    }

    #[test]
    fn another_cookie_store_is_its_own_look_up() {
        assert!(!shares_with(
            |a, _, _| a.store_id = Some("firefox-container-2".into())
        ));
        assert!(!shares_with(|a, _, _| a.store_id = None));
    }

    #[test]
    fn other_cookies_are_their_own_look_up() {
        assert!(!shares_with(|_, c, _| c.cookies = Some("a=2".into())));
        assert!(!shares_with(|_, c, _| c.cookies = None));
    }

    #[test]
    fn another_cookie_jar_is_its_own_look_up() {
        assert!(!shares_with(|_, c, _| c.cookie_jar[0].value = "2".into()));
        assert!(!shares_with(|_, c, _| c.cookie_jar.clear()));
    }

    #[test]
    fn another_referrer_is_its_own_look_up() {
        assert!(!shares_with(
            |_, c, _| c.referer = Some("https://videos.test/other".into())
        ));
    }

    #[test]
    fn another_user_agent_is_its_own_look_up() {
        assert!(!shares_with(|_, c, _| c.user_agent = Some("UA/2".into())));
    }

    #[test]
    fn other_choices_are_their_own_look_up() {
        assert!(!shares_with(|_, _, m| *m = true));
    }

    /// Never shared: a failure or a list (the click looks it up again), a look-up running longer
    /// than it may, a video found more than ten minutes after its look-up began (however late a
    /// click shared it before).
    #[test]
    fn what_a_click_never_shares() {
        let now = Instant::now();
        let ask = look(SRC, None, None);
        let cmd = sent(false, "a=1");
        let mut l = Lookups::default();
        let job = l.begin(&ask, &cmd, false, now).unwrap();
        *lock(&job.found) = Found::Failed("yt-dlp failed".into());
        assert!(l.share(&ask, &cmd, false, now).is_none());
        *lock(&job.found) = Found::List;
        assert!(l.share(&ask, &cmd, false, now).is_none());
        *lock(&job.found) = Found::Looking;
        assert!(
            l.share(
                &ask,
                &cmd,
                false,
                now + LOOKUP_WAIT - Duration::from_secs(1)
            )
            .is_some()
        );
        assert!(l.share(&ask, &cmd, false, now + LOOKUP_WAIT).is_none());
        answered(&job, STREAM);
        let late = now + TTL - Duration::from_secs(1);
        let token = l.share(&ask, &cmd, false, late).unwrap();
        assert!(
            l.share(&ask, &cmd, false, now + TTL).is_none(),
            "never extended"
        );
        // The token given late lives its own ten minutes.
        assert_eq!(l.poll(&token, now + TTL).state, lookup::READY);
        // A new look-up of the same thing takes the old one's place.
        let mut l = Lookups::default();
        let a = l.begin(&ask, &cmd, false, now).unwrap();
        let b = l.begin(&ask, &cmd, false, now).unwrap();
        answered(&a, STREAM);
        let t = l.share(&ask, &cmd, false, now).unwrap();
        answered(&b, &STREAM.replace("hls-2500", "other"));
        assert!(Arc::ptr_eq(&l.searches[&t].found, &b.found));
        // At most KEPT of them.
        let mut l = Lookups::default();
        for i in 0..KEPT + 5 {
            let job = l
                .begin(&look(&format!("{SRC}?{i}"), None, None), &cmd, false, now)
                .unwrap();
            drop(job);
        }
        assert_eq!(l.recent.len(), KEPT);
    }

    /// A look-up's thread that ends without an answer (a panic) leaves it failed, never
    /// "looking" (a second click would share it) nor its yt-dlp answer pending (its download
    /// would wait for it).
    #[test]
    fn a_look_up_ended_without_an_answer_is_failed() {
        let found = Shared::default();
        let info = InfoSlot::new(Instant::now());
        let (f, i) = (found.clone(), info.clone());
        let ended = std::thread::spawn(move || {
            let _ended = Ended { found: f, info: i };
            panic!("on the look-up's thread");
        })
        .join();
        assert!(ended.is_err());
        assert!(
            matches!(&*lock(&found), Found::Failed(e) if e.contains("stopped without saying why"))
        );
        assert!(!info.pending());
        // The menu's answer stays when only yt-dlp's for the download is missing.
        let found: Shared = Arc::new(Mutex::new(Found::List));
        let info = InfoSlot::new(Instant::now());
        drop(Ended {
            found: found.clone(),
            info: info.clone(),
        });
        assert!(matches!(&*lock(&found), Found::List));
        assert!(!info.pending());
    }

    /// MDM's reading of a stream and yt-dlp's run at once: yt-dlp starts at once however slow
    /// the stream's server, the first answer takes the menu and stays, MDM's reading wins
    /// against a yt-dlp that failed; yt-dlp's answer is kept for the download either way.
    #[test]
    fn the_two_readings_run_at_once() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let video = |title: &str| {
            mdm_tools::resolve::parse(
                STREAM
                    .replace("\"master\"", &format!("{title:?}"))
                    .as_bytes(),
            )
            .unwrap()
        };
        let ytdlp = |title: &str| -> Kept {
            (
                Ok(Looked::Video(video(title))),
                Some(mdm_tools::proc::LookedUp {
                    json: STREAM.as_bytes().to_vec(),
                    without_cookies: false,
                    warnings: Vec::new(),
                }),
            )
        };
        let failed = || -> Kept { (Err("yt-dlp failed: no".into()), None) };
        let title = |found: &Shared| match &*lock(found) {
            Found::Video(v, _) => v.title.clone(),
            Found::Failed(e) => format!("failed: {e}"),
            _ => "other".into(),
        };
        let reading = |title: &'static str, ms: u64| {
            let v = (!title.is_empty()).then(|| video(title));
            async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                v
            }
        };
        let fresh = || (Shared::default(), InfoSlot::new(Instant::now()));

        // MDM's reading first: the menu's; yt-dlp's kept for the download.
        let (found, info) = fresh();
        settle(
            &found,
            &info,
            false,
            rt.handle(),
            reading("mine", 0),
            || {
                std::thread::sleep(Duration::from_millis(200));
                ytdlp("yt-dlp's")
            },
        );
        assert_eq!(title(&found), "mine");
        assert!(!info.pending());

        // yt-dlp first: its answer, which MDM's later reading doesn't replace.
        let (found, info) = fresh();
        settle(
            &found,
            &info,
            false,
            rt.handle(),
            reading("mine", 300),
            || ytdlp("yt-dlp's"),
        );
        assert_eq!(title(&found), "yt-dlp's");
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(title(&found), "yt-dlp's", "stays");
        drop(info);

        // yt-dlp failed first: MDM's reading still brings the video.
        let (found, info) = fresh();
        settle(
            &found,
            &info,
            false,
            rt.handle(),
            reading("mine", 200),
            failed,
        );
        assert_eq!(title(&found), "mine");
        assert!(!info.pending(), "nothing kept: the download looks it up");

        // Both fail: yt-dlp's error.
        let (found, info) = fresh();
        settle(&found, &info, false, rt.handle(), reading("", 100), failed);
        assert_eq!(title(&found), "failed: yt-dlp failed: no");
        drop(info);

        // A slow playlist server never holds yt-dlp up.
        let (found, info) = fresh();
        let begun = Instant::now();
        let mut started = None;
        settle(&found, &info, false, rt.handle(), reading("", 3000), || {
            started = Some(begun.elapsed());
            ytdlp("yt-dlp's")
        });
        assert!(started.unwrap() < Duration::from_millis(100), "{started:?}");
        assert!(begun.elapsed() < Duration::from_secs(1));
        assert_eq!(title(&found), "yt-dlp's");
        drop(info);
    }

    /// The menu looks up what the Download video window would: Dailymotion's embedded player is
    /// the video's own page (#359), downloaded from there too.
    #[test]
    fn the_menu_looks_up_what_the_window_would() {
        let mut l = Lookups::default();
        let now = Instant::now();
        let player = "https://geo.dailymotion.com/player/x19jsm.html?video=k4lGbCpdJJFSQgMvE8W";
        let own = "https://www.dailymotion.com/video/k4lGbCpdJJFSQgMvE8W";
        let job = l
            .begin(&look(player, None, None), &sent(false, ""), false, now)
            .unwrap();
        assert_eq!(job.address, own);
        answered(&job, STREAM);
        let p = l.pick(&pick(&job.token, 0, None), false, now).unwrap();
        assert_eq!(
            (p.address.as_str(), p.context.video_page.as_deref()),
            (own, Some(own))
        );
    }

    /// Two yt-dlp look-ups at a time: the third is `busy` until one of them ends, its place given
    /// back whatever happens to its token.
    #[test]
    fn two_look_ups_at_a_time() {
        let mut l = Lookups::default();
        let now = Instant::now();
        let a = l
            .begin(&look(PAGE, None, None), &sent(false, ""), false, now)
            .unwrap();
        let b = l
            .begin(&look(PAGE, None, None), &sent(false, ""), false, now)
            .unwrap();
        assert!(
            l.begin(&look(PAGE, None, None), &sent(false, ""), false, now)
                .is_none(),
            "busy"
        );
        // One ends on its own thread.
        let Job { running, .. } = a;
        std::thread::spawn(move || drop(running)).join().unwrap();
        let c = l
            .begin(&look(PAGE, None, None), &sent(false, ""), false, now)
            .unwrap();
        assert!(
            l.begin(&look(PAGE, None, None), &sent(false, ""), false, now)
                .is_none()
        );
        // Forgotten tokens' threads still give their places back.
        let later = now + TTL;
        assert_eq!(l.poll(&b.token, later).state, lookup::GONE);
        drop(b);
        drop(c);
        assert!(
            l.begin(&look(PAGE, None, None), &sent(false, ""), false, later)
                .is_some()
        );
        assert_eq!(l.running.load(Ordering::SeqCst), 0, "dropped at once here");
    }

    /// Ten minutes, or twenty newer look-ups, and a token is gone.
    #[test]
    fn tokens_are_forgotten() {
        let mut l = Lookups::default();
        let now = Instant::now();
        let first = l
            .begin(&look(PAGE, None, None), &sent(false, ""), false, now)
            .unwrap()
            .token;
        assert_eq!(
            l.poll(&first, now + TTL - Duration::from_secs(1)).state,
            lookup::PENDING
        );
        assert_eq!(l.poll(&first, now + TTL).state, lookup::GONE);
        let tokens: Vec<String> = (0..=KEPT as u64)
            .map(|i| {
                l.begin(
                    &look(PAGE, None, None),
                    &sent(false, ""),
                    false,
                    now + Duration::from_millis(i),
                )
                .unwrap()
                .token
            })
            .collect();
        let at = now + Duration::from_secs(1);
        assert_eq!(l.poll(&tokens[0], at).state, lookup::GONE, "the oldest");
        assert_eq!(l.poll(&tokens[KEPT], at).state, lookup::PENDING);
        assert_eq!(l.poll(&tokens[1], at).state, lookup::PENDING);
        assert_eq!(l.searches.len(), KEPT);
    }

    /// The browser never names a selector: only a place among the choices offered, while they
    /// are the ones offered.
    #[test]
    fn a_choice_must_be_one_offered() {
        let mut l = Lookups::default();
        let now = Instant::now();
        let job = l
            .begin(&look(SRC, None, None), &sent(false, ""), false, now)
            .unwrap();
        let t = job.token.clone();
        assert!(
            l.pick(&pick(&t, 0, None), false, now).is_none(),
            "still looking"
        );
        answered(&job, STREAM);
        assert!(
            l.pick(&pick(&t, 2, None), false, now).is_none(),
            "out of range"
        );
        assert!(l.pick(&pick(&t, usize::MAX, None), false, now).is_none());
        assert!(l.pick(&pick("other", 0, None), false, now).is_none());
        assert!(
            l.pick(&pick(&t, 0, None), false, now + TTL).is_none(),
            "forgotten"
        );
        let job = l
            .begin(&look(SRC, None, None), &sent(false, ""), false, now)
            .unwrap();
        *lock(&job.found) = Found::List;
        assert!(l.pick(&pick(&job.token, 0, None), false, now).is_none());
        *lock(&job.found) = Found::Failed("yt-dlp failed".into());
        assert!(l.pick(&pick(&job.token, 0, None), false, now).is_none());
    }

    /// What isn't a video to choose from is said: a list, nothing to download, yt-dlp's error
    /// (in MDM's words when it has some).
    #[test]
    fn lists_and_failures_are_said() {
        let mut l = Lookups::default();
        let now = Instant::now();
        let job = l
            .begin(&look(PAGE, None, None), &sent(false, ""), false, now)
            .unwrap();
        answered(
            &job,
            r#"{"_type": "playlist", "title": "Talks", "playlist_count": 12}"#,
        );
        assert_eq!(l.poll(&job.token, now), VideoLookupAnswer::of(lookup::LIST));
        answered(
            &job,
            r#"{"title": "Slides", "formats": [{"format_id": "0", "ext": "jpg", "vcodec": "none", "acodec": "none"}]}"#,
        );
        assert_eq!(
            l.poll(&job.token, now),
            VideoLookupAnswer::failed(lookup::NO_CHOICES, None)
        );
        *lock(&job.found) = Found::Failed("yt-dlp failed: Video unavailable".into());
        let failed = l.poll(&job.token, now);
        assert_eq!(
            (failed.state.as_str(), failed.reason.as_deref()),
            (lookup::FAILED, Some(lookup::ERROR))
        );
        assert_eq!(
            failed.message.as_deref(),
            Some("yt-dlp failed: Video unavailable")
        );
        *lock(&job.found) = Found::Failed("x".repeat(10_000));
        assert_eq!(
            l.poll(&job.token, now).message.unwrap().chars().count(),
            MESSAGE_MAX
        );
        assert!(matches!(Found::of(Err("e".into()), true), Found::Failed(_)));
    }

    /// The choice is added with MDM's selector for it, named after the page that played it,
    /// with the subtitles the window would preselect; started, or queued when Options say so.
    #[test]
    fn the_choice_is_added_as_the_window_would() {
        let mut l = Lookups::default();
        let now = Instant::now();
        let folder = PathBuf::from("downloads").join("Video");
        let de = Subs {
            langs: vec!["de".into()],
            automatic: false,
            embed: false,
        };
        let job = l
            .begin(
                &look(SRC, Some("A talk: in Lisbon"), None),
                &sent(false, "a=1"),
                false,
                now,
            )
            .unwrap();
        answered(&job, STREAM);
        let p = l.pick(&pick(&job.token, 0, None), false, now).unwrap();
        drop(job);
        let add = video_add(p, &folder, None, true, &de).unwrap();
        assert_eq!(add.page.as_str(), SRC);
        assert_eq!(
            add.selector, "hls-2500/b[height=720]",
            "MDM's own, never the browser's"
        );
        assert_eq!(add.dest, folder.join("A talk_ in Lisbon.mp4"));
        assert_eq!(add.size, Some(60 * 2500 * 125));
        assert!(add.start_now);
        assert_eq!(add.context.cookies.as_deref(), Some("a=1"));
        assert_eq!(
            add.subs.langs,
            ["de"],
            "the last video's choice, on this page too"
        );
        assert!(add.remember.is_none(), "the remembered choice stays");
        // The look-up's yt-dlp answer goes with it, for the very choice made.
        let pre = add.preloaded.as_ref().unwrap();
        assert_eq!(
            (pre.selector.as_str(), pre.label.as_str(), pre.merge),
            ("hls-2500/b[height=720]", "720p · H.264", false)
        );
        // No page title (a page yt-dlp names itself), Options' "only add to the queue", a
        // language this page doesn't have.
        let job = l
            .begin(&look(SRC, None, None), &sent(false, ""), false, now)
            .unwrap();
        answered(&job, STREAM);
        let p = l.pick(&pick(&job.token, 1, None), false, now).unwrap();
        drop(job);
        let ja = Subs {
            langs: vec!["ja".into()],
            ..Default::default()
        };
        let add = video_add(p, &folder, None, false, &ja).unwrap();
        assert_eq!(add.dest, folder.join("master.mp4"));
        assert_eq!(add.selector, "hls-800/b[height=360]");
        assert!(!add.start_now);
        assert!(add.subs.langs.is_empty());
        // A video yt-dlp titled itself keeps its title, whatever the page's (#345).
        let job = l
            .begin(
                &look(PAGE, Some("The page's title"), None),
                &sent(false, ""),
                false,
                now,
            )
            .unwrap();
        answered(&job, &STREAM.replace("\"master\"", "\"Its own title\""));
        assert_eq!(
            l.poll(&job.token, now).title.as_deref(),
            Some("Its own title")
        );
        let p = l.pick(&pick(&job.token, 0, None), false, now).unwrap();
        drop(job);
        let add = video_add(p, &folder, None, true, &Subs::default()).unwrap();
        assert_eq!(add.dest, folder.join("Its own title.mp4"));
        // Never an address that isn't on the web.
        let job = l
            .begin(
                &look("file:///C:/v.mp4", None, None),
                &sent(false, ""),
                false,
                now,
            )
            .unwrap();
        answered(&job, STREAM);
        let p = l.pick(&pick(&job.token, 0, None), false, now).unwrap();
        assert!(video_add(p, &folder, None, true, &Subs::default()).is_none());
    }

    /// What the browser's menu is told of each choice (#755, #801): the height, frame rate,
    /// codec and range of a video's, the sound's bitrate, whether its size is about.
    #[test]
    fn the_buttons_details_of_each_choice() {
        const V: &str = r#"{"title": "A talk", "duration": 60, "formats": [
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a", "tbr": 129.5, "filesize": 900},
            {"format_id": "701", "ext": "mp4", "height": 1080, "fps": 60, "vcodec": "av01.0.09M.10", "acodec": "none", "dynamic_range": "HDR10", "filesize": 9000},
            {"format_id": "299", "ext": "mp4", "height": 1080, "fps": 60, "vcodec": "avc1", "acodec": "none", "dynamic_range": "SDR", "filesize": 8000},
            {"format_id": "136", "ext": "mp4", "height": 720, "fps": 30, "vcodec": "avc1", "acodec": "none", "filesize_approx": 7000}]}"#;
        let v = mdm_tools::resolve::parse(V.as_bytes()).unwrap();
        let a: Vec<VideoChoice> = mdm_tools::resolve::choices(&v, true)
            .iter()
            .map(menu_choice)
            .collect();
        let of = |i: usize| {
            let c = &a[i];
            (
                c.height,
                c.fps,
                c.codec.as_deref(),
                c.range.as_deref(),
                c.size,
                c.approx,
            )
        };
        assert_eq!(
            of(0),
            (
                Some(1080),
                Some(60),
                Some("AV1"),
                Some("HDR"),
                Some(9900),
                false
            )
        );
        assert_eq!(
            of(1),
            (
                Some(1080),
                Some(60),
                Some("H.264"),
                Some("SDR"),
                Some(8900),
                false
            )
        );
        assert_eq!(
            of(2),
            (Some(720), Some(30), Some("H.264"), None, Some(7900), true),
            "a size the site only guessed"
        );
        let sound = &a[3];
        assert!(sound.original && sound.audio);
        assert_eq!(
            (sound.kbps, sound.size, sound.approx),
            (Some(130), Some(900), false)
        );
        let mp3 = a.iter().find(|c| c.label.contains("320")).unwrap();
        assert_eq!(
            (mp3.kbps, mp3.approx, mp3.original),
            (Some(320), true, false)
        );
        let vbr = a.iter().find(|c| c.label.contains("VBR")).unwrap();
        assert_eq!(vbr.kbps, None, "VBR says no rate");
    }

    /// #852: YouTube looked up without a JavaScript runtime: the menu says so with its
    /// qualities, as the Download video window does, with the help page's section; a look-up
    /// that left nothing to offer says it too.
    #[test]
    fn a_look_up_without_a_javascript_runtime_says_so_in_the_menu() {
        let mut l = Lookups::default();
        let now = Instant::now();
        let a = l
            .begin(&look(PAGE, None, None), &sent(false, "a=1"), false, now)
            .unwrap();
        let mut v = match mdm_tools::resolve::parse_looked(STREAM.as_bytes()).unwrap() {
            Looked::Video(v) => v,
            Looked::List(_) => unreachable!(),
        };
        v.needs_js_runtime = true;
        *lock(&a.found) = Found::of(Ok(Looked::Video(v.clone())), false);
        let ready = l.poll(&a.token, now);
        assert_eq!(ready.state, lookup::READY);
        let notice = ready.notice.expect("the notice");
        assert_eq!(notice.text, crate::videodlg::js_runtime_note());
        assert!(
            notice.text.contains("JavaScript runtime"),
            "{}",
            notice.text
        );
        assert!(!notice.link.is_empty());
        assert!(
            notice.url.starts_with("https://misterdownloadmanager.com/")
                && notice.url.ends_with("/help/video.html#javascript-runtime"),
            "{}",
            notice.url
        );
        // Nothing to offer at all: said with the reason.
        v.formats.clear();
        *lock(&a.found) = Found::of(Ok(Looked::Video(v)), false);
        let none = l.poll(&a.token, now);
        assert_eq!(
            (none.state.as_str(), none.reason.as_deref()),
            (lookup::FAILED, Some(lookup::NO_CHOICES))
        );
        assert!(none.notice.is_some());
    }
}
