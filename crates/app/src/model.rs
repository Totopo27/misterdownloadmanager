//! Download list items, IDM's default categories, and IDM-style formatting.

use std::path::{Path, PathBuf};
use std::time::Instant;

use mdm_engine::Download;
use mdm_engine::download::State;
use serde::{Deserialize, Serialize};
use url::Url;

/// IDM's default file categories (notes/research/idm-ui-and-integration.md, notes/UI.md), our
/// Torrents (D-021) and the user's own (D-021: Options → Save to → New, the Categories
/// right-click menu). Built-in ones are saved by name, so lists from before load unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    General,
    Compressed,
    Documents,
    Music,
    Programs,
    Video,
    /// Every torrent download (not by file type): its own folder, `Downloads\Torrents`.
    Torrents,
    /// The user's category with this id (`Settings::custom_categories`).
    Custom(u32),
}

/// A category the user made: its name, file types (`MP4 R0*`…, as Options lists them) and
/// folder (None: `Downloads\<name>`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomCategory {
    pub id: u32,
    pub name: String,
    pub types: String,
    pub folder: Option<String>,
}

/// The user's categories as the settings hold them, for code that only has a `Category` (its
/// label, its file types). Set at start and whenever they change (`set_custom`).
static CUSTOM: std::sync::RwLock<Vec<CustomCategory>> = std::sync::RwLock::new(Vec::new());

pub fn set_custom(list: &[CustomCategory]) {
    *CUSTOM.write().unwrap() = list.to_vec();
}

pub fn custom(id: u32) -> Option<CustomCategory> {
    CUSTOM.read().unwrap().iter().find(|c| c.id == id).cloned()
}

/// A built-in category the user changed (#93): another name, other file types, or deleted.
/// What it doesn't say stays as shipped (its folder lives in `Settings::category_dirs`, under
/// the built-in's `key`, as before).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinEdit {
    pub category: Category,
    /// The user's name for it, kept as typed (None: the shipped name, translated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its file types, as Options lists them (`MP3 FLAC`; None: the shipped ones).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<String>,
    /// Deleted: not in the Categories pane, Options or Download File Info, and no file goes to it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

/// The built-in categories' edits as the settings hold them; set at start and whenever they
/// change (`set_builtin`), like `CUSTOM`.
static BUILTIN: std::sync::RwLock<Vec<BuiltinEdit>> = std::sync::RwLock::new(Vec::new());

pub fn set_builtin(list: &[BuiltinEdit]) {
    *BUILTIN.write().unwrap() = list.to_vec();
}

fn builtin(cat: Category) -> Option<BuiltinEdit> {
    BUILTIN
        .read()
        .unwrap()
        .iter()
        .find(|e| e.category == cat)
        .cloned()
}

/// `types` (`MP3 *.flac, R0*`) as a list of lower-case patterns.
fn parse_types(types: &str) -> Vec<String> {
    types
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .map(|t| t.trim_start_matches("*.").trim_start_matches('.'))
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Does `ext` (lower case) match a file-type pattern (`mp4`, `r0*`)?
fn type_matches(pattern: &str, ext: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    match pattern.strip_suffix('*') {
        Some(prefix) => ext.starts_with(prefix),
        None => ext == pattern,
    }
}

impl Category {
    /// IDM's named categories, then Torrents.
    pub const BUILT_IN: [Category; 6] = [
        Category::Compressed,
        Category::Documents,
        Category::Music,
        Category::Programs,
        Category::Video,
        Category::Torrents,
    ];

    /// The named categories: the built-in ones not deleted, then the user's (as in the
    /// Categories tree).
    pub fn named() -> Vec<Category> {
        let custom = CUSTOM.read().unwrap();
        Category::BUILT_IN
            .into_iter()
            .filter(|c| !c.deleted())
            .chain(custom.iter().map(|c| Category::Custom(c.id)))
            .collect()
    }

    /// A built-in category the user deleted (#93).
    pub fn deleted(self) -> bool {
        !self.is_custom() && builtin(self).is_some_and(|e| e.deleted)
    }

    /// Can its name and file types be changed, and can it be deleted? The user's categories and
    /// the built-in ones by file type; not General (where everything else goes) nor Torrents
    /// (by source, not by file type).
    pub fn editable(self) -> bool {
        !matches!(self, Category::General | Category::Torrents)
    }

    /// The name settings and folders know it by, whatever the user renamed it to: a built-in
    /// one's English name (`Downloads\Music`, Options' stored folder), the user's for theirs.
    pub fn key(self) -> std::borrow::Cow<'static, str> {
        match self {
            Category::Custom(_) => self.label(),
            c => std::borrow::Cow::Borrowed(c.shipped_name()),
        }
    }

    /// A built-in category's name as shipped (English; shown translated).
    fn shipped_name(self) -> &'static str {
        match self {
            // i18n: category name: files of no particular kind
            Category::General => crate::i18n::mark("General"),
            // i18n: category name: compressed archives (zip, rar, 7z)
            Category::Compressed => crate::i18n::mark("Compressed"),
            // i18n: category name: documents (pdf, doc…)
            Category::Documents => crate::i18n::mark("Documents"),
            // i18n: category name: audio files
            Category::Music => crate::i18n::mark("Music"),
            // i18n: category name: programs and installers (exe, msi)
            Category::Programs => crate::i18n::mark("Programs"),
            // i18n: category name: video files
            Category::Video => crate::i18n::mark("Video"),
            // i18n: category name: torrent downloads (BitTorrent)
            Category::Torrents => crate::i18n::mark("Torrents"),
            Category::Custom(_) => "",
        }
    }

    /// The name as shown, in the interface language (the user's names as typed).
    pub fn shown(self) -> String {
        match self {
            Category::Custom(_) => self.label().into_owned(),
            c => match builtin(c).and_then(|e| e.name) {
                Some(name) => name,
                None => c.shipped_shown(),
            },
        }
    }

    /// A built-in category's shipped name in the interface language.
    pub fn shipped_shown(self) -> String {
        crate::i18n::t(self.shipped_name()).to_owned()
    }

    /// Every category: General first.
    pub fn all() -> Vec<Category> {
        std::iter::once(Category::General)
            .chain(Category::named())
            .collect()
    }

    /// The name: English for built-in ones as shipped (shown translated, `t`), the user's for
    /// theirs and for a built-in one they renamed. Settings and folders use [`Self::key`].
    pub fn label(self) -> std::borrow::Cow<'static, str> {
        use std::borrow::Cow::{Borrowed, Owned};
        match self {
            Category::Custom(id) => Owned(custom(id).map_or_else(|| "?".to_owned(), |c| c.name)),
            c => match builtin(c).and_then(|e| e.name) {
                Some(name) => Owned(name),
                None => Borrowed(c.shipped_name()),
            },
        }
    }

    /// The user's own category (editable, deletable)?
    pub fn is_custom(self) -> bool {
        matches!(self, Category::Custom(_))
    }

    fn extensions(self) -> Vec<String> {
        if let Some(types) = builtin(self).and_then(|e| e.types) {
            return parse_types(&types);
        }
        self.shipped_extensions()
    }

    /// A built-in category's file types as shipped (the user's: theirs).
    fn shipped_extensions(self) -> Vec<String> {
        let fixed: &[&str] = match self {
            Category::General | Category::Torrents => &[],
            Category::Compressed => &[
                "zip", "rar", "r0*", "r1*", "arj", "gz", "sit", "sitx", "sea", "ace", "bz2", "7z",
            ],
            Category::Documents => &["doc", "pdf", "ppt", "pps", "docx", "pptx"],
            Category::Music => &[
                "mp3", "wav", "wma", "mpa", "ram", "ra", "aac", "aif", "m4a", "tsa",
            ],
            Category::Programs => &["exe", "msi"],
            Category::Video => &[
                "avi", "mpg", "mpe", "mpeg", "asf", "wmv", "mov", "qt", "rm", "mp4", "flv", "m4v",
                "webm", "ogv", "ogg", "mkv", "ts", "tsv",
            ],
            Category::Custom(id) => {
                return custom(id)
                    .map(|c| parse_types(&c.types))
                    .unwrap_or_default();
            }
        };
        fixed.iter().map(|e| (*e).to_owned()).collect()
    }

    /// A built-in category's file types as shipped, as Options writes them (`ZIP RAR R0*`).
    pub fn shipped_types_text(self) -> String {
        upper_list(&self.shipped_extensions())
    }

    /// Options → Save to: "Automatically put in this category the following file types".
    pub fn types_text(self) -> String {
        match self {
            Category::General => {
                crate::i18n::t("The file types that are not listed in any other category").into()
            }
            Category::Torrents => {
                crate::i18n::t("Torrent downloads (magnet links and .torrent files)").into()
            }
            c => upper_list(&c.extensions()),
        }
    }

    /// The category a file name falls into: the user's categories first (that is why they made
    /// one), then IDM's; Torrents only by source, never by name.
    pub fn of(file_name: &str) -> Category {
        let Some((_, ext)) = file_name.rsplit_once('.') else {
            return Category::General;
        };
        let ext = ext.to_ascii_lowercase();
        let custom: Vec<Category> = CUSTOM
            .read()
            .unwrap()
            .iter()
            .map(|c| Category::Custom(c.id))
            .collect();
        // A deleted built-in takes no file: its types go to another category, or to General.
        custom
            .into_iter()
            .chain(Category::BUILT_IN.into_iter().filter(|c| !c.deleted()))
            .find(|c| c.extensions().iter().any(|p| type_matches(p, &ext)))
            .unwrap_or(Category::General)
    }

    /// Default folder: `Downloads\` for General, `Downloads\<Category>\` otherwise (a user's
    /// category: its own folder when it has one).
    pub fn folder(self, downloads: &Path) -> PathBuf {
        match self {
            Category::Custom(id) => match custom(id) {
                Some(CustomCategory {
                    folder: Some(f), ..
                }) => PathBuf::from(f),
                _ => self.default_folder(downloads),
            },
            c => c.default_folder(downloads),
        }
    }

    /// The folder it has when none was chosen: `Downloads\<name>` (General: `Downloads\`).
    pub fn default_folder(self, downloads: &Path) -> PathBuf {
        match self {
            Category::General => downloads.to_owned(),
            Category::Custom(id) => match custom(id) {
                Some(c) => downloads.join(mdm_engine::naming::sanitize(&c.name)),
                None => downloads.to_owned(),
            },
            // The shipped name, whatever it is called now: renaming doesn't move a folder.
            c => downloads.join(c.key().as_ref()),
        }
    }
}

/// `["zip", "r0*"]` as Options writes it: `ZIP R0*`.
fn upper_list(exts: &[String]) -> String {
    exts.iter()
        .map(|e| e.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The base folder that categories without their own folder live under (#556): General's
/// folder when one was chosen in Options → Save to, else `downloads`.
pub fn default_downloads<'a>(
    downloads: &'a Path,
    dirs: &'a [(String, String)],
) -> std::borrow::Cow<'a, Path> {
    let general = Category::General.key();
    dirs.iter()
        .find(|(c, _)| *c == *general)
        .map(|(_, d)| std::borrow::Cow::Owned(crate::dialogs::resolve_path(d)))
        .unwrap_or(std::borrow::Cow::Borrowed(downloads))
}

/// The category's folder: the one chosen in Options → Save to (or remembered from Download
/// File Info), else the default under [`default_downloads`] (#556).
pub fn category_folder(cat: Category, downloads: &Path, dirs: &[(String, String)]) -> PathBuf {
    let base = default_downloads(downloads, dirs);
    if cat.is_custom() {
        return cat.folder(&base);
    }
    let key = cat.key();
    dirs.iter()
        .find(|(c, _)| *c == *key)
        // A folder may be saved as shown, relative to the user's home.
        .map(|(_, d)| crate::dialogs::resolve_path(d))
        .unwrap_or_else(|| cat.folder(&base))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Added with "Download Later", never started.
    Queued,
    Downloading,
    /// Stopped by the user; resumable from its state file.
    Stopped,
    Complete,
    Error,
}

/// One row of the download list. The persisted part is everything but `live`.
#[derive(Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    pub url: Url,
    pub file_name: String,
    pub dest: PathBuf,
    pub size: Option<u64>,
    pub downloaded: u64,
    pub status: Status,
    pub category: Category,
    pub description: String,
    pub last_try: Option<String>,
    pub error: Option<String>,
    pub resumable: Option<bool>,
    /// The hash the finished file must have (Verify checksum…; beyond IDM, D-015).
    #[serde(default)]
    pub checksum: Option<mdm_engine::verify::Hash>,
    /// When the download was added ("Added" column), as `Last Try Date` is written. Downloads
    /// added before the column existed have none.
    #[serde(default)]
    pub added: Option<String>,
    /// Its own speed limit, bytes/s (progress window → Speed Limiter, "Remember Speed Limiter
    /// settings for this file on download stop/resume"): used again whenever it starts.
    #[serde(default)]
    pub own_limit: Option<u64>,
    /// A video from a page, downloaded by yt-dlp (its `url` is the page).
    #[serde(default)]
    pub video: Option<VideoSpec>,
    /// A torrent or magnet (M15): its `url` is a magnet link, `dest` its folder or one file.
    #[serde(default)]
    pub torrent: Option<crate::torrents::TorrentSpec>,
    /// An image gallery downloaded by gallery-dl (#58): its `url` is the gallery's page, `dest`
    /// the folder its files go into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gallery: Option<GallerySpec>,
    /// The event log: (unix seconds, what happened), oldest first, the last `LOG` entries.
    #[serde(default)]
    pub log: Vec<(i64, String)>,
    /// `file_name` is only the address's guess (nobody named it: no browser, no answer, no
    /// user): the download starts into its folder, under the name the server gives (its
    /// Content-Disposition), as a browser does (#28). Cleared once the file is made.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub name_from_url: bool,
    /// Its name was given without waiting for its folder to be looked at (Download File Info:
    /// a sleeping drive or a network folder can take seconds to answer, #743): nothing of its
    /// own is on the disk yet, and it starts under that name or the next free numbered one, as
    /// the engine finds as it starts (`Destination::Free`). Cleared once the file is made.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub number_at_start: bool,
    /// The Link grabber package it came in (#37): its archives are extracted together once
    /// every volume is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackageTag>,
    /// Where its server was, as its last run saw it (#140): Properties shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<crate::whereis::Seen>,
    /// Downloaded directly, never through a debrid account (#44): the user's choice (Download
    /// File Info, File Properties), or the browser had already started it (the host was sending
    /// the file). Kept, so a download goes the same way after a restart.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub direct: bool,
    /// A torrent through a debrid account (#44): its `url` is the magnet link, `dest` its folder
    /// (or its one file, once known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteSpec>,
    /// Safe deferred main name chosen in the progress window: applied in place once the
    /// download reaches `Status::Complete`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_on_complete: Option<String>,
    /// True when a folder download's destination already held files before MDM added it, so
    /// folder-level deferred rename refuses to sweep untracked files along.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preexisting_folder: bool,
    #[serde(skip)]
    pub live: Option<Live>,
    /// The session's temporary view while a torrent is being checked; never saved.
    #[serde(skip)]
    pub torrent_check: Option<(u64, u64)>,
    /// The engine's last view of this download (segments, connections).
    #[serde(skip)]
    pub snap: Option<mdm_engine::Snapshot>,
    /// "Use authorization" login; kept in memory only, never written to disk.
    #[serde(skip)]
    pub login: Option<(String, String)>,
    /// Request context from the browser (Referer, Cookie, User-Agent); memory only.
    #[serde(skip)]
    pub context: mdm_ipc::Command,
    /// What the Download complete window says besides (a video's subtitles the site refused,
    /// #68), in the interface language; memory only.
    #[serde(skip)]
    pub notices: Vec<String>,
    /// Finished video / subtitle / gallery paths waiting to be marked as downloaded from the
    /// web once any deferred rename in `App::poll` has moved them to their final names.
    #[serde(skip)]
    pub(crate) pending_mark: Option<PendingMark>,
    /// Files known to belong to a finished gallery or debrid folder download, kept across
    /// restarts so a requeued or resumed folder rename still checks for untracked files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) written_files: Option<Vec<PathBuf>>,
}

/// Files saved by `yt-dlp` or `gallery-dl` waiting for Mark-of-the-Web after deferred rename.
#[derive(Clone, Debug)]
pub(crate) struct PendingMark {
    pub main_file: Option<PathBuf>,
    pub side_files: Vec<PathBuf>,
    pub gallery_files: Vec<PathBuf>,
    pub url: String,
    pub referer: Option<String>,
}

/// A Link grabber package, as each of its downloads keeps it (#37).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageTag {
    /// The same for every download of the package.
    pub id: u64,
    pub name: String,
    /// Extract its archives when they are all downloaded.
    pub extract: bool,
    /// Then move them to the Recycle Bin (the Trash).
    pub delete_after: bool,
    /// The archives' password, typed in Link grabber: memory only, never written to disk.
    #[serde(skip)]
    pub password: Option<String>,
}

/// What runs a download: the engine (a file), yt-dlp (a video from a page), gallery-dl (an
/// image gallery, #58), or librqbit (a torrent).
pub enum Job {
    File(Download),
    Video {
        job: mdm_tools::video::VideoJob,
        url: Url,
    },
    Gallery {
        job: mdm_tools::gallery::GalleryJob,
        url: Url,
    },
    Torrent(crate::torrents::Run),
    /// A torrent through a debrid account, then its files over HTTP (#44).
    Remote(mdm_debrid::torrents::RemoteTorrent),
}

/// A torrent sent to a debrid account (#44).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSpec {
    /// The account it went to (Options → Accounts).
    pub account: u64,
    /// The service's id for it, once sent: a restart continues there, nothing is sent twice.
    #[serde(default)]
    pub transfer: Option<String>,
    /// The folder its files go into (a torrent of several files gets its own folder there).
    pub folder: PathBuf,
}

/// A gallery download's choice (#58).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GallerySpec {
    /// At most this many files (the first the site lists, on most the newest); none: all.
    #[serde(default)]
    pub limit: Option<u64>,
}

/// A helper's progress (yt-dlp's, gallery-dl's) as the engine's view: one segment when its size
/// is known, no connections, its notes as events.
fn tool_snapshot(p: mdm_tools::video::Progress, url: &Url) -> mdm_engine::Snapshot {
    use mdm_tools::video::State as V;
    let (state, file, error) = match p.state {
        V::Running => (State::Downloading, None, None),
        V::Done(f) => (State::Done, Some(f), None),
        V::Failed(e) => (State::Failed, None, Some(e)),
        V::Stopped => (State::Cancelled, None, None),
    };
    let size = p.total;
    mdm_engine::Snapshot {
        state,
        url: url.clone(),
        file,
        size,
        downloaded: p.downloaded,
        resumable: Some(true),
        segments: size
            .map(|t| {
                vec![mdm_engine::download::SegmentView {
                    start: 0,
                    cursor: p.downloaded.min(t),
                    end: t,
                }]
            })
            .unwrap_or_default(),
        connections: Vec::new(),
        error,
        failure: None,
        events: p
            .events
            .iter()
            .enumerate()
            .map(|(i, (at, text))| mdm_engine::download::Event {
                seq: i as u64 + 1,
                at: *at,
                text: text.clone(),
            })
            .collect(),
        // yt-dlp's and gallery-dl's connections are theirs: not seen here (#140).
        server: None,
        at_service: None,
    }
}

impl Job {
    /// The engine's view, or one made from yt-dlp's progress (one segment, no connections).
    pub fn snapshot(&self) -> mdm_engine::Snapshot {
        match self {
            Job::File(d) => d.snapshot(),
            Job::Torrent(t) => t.snapshot(),
            Job::Remote(r) => r.snapshot(),
            Job::Video { job, url } => tool_snapshot(job.progress(), url),
            Job::Gallery { job, url } => tool_snapshot(job.progress(), url),
        }
    }

    pub fn cancel(&self) {
        match self {
            Job::File(d) => d.cancel(),
            Job::Video { job, .. } => job.stop(),
            Job::Gallery { job, .. } => job.stop(),
            Job::Torrent(t) => t.cancel(),
            Job::Remote(r) => r.cancel(),
        }
    }

    /// The speed limit can change while a file downloads; yt-dlp takes it at the start; a
    /// torrent's applies to the whole torrent session.
    pub fn set_speed_limit(&self, rate: Option<u64>) {
        match self {
            Job::File(d) => d.set_speed_limit(rate),
            Job::Torrent(t) => t.set_speed_limit(rate),
            Job::Remote(r) => r.set_speed_limit(rate),
            Job::Video { .. } | Job::Gallery { .. } => {}
        }
    }

    pub fn speed_limit(&self) -> Option<u64> {
        match self {
            Job::File(d) => d.speed_limit(),
            Job::Torrent(t) => t.speed_limit(),
            Job::Remote(_) | Job::Video { .. } | Job::Gallery { .. } => None,
        }
    }
}

/// A subtitle language a video came without, in the interface language (#68).
pub fn sub_notice(p: &mdm_tools::video::SubProblem, site: &str) -> String {
    use mdm_tools::video::SubReason;
    let language = p.name.as_str();
    if p.beside {
        return crate::i18n::fmt(
            // i18n: after a video download: {language} = a language's name; its subtitles were to go inside the video, but came on a second try, so they're a separate file beside it
            crate::i18n::t(
                "Subtitles in {language} came on a second try: they're beside the video, not in it.",
            ),
            &[("language", language)],
        );
    }
    match &p.reason {
        SubReason::TooManyRequests => crate::i18n::fmt(
            // i18n: after a video download: {language} = a language's name, {site} = a web site's name (youtube.com); the site refused because it was asked too often
            crate::i18n::t(
                "Subtitles in {language} couldn't be downloaded: {site} refused (too many requests).",
            ),
            &[("language", language), ("site", site)],
        ),
        SubReason::Other(why) => crate::i18n::fmt(
            // i18n: after a video download: {language} = a language's name, {reason} = the helper program's own (English) words
            crate::i18n::t("Subtitles in {language} couldn't be downloaded: {reason}"),
            &[("language", language), ("reason", why)],
        ),
    }
}

/// A video download's choice (from "Download video from a page"): the page and yt-dlp's
/// format selector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoSpec {
    pub selector: String,
    /// The subtitles saved with it (#68; none in a list saved before them).
    #[serde(default)]
    pub subs: mdm_tools::video::Subs,
}

/// A running download and its speed measurement.
pub struct Live {
    pub dl: Job,
    pub rate: f64,
    sample: (Instant, u64),
    /// The engine's last event already copied into the item's log.
    seen: u64,
}

impl Live {
    pub fn new(dl: Job, downloaded: u64) -> Self {
        Live {
            dl,
            seen: 0,
            rate: 0.0,
            sample: (Instant::now(), downloaded),
        }
    }
}

/// Entries kept in an item's event log.
pub const LOG: usize = 50;

fn push_log(log: &mut Vec<(i64, String)>, at: i64, text: String) {
    if log.len() >= LOG {
        log.remove(0);
    }
    log.push((at, text));
}

/// The size a row shows after a snapshot: the server's when it said one, else what was known. A
/// download that ended without one is as long as what came down, not what the browser announced
/// for the file it was going to fetch (#374).
fn size_after(
    state: State,
    server: Option<u64>,
    downloaded: u64,
    known: Option<u64>,
) -> Option<u64> {
    match server {
        None if state == State::Done => Some(downloaded),
        server => server.or(known),
    }
}

impl Item {
    /// Add an app-side event (added, started, stopped …) to the log.
    pub fn note(&mut self, text: impl Into<String>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        push_log(&mut self.log, now, text.into());
    }

    /// The last `n` log entries as tooltip lines ("14:02:11  Connected to …").
    pub fn log_lines(&self, n: usize) -> String {
        self.log
            .iter()
            .rev()
            .take(n)
            .rev()
            .map(|(at, text)| {
                // In the interface's way (English month names in every language before).
                // With the offset of now (crate::clock, #536).
                let t = crate::clock::local_at(*at)
                    .map(|t| when_text(t.naive_local()))
                    .unwrap_or_default();
                format!("{t}  {text}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// What tells where this row's files are (its place, its kind, the files it wrote), for a
    /// look at the disk on a thread other than the window's (#743): no live download, no log.
    pub fn place_copy(&self) -> Item {
        let mut c = Item::new(self.id, self.url.clone(), self.dest.clone(), self.category);
        c.file_name.clone_from(&self.file_name);
        c.status = self.status;
        c.downloaded = self.downloaded;
        c.video.clone_from(&self.video);
        c.torrent.clone_from(&self.torrent);
        c.gallery.clone_from(&self.gallery);
        c.remote.clone_from(&self.remote);
        c.preexisting_folder = self.preexisting_folder;
        c.written_files.clone_from(&self.written_files);
        c
    }

    /// A new, not yet started download (queued state, nothing on disk).
    pub fn new(id: u64, url: Url, dest: PathBuf, category: Category) -> Self {
        Item {
            id,
            url,
            file_name: dest
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            dest,
            size: None,
            downloaded: 0,
            status: Status::Queued,
            category,
            description: String::new(),
            last_try: None,
            error: None,
            resumable: None,
            checksum: None,
            added: Some(now_stamp()),
            own_limit: None,
            video: None,
            torrent: None,
            gallery: None,
            log: Vec::new(),
            name_from_url: false,
            number_at_start: false,
            package: None,
            server: None,
            direct: false,
            remote: None,
            rename_on_complete: None,
            preexisting_folder: false,
            live: None,
            torrent_check: None,
            snap: None,
            login: None,
            context: Default::default(),
            notices: Vec::new(),
            pending_mark: None,
            written_files: None,
        }
    }
    /// This row's resume state is at its place. `downloaded` can't say: after a crash the list
    /// holds what it held at its last save, often nothing.
    pub fn owns_resume_state(&self) -> bool {
        let state = mdm_engine::state::state_path(&self.dest);
        mdm_engine::state::ResumeState::load(&state).is_ok_and(|s| s.url == self.url)
    }

    /// Whether this row has its place on the disk: running, part-way (its pieces and resume
    /// state are there), or finished with the file still there. A row not started yet holds
    /// nothing: several may wait under one name, and the first to start keeps it.
    pub fn holds_place(&self) -> bool {
        self.live.is_some()
            || if self.status == Status::Complete {
                self.dest.exists()
            } else {
                self.downloaded > 0 || self.owns_resume_state()
            }
    }

    /// Pull the engine's snapshot into the row. Returns true when the download just ended.
    pub fn poll(&mut self) -> bool {
        let Some(live) = &mut self.live else {
            return false;
        };
        let s = live.dl.snapshot();
        // The service's id for a torrent sent to it (#44), kept to continue after a restart.
        if let (Job::Remote(r), Some(spec)) = (&live.dl, self.remote.as_mut())
            && spec.transfer.is_none()
        {
            spec.transfer = r.transfer();
        }
        if let Some(server) = &s.server {
            self.server = Some(crate::whereis::Seen::from(server));
        }
        self.downloaded = s.downloaded;
        self.size = size_after(s.state, s.size, s.downloaded, self.size);
        self.resumable = s.resumable.or(self.resumable);
        if let Some(f) = &s.file {
            // Named now: a later start from scratch keeps this name, and its file is its own.
            let guessed = std::mem::take(&mut self.name_from_url);
            self.number_at_start = false;
            if *f != self.dest {
                // The file as saved (a video's real extension, a numbered duplicate, the name
                // the server gave).
                self.dest = f.clone();
                if let Some(name) = f.file_name() {
                    self.file_name = name.to_string_lossy().into_owned();
                }
                // The guess's category was the address's: the real name's now (the file stays
                // in the folder it was started in).
                if guessed {
                    self.category = Category::of(&self.file_name);
                }
            }
        }
        let dt = live.sample.0.elapsed().as_secs_f64();
        if dt >= 0.5 {
            let inst = s.downloaded.saturating_sub(live.sample.1) as f64 / dt;
            live.rate = if live.rate == 0.0 {
                inst
            } else {
                0.6 * live.rate + 0.4 * inst
            };
            live.sample = (Instant::now(), s.downloaded);
        }
        let seen = live.seen;
        for e in s.events.iter().filter(|e| e.seq > seen) {
            let at =
                e.at.duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64);
            live.seen = e.seq;
            push_log(&mut self.log, at, e.text.clone());
        }
        self.snap = Some(s.clone());
        let ended = match s.state {
            State::Probing | State::Downloading | State::Verifying | State::Completing => None,
            State::Done => Some(Status::Complete),
            State::Cancelled => Some(Status::Stopped),
            State::Failed => Some(Status::Error),
        };
        match ended {
            Some(status) => {
                self.status = status;
                // In the interface language; the engine's English when it gave no failure.
                self.error = s.failure.as_ref().map(crate::errors::describe).or(s.error);
                if status == Status::Complete {
                    self.downloaded = self.size.unwrap_or(self.downloaded);
                }
                // A video and its subtitles (#68), marked as downloaded from the web once
                // yt-dlp has saved them (and after any deferred rename in `App::poll`).
                let mut notices = Vec::new();
                if status == Status::Complete
                    && let Some(Live {
                        dl: Job::Video { job, url },
                        ..
                    }) = &self.live
                    && matches!(url.scheme(), "http" | "https")
                {
                    let done = job.progress();
                    // The subtitles the site wouldn't give: said in the log and the Download
                    // complete window (the video is saved all the same).
                    let site = url.host_str().unwrap_or("").trim_start_matches("www.");
                    notices = done
                        .sub_problems
                        .iter()
                        .map(|p| sub_notice(p, site))
                        .collect();
                    self.pending_mark = Some(PendingMark {
                        main_file: s.file.clone(),
                        side_files: done.side_files,
                        gallery_files: Vec::new(),
                        url: url.to_string(),
                        referer: self.context.referer.clone(),
                    });
                }
                // A gallery's files (#58), each marked as downloaded from the web after any
                // deferred folder rename in `App::poll`.
                if status == Status::Complete
                    && let Some(Live {
                        dl: Job::Gallery { job, url },
                        ..
                    }) = &self.live
                {
                    let known = job.known_files();
                    let list = self.written_files.get_or_insert_with(Vec::new);
                    for p in known {
                        if !list.contains(&p) {
                            list.push(p);
                        }
                    }
                    if matches!(url.scheme(), "http" | "https") {
                        self.pending_mark = Some(PendingMark {
                            main_file: None,
                            side_files: Vec::new(),
                            gallery_files: job.saved(),
                            url: url.to_string(),
                            referer: None,
                        });
                    }
                }
                if status == Status::Complete
                    && let Some(Live {
                        dl: Job::Remote(r), ..
                    }) = &self.live
                {
                    if r.preexisting_folder() {
                        self.preexisting_folder = true;
                    }
                    let saved = r.saved_files();
                    let list = self.written_files.get_or_insert_with(Vec::new);
                    for p in saved {
                        if !list.contains(&p) {
                            list.push(p);
                        }
                    }
                }
                for n in &notices {
                    self.note(n.clone());
                }
                self.notices = notices;
                self.live = None;
                // The session's last check numbers belong to the run just ended (a finished
                // torrent leaves the session): not left saying "Checking" until the next tick.
                self.torrent_check = None;
                true
            }
            None => false,
        }
    }

    /// Resume / Download Now may start it: not running, and not finished (a gallery finished
    /// may start again: it fetches the files not there yet, #58).
    pub fn can_start(&self) -> bool {
        self.live.is_none()
            && (matches!(
                self.status,
                Status::Queued | Status::Stopped | Status::Error
            ) || (self.status == Status::Complete && self.gallery.is_some()))
    }

    pub fn rate(&self) -> f64 {
        self.live.as_ref().map_or(0.0, |l| l.rate)
    }

    /// How much has come, 0 to 1, or None without a size to take a share of (a live stream,
    /// some HLS, a server that doesn't say): no percentage rather than 0 %. A video reads at
    /// most 99.9 % until the file is finished: another stream or the merge may still follow
    /// (#344), and the sizes beside it stay the real ones.
    pub fn fraction(&self) -> Option<f64> {
        let total = self.size.filter(|t| *t > 0)?;
        let f = (self.downloaded as f64 / total as f64).clamp(0.0, 1.0);
        Some(if self.video.is_some() && self.status != Status::Complete {
            f.min(mdm_tools::video::MAX_WHILE_RUNNING)
        } else {
            f
        })
    }

    /// The Status column: `13.38%`, `Complete`, `Error`, or blank.
    /// A torrent at its debrid service (#44), before its files come, or a file-host link whose file
    /// the service fetches first (TorBox): "45% at Real-Debrid" (the percentage first: the list's
    /// Status column is narrow).
    pub fn at_service(&self) -> Option<String> {
        let (service, progress) = match &self.live {
            Some(Live {
                dl: Job::Remote(r), ..
            }) => r.at_service().map(|(s, p)| (s.to_owned(), p))?,
            Some(_) => self.snap.as_ref()?.at_service.clone()?,
            None => return None,
        };
        let service = service.as_str();
        Some(crate::i18n::fmt(
            // i18n: a torrent's status while the user's debrid service downloads it, before MDM gets its files; {service} the service ("Real-Debrid"), {percent} how far it is ("45%")
            crate::i18n::t("{percent} at {service}"),
            &[
                ("service", service),
                (
                    "percent",
                    &progress.map_or_else(|| "...".to_owned(), |p| percent(p as f64)),
                ),
            ],
        ))
    }

    /// "Checking 42%" while a torrent's files are read and checked (added over files already
    /// there, Recheck): a running one from its live numbers (librqbit counts the bytes checked),
    /// one not running from the session's (`torrent_check`, every 2 s). Whole percents: the
    /// list's Status column is narrow.
    pub fn torrent_checking(&self) -> Option<String> {
        self.torrent.as_ref()?;
        let (checked, total) = match (&self.live, &self.snap) {
            (Some(_), Some(s)) if s.state == State::Verifying => {
                (s.downloaded, s.size.unwrap_or(0))
            }
            (Some(_), _) => return None,
            (None, _) => self.torrent_check?,
        };
        let ratio = if total > 0 {
            checked as f64 / total as f64
        } else {
            0.0
        };
        Some(crate::i18n::fmt(
            // i18n: a torrent's status while its files are read and checked; {percent} is how much is checked, e.g. "42%"
            crate::i18n::t("Checking {percent}"),
            &[("percent", &crate::i18n::percent(ratio.min(1.0), 0))],
        ))
    }

    pub fn status_text(&self) -> String {
        if let Some(at) = self.at_service() {
            return at;
        }
        if let Some(checking) = self.torrent_checking() {
            return checking;
        }
        if self
            .snap
            .as_ref()
            .is_some_and(|s| s.state == State::Verifying)
        {
            return crate::i18n::t("Verifying...").into();
        }
        match self.status {
            // i18n: a download's status (state it is in, not a command): finished
            Status::Complete => crate::i18n::t("Complete").into(),
            // i18n: a download's status (state it is in, not a command): it failed
            Status::Error => crate::i18n::t("Error").into(),
            // Where IDM leaves the cell blank (nothing yet, or no size to take a share of), it
            // says what the download is doing.
            // i18n: a download's status (state it is in, not a command): waiting in a queue for its turn
            Status::Queued if self.downloaded == 0 => crate::i18n::t("Waiting").into(),
            _ => match (self.fraction(), self.status) {
                (Some(f), _) => percent(f),
                // i18n: a download's status (state it is in, not a command): stopped by the user
                (_, Status::Stopped) => crate::i18n::t("Stopped").into(),
                (_, Status::Queued) => crate::i18n::t("Waiting").into(),
                // No size to take a share of: how much has come (it looked idle).
                (_, Status::Downloading) if self.downloaded > 0 => size_list(self.downloaded),
                _ => String::new(),
            },
        }
    }

    pub fn time_left(&self) -> String {
        match (self.status, self.size) {
            (Status::Downloading, Some(t)) if self.rate() > 1.0 => {
                time((t.saturating_sub(self.downloaded)) as f64 / self.rate())
            }
            _ => String::new(),
        }
    }
}

/// The "Last Try Date" and "Added" columns as stored, e.g. `Oct 28 21:44:12 2026` (the list's
/// format; read back by `stamp_time`).
pub fn now_stamp() -> String {
    crate::clock::now().format(STAMP).to_string()
}

const STAMP: &str = "%b %d %H:%M:%S %Y";

/// A stored stamp as a time (local), for sorting and for `date_text`.
pub fn stamp_time(stored: &str) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(stored.trim(), STAMP).ok()
}

/// A stored stamp as the list shows it: in the formats chosen in Options → Regional; else IDM's
/// own text in English (on the system's 12- or 24-hour clock), and the user's Windows region
/// format in other languages (its digits and order; month names would be in the region's
/// language).
pub fn date_text(stored: &str) -> String {
    let Some(t) = stamp_time(stored) else {
        return stored.to_owned();
    };
    when_text(t)
}

/// A local time as the interface writes dates and times (Options → Regional; IDM's text in
/// English, on the system's clock; the Windows region's elsewhere).
pub fn when_text(t: chrono::NaiveDateTime) -> String {
    let formats = crate::regional::current();
    if formats.custom {
        return crate::i18n::ltr(&crate::regional::short_date_time(t));
    }
    if english() {
        return stamp_text(t, formats.clock24);
    }
    #[cfg(windows)]
    if let Some(s) = crate::win32::local_date_time(t) {
        return crate::i18n::ltr(&s);
    }
    crate::i18n::ltr(&crate::regional::short_date_time(t))
}

/// IDM's text for a time, as stored (`Oct 28 21:44:12 2026`) on a 24-hour clock, and with the
/// system's 12-hour clock (#392) `Oct 28 9:44:12 PM 2026`.
fn stamp_text(t: chrono::NaiveDateTime, clock24: bool) -> String {
    use chrono::Timelike;
    format!(
        "{} {} {}",
        t.format("%b %d"),
        crate::regional::time_text(t.hour(), t.minute(), t.second(), clock24),
        t.format("%Y")
    )
}

/// Whether numbers are written IDM's way (English): `7.05 MB`, `13.38%`, `1 min 22 sec`.
/// Every other language writes them as its region does (digits, decimal mark, unit names; D-019).
fn english() -> bool {
    crate::i18n::code() == "en"
}

/// `13.38%` (IDM), or the interface language's percentage (`13,38 %`).
pub fn percent(ratio: f64) -> String {
    crate::i18n::percent(ratio, 2)
}

/// `{n} MB`-style unit text in the interface language (catalog context `unit`).
fn unit(n: f64, decimals: usize, template: &str) -> String {
    crate::i18n::i18n().measure(crate::i18n::lang(), n, decimals, template)
}

/// `7.05 MB`, `412.20 KB`, `1.97 GB` — IDM shows two decimals.
pub fn size(b: u64) -> String {
    if !english() {
        let v = b as f64;
        let k = 1024.0;
        return if v >= k * k * k {
            unit(v / (k * k * k), 2, "{n} GB")
        } else if v >= k * k {
            unit(v / (k * k), 2, "{n} MB")
        } else if v >= k {
            unit(v / k, 2, "{n} KB")
        } else {
            crate::i18n::tn(b, "{n} byte", "{n} bytes")
        };
    }
    let b = b as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB", b / (1024.0 * 1024.0 * 1024.0)) // not translated: English path
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.2} MB", b / (1024.0 * 1024.0)) // not translated: English path
    } else if b >= 1024.0 {
        format!("{:.2} KB", b / 1024.0) // not translated: English path
    } else {
        format!("{b} Bytes") // not translated: English path
    }
}

/// `200.000 MB` — the progress dialog uses three decimals (and KB below a kilobyte).
pub fn size3(b: u64) -> String {
    let v = b as f64;
    let k = 1024.0;
    if !english() {
        return if v >= k * k * k {
            unit(v / (k * k * k), 3, "{n} GB")
        } else if v >= k * k {
            unit(v / (k * k), 3, "{n} MB")
        } else {
            unit(v / k, 3, "{n} KB")
        };
    }
    if v >= k * k * k {
        format!("{:.3} GB", v / (k * k * k)) // not translated: English path
    } else if v >= k * k {
        format!("{:.3} MB", v / (k * k)) // not translated: English path
    } else {
        format!("{:.3} KB", v / k) // not translated: English path
    }
}

/// `11.623 MB/sec`, `512.000 KB/sec` (IDM's progress dialog uses three decimals).
pub fn rate(bps: f64) -> String {
    if !english() {
        return if bps >= 1024.0 * 1024.0 {
            unit(bps / (1024.0 * 1024.0), 3, "{n} MB/sec")
        } else {
            unit(bps / 1024.0, 3, "{n} KB/sec")
        };
    }
    if bps >= 1024.0 * 1024.0 {
        format!("{:.3} MB/sec", bps / (1024.0 * 1024.0)) // not translated: English path
    } else {
        format!("{:.3} KB/sec", bps / 1024.0) // not translated: English path
    }
}

/// The list's Size column: IDM puts two spaces before the unit (`200.00  MB`).
pub fn size_list(b: u64) -> String {
    if english() {
        size(b).replacen(' ', "  ", 1)
    } else {
        size(b)
    }
}

/// `14 sec`, `1 min 22 sec`, `2 hour(s) 5 min`.
pub fn time(secs: f64) -> String {
    let s = secs.round() as u64;
    if !english() {
        let n = crate::i18n::integer;
        let (h, m, sec) = (n(s / 3600), n((s % 3600) / 60), n(s % 60));
        let t = crate::i18n::t;
        return match s {
            0..60 => crate::i18n::fmt(t("{s} sec"), &[("s", &sec)]),
            60..3600 => crate::i18n::fmt(t("{m} min {s} sec"), &[("m", &m), ("s", &sec)]),
            _ => crate::i18n::fmt(t("{h} hour(s) {m} min"), &[("h", &h), ("m", &m)]),
        };
    }
    match s {
        0..60 => format!("{s} sec"), // not translated: English path
        60..3600 => format!("{} min {} sec", s / 60, s % 60), // not translated: English path
        _ => format!("{} hour(s) {} min", s / 3600, (s % 3600) / 60), // not translated: English path
    }
}

#[cfg(test)]
mod size_tests {
    use super::*;

    #[test]
    fn a_finished_download_is_as_long_as_what_came_down() {
        // The browser announced 178 MB; the server sent a 15 KB page and no size.
        assert_eq!(
            size_after(State::Done, None, 14473, Some(186662440)),
            Some(14473)
        );
        assert_eq!(
            size_after(State::Downloading, None, 14473, Some(186662440)),
            Some(186662440)
        );
        assert_eq!(size_after(State::Done, Some(100), 100, None), Some(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video_item(size: Option<u64>, downloaded: u64, status: Status) -> Item {
        let url = Url::parse("https://example.org/a.mp4").unwrap();
        let mut it = Item::new(1, url, PathBuf::from("a.mp4"), Category::Video);
        it.size = size;
        it.downloaded = downloaded;
        it.status = status;
        it.video = Some(VideoSpec {
            selector: "18".into(),
            subs: Default::default(),
        });
        it
    }

    /// Without a size there's no share to show: no 0 % (from #384).
    #[test]
    fn no_size_no_percentage() {
        let it = video_item(None, 5_000_000, Status::Downloading);
        assert_eq!(it.fraction(), None);
        assert_eq!(video_item(Some(0), 5, Status::Downloading).fraction(), None);
    }

    /// A video with all its bytes in may still be merging: 99.9 % until the file is finished,
    /// with the real sizes beside it.
    #[test]
    fn a_video_reads_100_only_when_finished() {
        let m = 1 << 20;
        let running = video_item(Some(100 * m), 100 * m, Status::Downloading);
        assert_eq!(running.fraction(), Some(0.999));
        let done = video_item(Some(100 * m), 100 * m, Status::Complete);
        assert_eq!(done.fraction(), Some(1.0));
        let mut file = video_item(Some(100), 100, Status::Downloading);
        file.video = None;
        assert_eq!(file.fraction(), Some(1.0));
    }

    #[test]
    fn a_folder_saved_as_shown_is_under_home() {
        let home = dirs::home_dir().unwrap();
        let dirs = vec![("Video".to_owned(), "Downloads/Video".to_owned())];
        let got = category_folder(Category::Video, Path::new("x"), &dirs);
        assert_eq!(got, home.join("Downloads/Video"));
        assert!(got.is_absolute());
        let whole = home.join("Elsewhere").display().to_string();
        let dirs = vec![("Video".to_owned(), whole.clone())];
        assert_eq!(
            category_folder(Category::Video, Path::new("x"), &dirs),
            PathBuf::from(whole)
        );
    }

    #[test]
    fn categories() {
        assert_eq!(Category::of("a.ZIP"), Category::Compressed);
        assert_eq!(Category::of("movie.part.r01"), Category::Compressed);
        assert_eq!(Category::of("setup.exe"), Category::Programs);
        assert_eq!(Category::of("song.mp3"), Category::Music);
        assert_eq!(Category::of("README"), Category::General);
        assert_eq!(Category::of("x.unknown"), Category::General);
    }

    #[test]
    fn formatting() {
        assert_eq!(size(7_392_461), "7.05 MB");
        assert_eq!(size(500), "500 Bytes");
        assert_eq!(time(14.2), "14 sec");
        assert_eq!(time(82.0), "1 min 22 sec");
        assert_eq!(time(7500.0), "2 hour(s) 5 min");
        assert_eq!(rate(12_187_628.0), "11.623 MB/sec");
    }
}

#[cfg(test)]
mod stamp_tests {
    #[test]
    fn stamps_read_back_and_sort_by_time() {
        let apr = super::stamp_time("Apr 02 08:00:00 2027").unwrap();
        let oct = super::stamp_time("Oct 28 21:44:12 2026").unwrap();
        assert!(oct < apr, "October 2026 comes before April 2027");
        assert!(super::stamp_time("not a date").is_none());
    }

    #[test]
    fn english_stamps_follow_the_clock() {
        let at = |s| super::stamp_time(s).unwrap();
        // 24-hour: exactly as stored.
        for s in ["Oct 28 21:44:12 2026", "Jan 01 00:05:09 2027"] {
            assert_eq!(super::stamp_text(at(s), true), s);
        }
        // 12-hour (#392): midnight and noon are 12.
        assert_eq!(
            super::stamp_text(at("Oct 28 21:44:12 2026"), false),
            "Oct 28 9:44:12 PM 2026"
        );
        assert_eq!(
            super::stamp_text(at("Jan 01 00:05:09 2027"), false),
            "Jan 01 12:05:09 AM 2027"
        );
        assert_eq!(
            super::stamp_text(at("Jun 15 12:00:00 2027"), false),
            "Jun 15 12:00:00 PM 2027"
        );
    }
}

/// An address as typed: `www.example.com/a.zip` (no scheme, as browsers take it) is read as
/// https; anything else as written.
pub fn parse_address(typed: &str) -> Result<url::Url, url::ParseError> {
    let typed = typed.trim();
    match url::Url::parse(typed) {
        Err(url::ParseError::RelativeUrlWithoutBase)
            if !typed.contains(char::is_whitespace)
                && typed
                    .split('/')
                    .next()
                    .is_some_and(|host| host.contains('.')) =>
        {
            url::Url::parse(&format!("https://{typed}"))
        }
        other => other,
    }
}

#[cfg(test)]
mod address_tests {
    #[test]
    fn an_address_without_its_scheme_is_https() {
        let u = super::parse_address(" www.example.com/a.zip ").unwrap();
        assert_eq!(u.as_str(), "https://www.example.com/a.zip");
        assert_eq!(super::parse_address("ftp://h/f").unwrap().scheme(), "ftp");
        assert!(super::parse_address("not an address").is_err());
    }
}

/// A finished download whose file was moved or deleted (#342). Not when its folder is gone too:
/// that is more likely a drive that is unplugged, and the file may come back.
pub fn finished_file_gone(status: Status, dest: &std::path::Path) -> bool {
    status == Status::Complete && !dest.exists() && dest.parent().is_some_and(|d| d.is_dir())
}

#[cfg(test)]
mod finished_file_gone_tests {
    use super::*;

    #[test]
    fn only_a_finished_file_gone_from_a_folder_that_is_there() {
        let dir = std::env::temp_dir().join(format!("mdm-342-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let there = dir.join("there.bin");
        std::fs::write(&there, b"x").unwrap();
        assert!(!finished_file_gone(Status::Complete, &there));
        assert!(finished_file_gone(Status::Complete, &dir.join("gone.bin")));
        // Not finished: its file isn't expected yet.
        assert!(!finished_file_gone(Status::Queued, &dir.join("gone.bin")));
        // Its folder is gone too: maybe a drive that is unplugged.
        assert!(!finished_file_gone(
            Status::Complete,
            &dir.join("no").join("gone.bin")
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
