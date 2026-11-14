//! The main window (notes/ui-spec/main-window.md): menu bar, toolbar, Categories tree and the
//! download list at IDM 6.42's measured positions, plus the download actions behind them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine as _;
use egui::{Pos2, Ui, pos2};
use mdm_engine::state::state_path;
use mdm_engine::{Destination, Download, Options, Request};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

use crate::chrome::{
    self, IconKey, MenuBarEvent, MenuEntry, MenuKind, PopupResult, ShellIcons, ToolbarEvent,
    TreeEvent, TreeRow,
};
use crate::dialogs::{self, AddUrl, Complete, FileInfo, Outcome, Start};
use crate::icons::Icon;
use crate::list;
use crate::model::{self, Category, Item, Live, Status};
use crate::progress::{self, Progress};
use crate::queue::{self, Member, Queue};
use crate::settings::Settings;
use crate::store;
use crate::winui;
use mdm_ipc as ipc;

/// A node of the Categories tree, what the list shows (saved: the last one comes back).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum View {
    All,
    Cat(Category),
    Unfinished(Option<Category>),
    Finished(Option<Category>),
    /// A queue (index into `App::queues`).
    Queue(usize),
}

/// What was being added when its address turned out to be in the list already.
enum Pending {
    /// From the browser or the command line.
    Command(Box<ipc::Command>),
    /// From Add URL.
    Url(dialogs::NewUrl),
    /// A video from Download video from a page, or the browser's quality menu (`true`), with
    /// the menu's look-up: it goes with the new download only, never to the one already there.
    Video(Box<crate::videodlg::VideoAdd>, bool),
}

/// What Options → Downloads → "If a duplicate download link is added" (0 ask, 1 numbered name,
/// 2 overwrite, 3 open or resume) makes of a new download ([`duplicate_step`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DupStep {
    /// Added: no duplicate, or added again under a numbered name.
    Add,
    /// Added once the one already there has gone, its files and its row: Options' overwrite
    /// (`true`: a video's yt-dlp files too, [`replaced_files`]), or its finished file is gone
    /// (#342, nothing to duplicate; `false`: its row, state and part file only, as for any file).
    Replace(u64, bool),
    /// Not added: the one already there is opened (finished) or resumed.
    Existing(u64),
    /// Not added yet: the user is asked (dialogs::Duplicate).
    Ask(u64),
}

/// [`DupStep`] for Options' `action` when `found` is the download already there and whether its
/// finished file is gone (#342).
fn duplicate_step(action: u8, found: Option<(u64, bool)>) -> DupStep {
    match found {
        None => DupStep::Add,
        Some((dup, true)) => DupStep::Replace(dup, false),
        Some((dup, false)) => match action {
            1 => DupStep::Add,
            2 => DupStep::Replace(dup, true),
            3 => DupStep::Existing(dup),
            _ => DupStep::Ask(dup),
        },
    }
}

/// A video already in the list as `page` in quality `selector` (another quality of the same page
/// is another download, not a duplicate).
fn same_video(item: &Item, page: &url::Url, selector: &str) -> bool {
    item.url == *page && item.video.as_ref().is_some_and(|v| v.selector == selector)
}

/// Where a video's choice goes after the duplicate check: added now, kept for the user's
/// answer, or (the one already there opened or resumed) dropped with the menu's look-up, which
/// only ever goes with the download born from its own choice.
enum VideoRoute {
    Add(Box<crate::videodlg::VideoAdd>),
    Ask(u64, Pending),
    Dropped,
}

fn video_route(step: DupStep, add: crate::videodlg::VideoAdd, from_menu: bool) -> VideoRoute {
    match step {
        DupStep::Add | DupStep::Replace(..) => VideoRoute::Add(Box::new(add)),
        DupStep::Ask(dup) => VideoRoute::Ask(dup, Pending::Video(Box::new(add), from_menu)),
        DupStep::Existing(_) => VideoRoute::Dropped,
    }
}

/// What the duplicate question's answer does ([`answered`]).
enum Answer {
    /// Open or resume the one already there (3): what was being added is dropped, a video's
    /// look-up with it.
    Existing(u64),
    /// Add it: under a numbered name (1), or over the one there (2, `Some`).
    Add(Option<u64>, Pending),
    /// The one asked about has left the list meanwhile: checked afresh, as if just added.
    Again(Pending),
}

/// The duplicate question about `dup` (`there`: still in the list) answered `choice`, numbered
/// as Options lists them.
fn answered(dup: u64, there: bool, choice: usize, pending: Pending) -> Answer {
    match choice {
        _ if !there => Answer::Again(pending),
        3 => Answer::Existing(dup),
        2 => Answer::Add(Some(dup), pending),
        _ => Answer::Add(None, pending),
    }
}

/// The menu's look-up for a video being added: it goes with a download started now only, and
/// only while fresh ([`crate::videolookup::TTL`], as at its start): a question answered long
/// after, or a download queued, looks the page up afresh.
fn kept_lookup(
    preloaded: Option<mdm_tools::video::Preloaded>,
    goes: VideoGoes,
    now: Instant,
) -> Option<mdm_tools::video::Preloaded> {
    preloaded.filter(|p| {
        goes == VideoGoes::Start
            && now.saturating_duration_since(p.slot.born()) < crate::videolookup::TTL
    })
}

/// What replacing download `id` takes from the disk; None when it isn't in `items` or runs (it
/// is left alone, the new copy then gets a numbered name). Its finished file, state and part
/// file, as for any download; with `yt_dlp` (Options' overwrite, chosen: never #342's automatic
/// one), for a video also what yt-dlp left for it (`mdm_tools::video::own_files`). Never a file
/// another download may own: its file, or one named after its name or stem and a dot (its part
/// files, running or paused) in that folder, letter case aside.
fn replaced_files(items: &[Item], id: u64, yt_dlp: bool) -> Option<Vec<PathBuf>> {
    let item = items.iter().find(|i| i.id == id)?;
    if item.live.is_some() {
        return None;
    }
    let low = |p: &std::path::Path| p.to_string_lossy().to_lowercase();
    let others: Vec<&Item> = items.iter().filter(|i| i.id != id).collect();
    let dest = &item.dest;
    let mut files = Vec::new();
    if item.status == Status::Complete {
        files.push(dest.clone());
    }
    files.push(state_path(dest));
    files.push(mdm_engine::naming::part_path(dest));
    if yt_dlp {
        for f in video_leftovers(items, id, true) {
            if !files.contains(&f) {
                files.push(f);
            }
        }
    }
    files.retain(|f| !others.iter().any(|o| low(&o.dest) == low(f)));
    Some(files)
}

/// What yt-dlp left beside video download `id`'s file (`mdm_tools::video::own_files`): the
/// streams of an unfinished run or a failed merge, their part files, the merge's file; with
/// `outputs`, also the finished file and its subtitles. Never a file another download in that
/// folder may own: one named after its name or stem and a dot, letter case aside. Nothing for a
/// download that isn't a video.
fn video_leftovers(items: &[Item], id: u64, outputs: bool) -> Vec<PathBuf> {
    let low = |p: &std::path::Path| p.to_string_lossy().to_lowercase();
    let Some(item) = items.iter().find(|i| i.id == id && i.video.is_some()) else {
        return Vec::new();
    };
    let (Some(dir), Some(stem)) = (item.dest.parent(), item.dest.file_stem()) else {
        return Vec::new();
    };
    let folder = low(dir);
    let others: Vec<&Item> = items.iter().filter(|i| i.id != id).collect();
    let named: Vec<String> = others
        .iter()
        .filter(|o| o.dest.parent().is_some_and(|p| low(p) == folder))
        .flat_map(|o| [o.dest.file_name(), o.dest.file_stem()])
        .flatten()
        .map(|n| format!("{}.", n.to_string_lossy().to_lowercase()))
        .collect();
    // The parts its failed merge kept, by name (yt-dlp names some, `Talk.faud-English.mp4`, as
    // no other leftover is named), while still beside its file.
    let kept = item.video.iter().flat_map(|v| &v.parts).filter(|p| {
        p.parent().is_some_and(|p| low(p) == folder)
            && p.file_name().is_some_and(|n| {
                n.to_string_lossy()
                    .to_lowercase()
                    .starts_with(&format!("{}.", stem.to_string_lossy().to_lowercase()))
            })
    });
    let mut found = mdm_tools::video::own_files(dir, &stem.to_string_lossy(), outputs);
    for p in kept {
        if p.is_file() && !found.iter().any(|f| low(f) == low(p)) {
            found.push(p.clone());
        }
    }
    found
        .into_iter()
        .filter(|f| {
            let name = f
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            !named.iter().any(|n| name.starts_with(n))
                && !others.iter().any(|o| low(&o.dest) == low(f))
        })
        .collect()
}

/// The quality menu's answer for its choice after the duplicate check: a new download started or
/// queued; else `sent` (the one already there was opened or resumed, or MDM asks what to do in its
/// Duplicate window), which the extension shows as sent to MDM.
fn menu_answer(step: DupStep, start_now: bool) -> &'static str {
    use ipc::api::lookup;
    match step {
        DupStep::Add | DupStep::Replace(..) if start_now => lookup::STARTED,
        DupStep::Add | DupStep::Replace(..) => lookup::QUEUED,
        DupStep::Existing(_) | DupStep::Ask(_) => lookup::SENT,
    }
}

/// Menu, toolbar and context-menu commands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cmd {
    AddUrl,
    /// Help → a page of the website (`""` = home).
    HelpPage(&'static str),
    Shortcuts,
    ReportProblem,
    CheckUpdates,
    About,
    TellFriend,
    /// Help → Support MDM: the creator's GitHub Sponsors page in the browser.
    Support,
    Exit,
    Stop,
    Remove,
    /// Shift+Delete: Remove with "Also delete the files from disk" ticked.
    RemoveWithFiles,
    Resume,
    StopAll,
    DeleteCompleted,
    /// Start / stop a queue (index into `App::queues`).
    StartQueue(usize),
    StopQueue(usize),
    /// Queue right-click → Edit queue (its files) / Schedule; Delete; Create new queue.
    /// Tasks → Show drop target (shown / hidden); the drop target's Show main window.
    ToggleDropTarget,
    ShowMain,
    /// Tasks → Export / Import (D-021): IDM's export file (.ef2) or a text file.
    Export {
        ef2: bool,
    },
    Import {
        ef2: bool,
    },
    /// Download again from the start (File menu, right-click), after asking.
    Redownload,
    /// Move the file or rename it (right-click, Ctrl+M).
    MoveRename,
    /// Categories right-click (D-021): Add category, Properties, Delete (the user's only).
    AddCategory,
    CategoryProps(Category),
    DeleteCategory(Category),
    EditQueue(usize),
    ScheduleQueue(usize),
    DeleteQueue(usize),
    NewQueue,
    /// Categories right-click → Browse: open the category's folder.
    Browse(Category),
    /// List right-click → On Double click → Open (true) / Properties.
    DoubleClickOpen(bool),
    /// Right-click → Add to queue → queue index; Delete from queue.
    AddToQueue(usize),
    RemoveFromQueue,
    ToggleCategories,
    /// View → Dark Mode support.
    ToggleDarkMode,
    /// View → Always dark (#60).
    ToggleAlwaysDark,
    /// View → Theme → a theme (#45): a built-in's id or a theme file's name.
    Theme(&'static str),
    /// View → Theme → Open themes folder.
    OpenThemesFolder,
    /// View → Theme → Add a theme file: a theme or a Base16 or pywal colour scheme, copied
    /// into the themes folder and picked.
    AddThemeFile,
    /// View → Draw with → OpenGL / Direct3D, Vulkan or Metal (#89, #206), from the next start.
    #[cfg(gpu)]
    DrawWith(crate::renderer::Api),
    /// View → Interface size → a size in percent (#262).
    InterfaceSize(u16),
    /// View → Text → Smooth (`true`) or Classic (Windows).
    #[cfg(windows)]
    SmoothText(bool),
    /// View → Language: a catalog's code, or follow Windows (`None`).
    Language(Option<&'static str>),
    /// View → Language...: the language picker.
    ChooseLanguage,
    Open,
    OpenWith,
    OpenFolder,
    Properties,
    /// Show or hide a list column (data slot).
    ToggleColumn(usize),
    /// Downloads → Speed Limiter.
    LimiterOn,
    LimiterOff,
    LimiterSettings,
    /// Downloads → Scheduler (also the toolbar button).
    Scheduler,
    /// Downloads → Options (also the toolbar button).
    Options,
    /// Tasks → Add batch download: an address with a `*` over a range.
    Batch,
    /// Tasks → Add batch download from clipboard (#709): the copied text's links in Link
    /// grabber, or a copied numbered address in Batch download.
    BatchClipboard,
    /// Tasks → Download video from a page… (yt-dlp, M9).
    VideoPage,
    /// Add torrent → Add magnet link… (M15): the Add torrent dialog, empty or with the
    /// clipboard's magnet.
    AddMagnet,
    /// Add torrent → Open torrent file…: pick a .torrent, then the Add torrent dialog.
    OpenTorrentFile,
    /// Add torrent → Torrent settings… (sharing after finishing, Windows associations).
    TorrentSettings,
    /// Right-click → Torrent (#39): read every piece again; ask the trackers again; add
    /// trackers (a window); copy its magnet link.
    TorrentRecheck,
    TorrentReannounce,
    /// Right-click → Torrent → Download in order, and its first and last pieces first (#39).
    TorrentInOrder,
    TorrentEndsFirst,
    TorrentAddTrackers,
    TorrentFiles,
    TorrentCopyMagnet,
    /// Tasks → RSS feeds… (also the Add Torrent button's arrow), #39: true opens it on its
    /// Download rules tab.
    RssFeeds(bool),
    /// Right-click → Refresh download address: open the source page, edit the address.
    RefreshAddress,
    /// Right-click → Verify checksum… (beyond IDM).
    VerifyChecksum,
    /// Right-click → Extract (beyond IDM, #37): a finished archive, into its folder.
    Extract,
    /// Tasks → Link grabber… (#37): the window, empty (links pasted in) or as it is.
    LinkGrabber,
    /// Downloads → Find (Ctrl-F) / Find Next (F3).
    Find,
    FindNext,
    /// View → Arrange files: sort by a column (data slot), or by order of addition.
    Arrange(Option<usize>),
    /// Header right-click → Clear all filters.
    ClearFilters,
    /// Every download the list shows (Ctrl+A).
    SelectAll,
    /// The All Downloads view (an empty list's link).
    ShowAll,
    /// Header right-click → Reset columns.
    ResetColumns,
    None,
}

/// What an RSS fetch or look-up sends back to the app (`App::rss_tick`).
enum RssEvent {
    Fetched(u64, Result<crate::rss::Parsed, String>),
    /// An article a rule took, looked up: what its torrent holds.
    Looked {
        rule: u64,
        feed: u64,
        title: String,
        res: Result<mdm_torrent::Meta, String>,
    },
}

/// A download's standing (a gone one counts as complete: nothing to start).
fn member_in(members: &std::collections::HashMap<u64, Member>, id: u64) -> Member {
    members.get(&id).copied().unwrap_or(Member::Complete)
}

/// A file move's answer, from its thread.
type MoveDone = std::sync::mpsc::Receiver<Result<(), String>>;

/// Move a file: a rename, else a copy and the original deleted (a copy whose original can't go,
/// in use, is taken back: the file stays in one place).
fn move_file(from: &std::path::Path, to: &std::path::Path) -> Result<(), String> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(|e| e.to_string())?;
    std::fs::remove_file(from).map_err(|e| {
        let _ = std::fs::remove_file(to);
        e.to_string()
    })
}

/// Whether Move/Rename can take a download now: a finished file, or nothing of it come in yet.
fn can_move(i: &Item) -> bool {
    i.live.is_none() && (i.status == Status::Complete || i.downloaded == 0)
}

/// What is said when a download can't move, or stays while the others of a selection move.
fn cant_move_notice() -> (&'static str, String) {
    (
        crate::i18n::mark("Move/Rename"),
        crate::i18n::t(
            "Only a finished download, or one not started yet, can be moved. Stop it and finish it first.",
        )
        .to_owned(),
    )
}

/// Where one download of a selection goes in `dir`: a name free there (on disk, what
/// `naming::destination` checks) and not one the selection has taken (two not started under one
/// name have no file yet to be seen).
fn place_in(
    dir: &std::path::Path,
    name: &str,
    taken: &mut std::collections::HashSet<PathBuf>,
) -> PathBuf {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut at = mdm_engine::naming::destination(dir, name);
    let mut n = 1u32;
    while !taken.insert(at.clone()) {
        at = mdm_engine::naming::destination(dir, &format!("{stem} ({n}){ext}"));
        n += 1;
    }
    at
}

/// A download's new place (its name from it). A name the user gave is no longer the address's
/// guess (`Item::name_from_url`); another folder alone keeps the server's say.
fn take_place(item: &mut Item, target: PathBuf) {
    let old_dest = item.dest.clone();
    if let Some(name) = target.file_name() {
        let name = name.to_string_lossy().into_owned();
        if name != item.file_name {
            item.name_from_url = false;
        }
        item.file_name = name;
    }
    if let Some(spec) = item.torrent.as_mut()
        && spec.folder == old_dest
    {
        spec.folder = target.clone();
    }
    if let Some(files) = item.written_files.as_mut() {
        for p in files {
            if let Ok(rel) = p.strip_prefix(&old_dest) {
                *p = target.join(rel);
            }
        }
    }
    item.dest = target;
    item.moved_away = false;
}

/// Update `item.pending_mark` after `from` has been renamed to `actual_to` (including renaming
/// `yt-dlp` sidecar subtitle files alongside a video, or rewriting saved file paths inside a
/// renamed gallery folder) so `flush_pending_mark` marks the final paths on disk.
fn apply_renamed_paths_to_pending_mark(
    item: &mut Item,
    from: &std::path::Path,
    actual_to: &std::path::Path,
    is_dir: bool,
) {
    let Some(pm) = item.pending_mark.as_mut() else {
        return;
    };
    if is_dir {
        for gf in &mut pm.gallery_files {
            if let Ok(rel) = gf.strip_prefix(from) {
                *gf = actual_to.join(rel);
            }
        }
    } else {
        if pm.main_file.is_some() {
            pm.main_file = Some(actual_to.to_path_buf());
        }
        if !pm.side_files.is_empty() {
            pm.side_files =
                crate::deferred_rename::rename_video_side_files(from, actual_to, &pm.side_files);
        }
    }
}

/// Whether `folder` is a root/category/save folder or contains another download's files, making a
/// folder-level deferred rename unsafe for `id`.
fn is_protected_or_shared_folder_with(
    id: u64,
    folder: &std::path::Path,
    downloads: &std::path::Path,
    settings: &Settings,
    rss: &crate::rss::Store,
    items: &[Item],
) -> bool {
    use crate::deferred_rename::{path_is_at_or_inside_dir, paths_refer_to_same_dir};

    if paths_refer_to_same_dir(folder, downloads)
        || paths_refer_to_same_dir(
            folder,
            &settings.category_dir(model::Category::General, downloads),
        )
    {
        return true;
    }
    for cat in model::Category::BUILT_IN {
        if paths_refer_to_same_dir(folder, &settings.category_dir(cat, downloads)) {
            return true;
        }
    }
    for (_, dir) in &settings.category_dirs {
        if !dir.is_empty() && paths_refer_to_same_dir(folder, std::path::Path::new(dir)) {
            return true;
        }
    }
    for c in &settings.custom_categories {
        if paths_refer_to_same_dir(
            folder,
            &settings.category_dir(model::Category::Custom(c.id), downloads),
        ) {
            return true;
        }
        if let Some(dir) = c.folder.as_deref().filter(|d| !d.is_empty())
            && paths_refer_to_same_dir(folder, std::path::Path::new(dir))
        {
            return true;
        }
    }
    for dir in &settings.folder_history {
        if !dir.is_empty() && paths_refer_to_same_dir(folder, std::path::Path::new(dir)) {
            return true;
        }
    }
    for rule in &rss.rules {
        if !rule.folder.is_empty()
            && paths_refer_to_same_dir(folder, std::path::Path::new(&rule.folder))
        {
            return true;
        }
    }
    for sys in [
        dirs::home_dir(),
        dirs::desktop_dir(),
        dirs::download_dir(),
        dirs::document_dir(),
    ]
    .into_iter()
    .flatten()
    {
        if paths_refer_to_same_dir(folder, &sys) {
            return true;
        }
    }
    items.iter().filter(|o| o.id != id).any(|other| {
        path_is_at_or_inside_dir(&other.dest, folder)
            || other
                .torrent
                .as_ref()
                .is_some_and(|t| path_is_at_or_inside_dir(&t.folder, folder))
            || other
                .remote
                .as_ref()
                .is_some_and(|r| path_is_at_or_inside_dir(&r.folder, folder))
    })
}

/// Whether a folder download's destination holds files that this download did not write.
fn folder_has_untracked_item_files(item: &Item) -> bool {
    if item.preexisting_folder {
        return true;
    }
    if let Some(files) = &item.written_files {
        return crate::deferred_rename::folder_has_untracked_files(&item.dest, files);
    }
    if let Some(spec) = &item.torrent
        && spec.folder == item.dest
        && item.dest.is_dir()
    {
        let manifest = crate::torrents::torrent_file(&spec.hash)
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| mdm_torrent::files_from_bytes(&b).ok());
        if let Some(entries) = manifest {
            let allowed: Vec<PathBuf> = entries
                .into_iter()
                .enumerate()
                .filter(|(i, f)| {
                    f.padding || spec.files.as_ref().is_none_or(|chosen| chosen.contains(i))
                })
                .map(|(_, f)| item.dest.join(f.path))
                .collect();
            return crate::deferred_rename::folder_has_untracked_files(&item.dest, &allowed);
        }
    }
    false
}

/// Translated reason key explaining why deferred rename is unavailable for `item`, or `None`
/// when `item` can be renamed safely on completion.
fn deferred_rename_disabled_reason_with(
    item: &Item,
    downloads: &std::path::Path,
    settings: &Settings,
    rss: &crate::rss::Store,
    items: &[Item],
) -> Option<&'static str> {
    if crate::deferred_rename::is_folder_target(item) {
        if is_protected_or_shared_folder_with(item.id, &item.dest, downloads, settings, rss, items)
        {
            // i18n: tooltip on the disabled Rename box when the folder holds another download or is a save/category folder
            return Some(crate::i18n::mark(
                "This folder is shared with another download or save folder",
            ));
        }
        if folder_has_untracked_item_files(item) {
            // i18n: tooltip on the disabled Rename box when the folder already holds files this download did not write
            return Some(crate::i18n::mark(
                "This folder holds files from outside this download",
            ));
        }
        return None;
    }
    if crate::deferred_rename::is_split_archive_volume(item, items) {
        // i18n: tooltip on the disabled Rename box when the file is one part of a split archive (.part1.rar, .7z.001…)
        return Some(crate::i18n::mark(
            "Split archive volumes keep their names for extraction",
        ));
    }
    if item.torrent.is_some() {
        // i18n: tooltip on the disabled Rename box for a single-file torrent
        return Some(crate::i18n::mark("A single-file torrent keeps its name"));
    }
    None
}

/// The place a download about to start takes instead of its own, when its own belongs to
/// another row (#442): two can wait under one name (nothing was on the disk when each was added,
/// and they can start in the same moment, before the engine has reserved anything). None: it
/// stays. A row with data of its own never moves, so a resume finds its pieces.
fn place_to_take(item: &Item, items: &[Item]) -> Option<PathBuf> {
    if item.downloaded > 0 || item.owns_resume_state() {
        return None;
    }
    let dir = item.dest.parent()?;
    let others = || {
        items
            .iter()
            .filter(|o| o.id != item.id && o.dest.parent() == Some(dir))
    };
    let lower = |p: &std::path::Path| p.to_string_lossy().to_lowercase();
    let stem = |o: &Item| {
        o.dest
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
    };
    if item.video.is_some() {
        // yt-dlp takes any `<stem>.*` there as its own (#32): the stem has to be free.
        others()
            .any(|o| stem(o) == stem(item) && o.holds_place())
            .then(|| {
                mdm_engine::naming::stem_destination(dir, &item.file_name, |s| {
                    let s = s.to_lowercase();
                    others().any(|o| stem(o).as_ref() == Some(&s) && o.holds_place())
                })
            })
    } else {
        others()
            .any(|o| lower(&o.dest) == lower(&item.dest) && o.holds_place())
            .then(|| {
                let mut taken: std::collections::HashSet<PathBuf> = others()
                    .filter(|o| o.holds_place())
                    .map(|o| o.dest.clone())
                    .collect();
                taken.insert(item.dest.clone());
                place_in(dir, &item.file_name, &mut taken)
            })
    }
}

/// The status strip's height, under the list and the Categories pane.
const STRIP_H: f32 = 22.0;

impl Cmd {
    /// The key that runs this command anywhere in the main window, as the shortcuts list writes
    /// it (Help → Keyboard shortcuts); menus show it beside the item.
    fn hint(&self) -> Option<&'static str> {
        Some(match self {
            Cmd::AddUrl => "Ctrl+N",
            Cmd::BatchClipboard => "Ctrl+B",
            Cmd::Find => "Ctrl+F",
            Cmd::FindNext => "F3",
            Cmd::MoveRename => "Ctrl+M",
            Cmd::Remove => "Delete",
            Cmd::RemoveWithFiles => "Shift+Delete",
            Cmd::Open => "Enter",
            Cmd::SelectAll => "Ctrl+A",
            Cmd::Properties => "Alt+Enter",
            Cmd::InterfaceSize(100) => "Ctrl+0",
            _ => return None,
        })
    }
}

/// The open popup menu (other than the menu bar's); notes/ui-spec/menus.md "Context menus".
#[derive(Clone, Copy, PartialEq, Debug)]
enum Popup {
    /// List header right-click: the column chooser.
    Columns,
    /// List right-click on a row, or on blank space (None).
    Item(Option<u64>),
    /// The Start Queue (true) / Stop Queue drop-down arrows.
    QueueDrop(bool),
    /// The Add Torrent button's drop-down arrow.
    TorrentDrop,
    /// Right-click on the drop target.
    DropTarget,
    /// Categories right-click: the node's view (None: the "Queues" root).
    Node(Option<View>),
}

/// Hidden to the tray, the window's memory is given back 2 s after it hid (its last frames done),
/// again 2 minutes after it hid (the clock's ticks page some back in, about 5 MB in the first
/// minute, and the process takes 4 MB more once, about 2 minutes after start), then every
/// 10 minutes while it stays hidden. `done` is how many times it was given back since hiding.
fn next_trim(done: u32) -> Duration {
    match done {
        0 => Duration::from_secs(2),
        1 => Duration::from_secs(118),
        _ => Duration::from_secs(600),
    }
}

/// Started with the session (#226), the window waits hidden until the notification area is
/// known: MDM's own tray icon created and a tray there to show it. A desktop's tray can come
/// after MDM (GNOME's AppIndicator extension at sign-in answered 11.5 s after MDM's icon, on
/// the lab's VM), and MDM's icon after its own first look: this long they are waited for,
/// counted from MDM's icon (from the start while it has none), the window still hidden,
/// before it shows minimized.
const TRAY_GRACE: Duration = Duration::from_secs(30);

/// How long after the main window loses focus a tray toggle still counts it as in
/// front (#520): clicking the icon makes the taskbar the foreground window first,
/// so `focused` is always false by the time Toggle runs on Windows. Unknown focus
/// (`None`) never counts: unsure means show, never hide.
const TRAY_TOGGLE_GRACE: Duration = Duration::from_millis(500);

/// Whether a tray left-click hides the window instead of showing it: hidden or
/// minimized never hides; a focused window does; an unfocused one does only when
/// it just lost the focus (the click itself took it), else it is behind other
/// windows and comes forward. Unknown focus never hides.
fn tray_toggle_hide(
    hidden: bool,
    minimized: bool,
    focused: Option<bool>,
    since_focused: Option<Duration>,
) -> bool {
    if hidden || minimized {
        return false;
    }
    if focused == Some(true) {
        return true;
    }
    focused == Some(false) && since_focused.is_some_and(|d| d < TRAY_TOGGLE_GRACE)
}

/// Where a start with the session (#226) leaves the window, decided here alone.
#[derive(Debug, PartialEq)]
enum AtLogin {
    /// Not known yet: hidden, looked at again in a second.
    Wait,
    /// MDM's icon is in a notification area: the window stays hidden there.
    Tray,
    /// No notification area within [`TRAY_GRACE`]: shown, minimized.
    Minimize,
}

/// `since` the start, and since MDM's tray `icon` was created (None: not yet): whether a tray
/// `host` shows it (Windows, macOS: always one; Linux: a StatusNotifierWatcher, #59). The
/// grace counts from the icon, so a slow start of MDM's own doesn't eat the desktop's time.
fn at_login(since: Duration, icon: Option<Duration>, host: bool) -> AtLogin {
    if icon.is_some() && host {
        AtLogin::Tray
    } else if icon.unwrap_or(since) >= TRAY_GRACE {
        AtLogin::Minimize
    } else {
        AtLogin::Wait
    }
}

/// Hand the memory the process isn't using now back to the system (#72).
fn trim_memory() {
    #[cfg(windows)]
    crate::win32::trim_working_set();
    #[cfg(unix)]
    crate::unix::trim_heap();
}

/// Tree rows carry what they select and which branch they toggle.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Branch {
    Leaf,
    All,
    Unfinished,
    Finished,
    Queues,
}

pub struct App {
    rt: tokio::runtime::Handle,
    items: Vec<Item>,
    next_id: u64,
    /// The list's selection (several downloads, Ctrl / Shift).
    selection: crate::selection::Selection,
    view: View,
    show_categories: bool,
    /// The node the list was last drawn for (a new one starts at the top).
    list_view: View,
    /// The download Move/Rename's dialog is for (winui::start_pick answers later).
    move_rename_for: Option<u64>,
    /// The downloads Move/Rename's folder dialog is for, several selected (#333).
    move_many_for: Vec<u64>,
    /// The Categories tree has the keys (else the list; Tab moves between them).
    tree_keys: bool,
    /// The app's context, to reach its windows from anywhere (bring one to the front).
    ui_ctx: egui::Context,
    /// The language screen readers were last told.
    a11y_lang: &'static str,
    /// Maximized as last seen (the saved size is the restored window's, not the screen's).
    maximized: Option<bool>,
    all_open: bool,
    unfinished_open: bool,
    finished_open: bool,
    queues_open: bool,
    add_url: Option<AddUrl>,
    file_info: Option<FileInfo>,
    /// Addresses (and their browser context) waiting for Download File Info to close.
    file_info_waiting: std::collections::VecDeque<(dialogs::NewUrl, ipc::Command)>,
    /// Download File Info's open answers, for the download each was given to (by id) to start
    /// from (#28: a one-time link refuses a second request).
    kept_answers: std::collections::HashMap<u64, mdm_engine::KeptAnswer>,
    /// Help menu windows.
    about: Option<crate::help::About>,
    updates: Option<crate::help::Updates>,
    shortcuts: Option<crate::help::Shortcuts>,
    tell_friend: Option<crate::help::TellFriend>,
    /// The language picker (first start, View → Language...).
    lang_picker: Option<crate::langpick::Picker>,
    /// The video helpers' offer (the first start's, after the language).
    helpers: Option<crate::helpers::Helpers>,
    /// "Files already there", opened on its own by `/show existing-files` (screenshots and the
    /// translations' fit check); Add torrent asks it itself.
    shown_existing: Option<crate::dialogs::ExistingFiles>,
    /// The browser extension's offer (extension.rs), and whether this run has weighed it.
    extension: Option<crate::extension::Extension>,
    extension_weighed: bool,
    /// The weekly update check, while it runs.
    auto_update: Option<crate::help::Slot>,
    /// The automatic check found a new version: its window comes to the front once shown.
    raise_updates: bool,
    /// Options was asked for from outside (the extension's Button settings..., `/show`): it comes
    /// to the front once shown, or it would open behind the browser (#77).
    raise_options: bool,
    /// When MDM started (the update check waits a few seconds after it).
    started: Instant,
    /// When this run last checked for updates by itself (seconds since the epoch).
    update_checked_this_run: Option<i64>,
    progress: Vec<Progress>,
    complete: Vec<Complete>,
    /// Downloads just finished whose progress window shows them whole for a moment
    /// ([`progress::SHOWN_WHOLE`]) before it gives way to Download complete: each one's id, and
    /// when it finished.
    finishing: Vec<(u64, Instant)>,
    /// Groups of downloads added together (#766) of which one finished or failed since their
    /// last summary: looked at each frame until all theirs are done.
    groups_open: std::collections::BTreeSet<u64>,
    /// Downloads of a group that finished: their group's summary speaks for them, they open no
    /// Download complete of their own (their progress window may still be closing).
    group_finished: std::collections::HashSet<u64>,
    /// Groups done: their summaries (Downloads complete).
    summaries: Vec<dialogs::GroupDone>,
    /// Download complete snoozed (#741), and how many finished meanwhile (the tray icon's tip).
    snooze: crate::finish::Snooze,
    snoozed_finished: u64,
    settings: Settings,
    speed_settings: Option<dialogs::SpeedSettings>,
    scheduler: Option<crate::scheduler::Scheduler>,
    options: Option<crate::options::OptionsDialog>,
    /// Downloads to finish off once stopped: Download File Info was cancelled after an early
    /// start (`None`: the download and its data go), or its file was given another place
    /// (`Some((dest, start))`: the data so far goes, it starts over there when `start`).
    after_stop: Vec<(u64, Option<(PathBuf, bool)>)>,
    /// Tasks → Show drop target: the floating box links are dragged onto.
    drop_target: Option<crate::droptarget::DropTarget>,
    /// Links and files dropped on the main window.
    drops: crate::droptarget::Drops,
    /// The queue selection panel after Download Later / a batch: the downloads waiting for it.
    queue_pick: Option<(Vec<u64>, dialogs::QueuePick)>,
    /// Link grabber (#37): links from the browser, Tasks → Import, drops and pastes, in
    /// packages to tick before adding.
    grabber: Option<crate::grabber::Grabber>,
    /// When `follow_assistive_technology` last asked (#131).
    assistive_checked: Option<Instant>,
    /// Archives extracted when their download completes (#37).
    extracts: crate::extractq::Extracts,
    /// Add category / Properties from the Categories tree.
    category_dlg: Option<crate::categories::CategoryDialog>,
    /// The category dialog was opened by Download File Info's "+" (its result is picked there).
    category_for_file_info: bool,
    /// Delete a category of the user's, asking first.
    confirm_category_delete: Option<dialogs::DeleteCategory>,
    /// File Properties windows, one per download.
    properties: Vec<crate::properties::Properties>,
    /// Verify checksum windows, one per download.
    checksums: Vec<crate::checksum::ChecksumDialog>,
    batch: Option<crate::batch::Batch>,
    video_dlg: Option<crate::videodlg::VideoDialog>,
    /// The browser's quality menu's look-ups (videolookup.rs).
    lookups: crate::videolookup::Lookups,
    /// Download a gallery (#58).
    gallery_dlg: Option<crate::gallerydlg::GalleryDialog>,
    torrent_dlg: Option<crate::torrentdlg::TorrentDialog>,
    /// Magnet links and .torrents handed over while Add torrent was open (#507), oldest first:
    /// each opens in Add torrent when the one before it closes.
    torrent_waiting: std::collections::VecDeque<(String, Option<crate::torrents::Remote>)>,
    /// Add torrent closed this frame: the next waiting one opens after a frame without it.
    torrent_closed: bool,
    torrent_settings: Option<crate::torrentdlg::TorrentSettings>,
    /// Right-click → Torrent → Add trackers… (#39).
    add_trackers: Option<crate::torrentdlg::AddTrackers>,
    torrent_files: Option<crate::torrentfiles::TorrentFiles>,
    /// RSS feeds and their download rules (#39, rss.rs), kept in rss.json; the window; the
    /// feeds being fetched; what the fetches and look-ups sent back (`rss_tick`).
    rss: crate::rss::Store,
    rss_dlg: Option<crate::rssdlg::RssWindow>,
    rss_busy: std::collections::HashSet<u64>,
    rss_events: std::sync::Arc<std::sync::Mutex<Vec<RssEvent>>>,
    /// Torrents whose list numbers wait for the session after an action (Recheck, files
    /// chosen; `torrent_resync`), by info hash: true when what the check found goes in the
    /// event log.
    torrent_resync: HashMap<String, bool>,
    /// Since when no torrent has needed the torrent engine (`torrent_tick`).
    torrent_idle_since: Option<Instant>,
    /// What each torrent sent, run after run (`torrent_accounts`), when last counted (whole
    /// seconds), and when its totals last made the list be saved.
    accountant: crate::torrents::Accountant,
    account_at: Instant,
    account_saved: Instant,
    /// Each running torrent's bytes from peers as last seen (by item id), and the torrents'
    /// smoothed speed with when it was measured (`torrent_speed`, for the global cap).
    torrent_fetched: HashMap<u64, u64>,
    torrent_speed: (f64, Instant),
    /// The torrents' share of the global cap as last set, and when (`apply_global_limit`).
    torrent_cap: (Option<u64>, Instant),
    /// File downloads' smoothed speed, when measured, and the global limiter's byte count
    /// then (`file_speed`).
    file_speed: (f64, Instant, u64),
    /// A blocklist is being fetched (torrents::refresh_blocklist).
    blocklist_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Startup work for torrents done (associations refreshed, sharing resumed).
    torrents_started: bool,
    /// The torrent session, started on first use (D-014).
    torrent_host: crate::torrents::Host,
    /// The remote control (#141): paired phones and computers on the local network.
    pub remote: Option<mdm_remote::Remote>,
    /// Torrents a paired device sent, their files being looked up (`remote::poll_torrents`).
    pub remote_torrents: Vec<crate::remote::Lookup>,
    /// Click'n'Load (#37): what pages post waits here, for its site's answer and Link grabber.
    cnl: crate::cnl::Cnl,
    /// Free space where downloads go, and when it was last looked up (`free_space`).
    free_space: std::sync::Arc<std::sync::Mutex<(Option<Instant>, Option<u64>)>>,
    /// Options → Proxy / Socks → Network adapter (#35): the watch, the route seen last frame, and
    /// the downloads stopped because it changed, to start again when it allows.
    netwatch: crate::netwatch::NetWatch,
    /// Options → Accounts (#44): the debrid accounts downloads go through, and what they were
    /// set up from (so they are set up again only when that changes).
    debrid: mdm_debrid::Debrid,
    debrid_from: u64,
    route: mdm_engine::Route,
    adapter_paused: Vec<u64>,
    /// Videos and galleries (#708): yt-dlp and gallery-dl take a speed limit only as they start
    /// (`--limit-rate`). The limits each running one was started under (`helper_limits`), the limit
    /// set in its progress window without Remember (it lasts until it is stopped), and the ones
    /// stopped to start again at a new limit, started once their jobs have ended
    /// (`limit_restart_tick`). #769 moves their downloads to the engine, whose limits follow a
    /// change by themselves.
    helper_rates: HashMap<u64, (Option<u64>, Option<u64>)>,
    /// The rate each running video and gallery was started with (`helper_rate`).
    helper_started: HashMap<u64, Option<u64>>,
    helper_own: HashMap<u64, Option<u64>>,
    limit_restarts: Vec<u64>,
    find: Option<crate::find::Find>,
    /// The last search, for Find Next.
    last_find: Option<crate::find::Query>,
    /// A message with OK: its window title and message: "Cannot find …", an export that failed…
    notice: Option<(&'static str, String)>,
    /// The list files Import is reading, beside the window (#805).
    importing: crate::transfer::Importing,
    /// Why the last save of the download list failed (told once per reason).
    save_failed: Option<String>,
    /// Finished files being moved (Move/Rename, Properties → Move): where to, and the answer.
    moving: Vec<(u64, PathBuf, MoveDone)>,
    /// Finished torrents whose deferred rename is running asynchronously through `torrent_host`.
    #[allow(clippy::type_complexity)]
    renaming_torrents: Vec<(
        u64,
        PathBuf,
        std::sync::mpsc::Receiver<(Result<PathBuf, String>, Option<String>)>,
    )>,
    /// Finished downloads whose deferred rename is persisted on `Item::rename_on_complete` and
    /// waiting for `torrent_host` (or startup recovery) to run it.
    deferred_torrent_queue: Vec<u64>,
    /// A finished queue asked to turn the computer off: the countdown, cancellable.
    turn_off: Option<dialogs::TurnOffCountdown>,
    /// A finished queue asked to exit.
    exit_requested: bool,
    /// The global speed cap shared by every download (Speed Limiter settings).
    global: std::sync::Arc<mdm_engine::limiter::Limiter>,
    /// Connection caps learned per host, shared by every download.
    host_limits: std::sync::Arc<mdm_engine::hosts::HostLimits>,
    /// Scheduler → Download limits: traffic in the window, and what the limit holds back.
    quota: crate::quota::Quota,
    /// "Show warning before stopping downloads": the notice, while open.
    limit_notice: Option<dialogs::LimitNotice>,
    /// The notification-area icon (created on the first frame), the window hidden in it, and
    /// a real exit under way (closing the window otherwise only hides it, as IDM does).
    tray: Option<crate::tray::Tray>,
    /// Linux: the icon being made beside the window (#828: its library waits for the session
    /// bus); `tray` once it is.
    #[cfg(target_os = "linux")]
    tray_setup: Option<crate::tray::Setup>,
    tray_limiter: Option<bool>,
    hotkey: Option<crate::hotkey::GlobalHotkey>,
    /// When the main window was last seen focused: a tray click steals the focus first
    /// (the taskbar becomes foreground), so a click on a frontmost window reads as
    /// unfocused by the time Toggle runs. Within [`TRAY_TOGGLE_GRACE`] it still counts
    /// as in front.
    tray_last_focused: Option<Instant>,
    hidden: bool,
    /// Hidden to the tray: when to hand the window's memory back to the system next (#72), and
    /// how many times it was since the window hid.
    trim_when_hidden: Option<Instant>,
    trims_since_hidden: u32,
    /// Started with the session, the window hidden until the notification area is known
    /// (#226, [`at_login`]): when, when MDM's tray icon was first seen, and when it was last
    /// looked at.
    at_login: Option<Instant>,
    at_login_icon: Option<Instant>,
    at_login_checked: Option<Instant>,
    exiting: bool,
    /// Run without a window (`--headless`, #157): nothing is shown or asked; what arrives is
    /// added as the remote control adds it.
    headless: bool,
    /// Add batch download from clipboard (#709): its read, until it answers.
    batch_clip: Option<crate::clipboard::Prefill>,
    confirm_delete: Option<dialogs::ConfirmDelete>,
    /// Queue right-click → Delete, asking first: the queue and the question.
    confirm_queue_delete: Option<(usize, dialogs::Confirm)>,
    /// Redownload, asking first: the download and the question.
    confirm_redownload: Option<(u64, dialogs::Confirm)>,
    /// A duplicate address asking what to do: the download already there, what was being added.
    duplicate: Option<(u64, Pending, dialogs::Duplicate)>,
    /// Duplicates that came while one was being asked about, next in turn.
    duplicates_waiting: std::collections::VecDeque<(u64, Pending, dialogs::Duplicate)>,
    /// The next duplicate check is passed (the user chose to add it anyway).
    duplicate_cleared: bool,
    downloads: PathBuf,
    dirty: bool,
    /// `secrets::Vault::confirmed` when last seen.
    secrets_confirmed: u64,
    last_save: Instant,
    /// Writes the list off the window's thread.
    saver: store::Saver,
    /// Writes `rss.json` off the window's thread: the feeds' timer saves it by itself (#513).
    rss_saver: store::Saver,
    /// When the light/dark mode was last checked against Windows' app mode.
    theme_checked: Option<Instant>,
    /// The zoom (View → Interface size, #262) the windows were last drawn at: a change is saved,
    /// and the windows that don't follow by themselves are made to (`follow_zoom`).
    zoom: f32,
    /// The main window was brought inside the screen after the start (scale::fit_window).
    fitted: bool,
    /// View → Theme's choices (#45), and the number of the theme watcher's answer they come from
    /// (`lookwatch::theme_files`: taken again when it changes).
    theme_list: Vec<crate::themefile::Entry>,
    theme_files_seen: u64,
    /// When `logic`'s every-2-seconds work last ran (crash state, torrents, #174).
    ticked: Option<Instant>,
    /// The main window's caption matches the mode.
    caption_set: bool,
    /// Windows: whether the main window's title bar is ours (a style theme's, #45), as applied
    /// to its frame; None until the window exists. And the window, once it is.
    #[cfg_attr(not(windows), allow(dead_code))]
    drawn_frame: Option<bool>,
    #[cfg_attr(not(windows), allow(dead_code))]
    main_hwnd: Option<isize>,
    list: list::ListState,
    icons: ShellIcons,
    /// Open top-level menu (index into `chrome::menus()`).
    open_menu: Option<usize>,
    /// Alt is down, and was pressed alone so far (releasing it opens the menu bar).
    alt_down: bool,
    alt_alone: bool,
    /// The open context / drop-down menu, and its top-left.
    popup: Option<(Popup, Pos2, u64)>,
    /// Counts menu opens: each open is a new menu window, even at the same place.
    popup_serial: u64,
    /// The column layout last seen, to notice changes that need saving.
    last_layout: Option<list::Layout>,
    /// Main download queue (0), synchronization queue (1).
    queues: Vec<Queue>,
    /// Failure time and count per download, for queue retries.
    failures: HashMap<u64, (Instant, u32)>,
    /// Stopped by hand while they could have run: a running queue leaves them alone until
    /// they are started again or the queue is started anew (#341).
    stopped_by_hand: std::collections::HashSet<u64>,
    /// Seconds since local midnight at the last scheduler tick (`None` until the clock's first
    /// answer, #536).
    clock: Option<u32>,
    /// Scheduler → "Wake the computer to run this queue": the wake set with the system (#142).
    wake: crate::wake::Waker,
    /// Commands from ipc.rs (command line, second launch, browser).
    commands: std::sync::mpsc::Receiver<ipc::Incoming>,
}

/// The login written in an address (`ftp://user:pass@host/…`), decoded.
fn url_login(url: &url::Url) -> Option<(String, String)> {
    let password = url.password()?;
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8_lossy()
            .into_owned()
    };
    Some((decode(url.username()), decode(password)))
}

/// `Authorization: Basic …` for "Use authorization".
/// What a download's requests carry: its headers (a login, the browser's Referer and Cookie, a
/// form's Content-Type) and the options its requests go with (User-Agent, proxy, a form's body).
/// Download File Info's look-up asks with the same as the download, so both get one answer.
#[derive(Clone)]
pub struct RequestParts {
    pub headers: HeaderMap,
    pub opts: Options,
}

/// The Referer a browser would send for `url` from page `referrer` (its default policy,
/// strict-origin-when-cross-origin): the whole address on the same site, only the site's origin
/// to another, nothing from https to http.
fn sent_referrer(url: &url::Url, referrer: &str) -> Option<String> {
    let from = url::Url::parse(referrer).ok()?;
    if !matches!(from.scheme(), "http" | "https") {
        return None;
    }
    if from.scheme() == "https" && url.scheme() == "http" {
        return None;
    }
    if from.origin() == url.origin() {
        let mut full = from.clone();
        full.set_fragment(None);
        let _ = full.set_username("");
        let _ = full.set_password(None);
        return Some(full.to_string());
    }
    Some(format!("{}/", from.origin().ascii_serialization()))
}

/// What a link found on a page takes along from that page.
fn page_data_for(link: &str, page: Option<&str>, data: Option<&str>) -> Option<String> {
    let (link, page) = (url::Url::parse(link).ok()?, url::Url::parse(page?).ok()?);
    let same_host = link.host_str().is_some()
        && link
            .host_str()
            .map(str::to_ascii_lowercase)
            .eq(&page.host_str().map(str::to_ascii_lowercase));
    let downgrade = page.scheme() == "https" && link.scheme() != "https";
    (same_host && !downgrade)
        .then(|| data.map(str::to_owned))
        .flatten()
}

/// Marks finished files (a video's and its subtitles, a gallery's pictures) as downloaded from the
/// web, as `mark_hook` does for the engine's, on a thread of their own: writing the mark has the
/// antivirus read the whole file again, and on a big video that kept the window waiting (#510). A
/// file moved or removed meanwhile is left alone: marking its old path would create an empty
/// file there.
pub(crate) fn mark_finished(
    files: Vec<std::path::PathBuf>,
    url: String,
    referrer: Option<String>,
) -> Option<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("mark-downloaded".into())
        .spawn(move || {
            for f in files.iter().filter(|f| f.is_file()) {
                crate::webmark::mark(f, &url, referrer.as_deref());
            }
        })
        .ok()
}

/// Marks each file the engine starts writing as downloaded from the web (Mark-of-the-Web, macOS'
/// quarantine: `webmark::mark`, as the option and Windows' policy say), as browsers mark theirs.
/// Only http(s) addresses (an FTP address may carry its login).
pub(crate) fn mark_hook(url: &url::Url, referrer: Option<&str>) -> mdm_engine::FileHook {
    let web = matches!(url.scheme(), "http" | "https");
    let (url, referrer) = (url.to_string(), referrer.map(str::to_owned));
    mdm_engine::FileHook::new(move |path: &std::path::Path| {
        if web {
            crate::webmark::mark(path, &url, referrer.as_deref());
        }
    })
}

/// The Google Drive file whose player the browser's video button was clicked over (#34): the
/// page itself when it is a Drive file's (`video_page`), or, when the button sent the player's
/// own stream (on Google's hosts), the page it came from (`referer`). None for any other video.
fn drive_file_behind_player(cmd: &ipc::Command) -> Option<String> {
    use mdm_engine::sites::gdrive;
    let drive_file = |s: &str| {
        url::Url::parse(s)
            .ok()
            .and_then(|u| gdrive::parse(&u))
            .is_some_and(|l| l.kind == gdrive::Kind::File)
    };
    let page = cmd.video_page.as_deref()?;
    if drive_file(page) {
        return Some(page.to_owned());
    }
    let googles = url::Url::parse(page).ok().and_then(|u| {
        u.host_str().map(|h| {
            let h = h.to_ascii_lowercase();
            [
                ".google.com",
                ".googleapis.com",
                ".googlevideo.com",
                ".googleusercontent.com",
            ]
            .iter()
            .any(|d| h.ends_with(d))
        })
    });
    cmd.referer
        .as_deref()
        .filter(|r| googles == Some(true) && drive_file(r))
        .map(str::to_owned)
}

/// What the browser's quality menu leaves to the Download video window (the button's click, as
/// before it): everything in a run without a window, a Google Drive file's player (#34: the
/// file itself, as uploaded, which the click's command downloads), and an address not on the
/// web (the browser's host sends none).
fn menu_needs_window(cmd: &ipc::Command, headless: bool) -> bool {
    headless
        || cmd.video_lookup.as_ref().is_some_and(|ask| {
            !(ask.address.starts_with("http://") || ask.address.starts_with("https://"))
                || drive_file_behind_player(&ipc::Command {
                    video_page: Some(ask.address.clone()),
                    referer: cmd.referer.clone(),
                    ..Default::default()
                })
                .is_some()
        })
}

/// Where a video just added goes ([`video_goes`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VideoGoes {
    /// Downloading now, its progress window up.
    Start,
    /// Into the queue the selection panel asks for (Options → Downloads).
    AskQueue,
    /// Into the main queue, nothing asked.
    MainQueue,
}

/// Where a video added goes: Download Now (`start_now`) starts it; Later asks which queue when
/// Options say so (`queue_panel`). A choice made in the browser's quality menu (`from_menu`,
/// with Options' "only add to the queue") asks nothing in MDM's window, which the browser's
/// choice doesn't open: it goes into the main queue, as the browser's own downloads do then
/// (`open_url`'s silent path). Never in no queue at all, which a cancelled panel would leave it.
fn video_goes(start_now: bool, from_menu: bool, queue_panel: bool) -> VideoGoes {
    if start_now {
        VideoGoes::Start
    } else if queue_panel && !from_menu {
        VideoGoes::AskQueue
    } else {
        VideoGoes::MainQueue
    }
}

pub fn auth_headers(login: Option<&(String, String)>) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Some((user, pass)) = login {
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        if let Ok(v) = HeaderValue::from_str(&format!("Basic {token}")) {
            h.insert(AUTHORIZATION, v);
        }
    }
    h
}

/// A stopped download's files: its state and part files, and its finished file when it was
/// complete (a Stop-then-remove of a download that finished meanwhile).
fn remove_files_of(dest: &std::path::Path, complete: bool) {
    let _ = std::fs::remove_file(state_path(dest));
    let _ = std::fs::remove_file(mdm_engine::naming::part_path(dest));
    if complete {
        let _ = std::fs::remove_file(dest);
    }
}

/// The icon for a file name's extension (empty: none).
fn file_icon(name: &str) -> IconKey {
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => IconKey::Ext(ext.to_ascii_lowercase()),
        _ => IconKey::Ext(String::new()),
    }
}

/// Sort key for list column col: numbers where IDM sorts numerically, else text.
/// `queued`: the download is in a queue (the Q column's mark).
fn sort_key(it: &Item, col: usize, queued: bool) -> (f64, String) {
    let frac = it.fraction().unwrap_or(0.0);
    match col {
        0 => (0.0, it.file_name.to_lowercase()),
        // As the column shows it: in a queue or not.
        1 => (f64::from(u8::from(queued)), String::new()),
        2 => (it.size.unwrap_or(0) as f64, String::new()),
        3 => (
            if it.status == Status::Complete {
                2.0
            } else {
                frac
            },
            String::new(),
        ),
        4 => (
            it.size.map_or(f64::MAX, |s| {
                (s.saturating_sub(it.downloaded)) as f64 / it.rate().max(1.0)
            }),
            String::new(),
        ),
        5 => (it.rate(), String::new()),
        // By time, not by the text ("Apr" would come before "Oct").
        6 => (
            it.last_try
                .as_deref()
                .and_then(model::stamp_time)
                .map_or(0.0, |t| t.and_utc().timestamp() as f64),
            String::new(),
        ),
        // Added: the order of addition itself (ids only grow), exact even without a time.
        list::ADDED => (it.id as f64, String::new()),
        list::SOURCE => (f64::from(u8::from(it.torrent.is_some())), String::new()),
        // A torrent's upload, its peers, and its share ratio (not the text: "10" would come
        // before "9"). Anything but a torrent, and a torrent that has taken nothing, sorts as
        // zero.
        list::UPLOAD => (
            it.torrent.as_ref().map_or(0.0, |t| t.up_speed),
            String::new(),
        ),
        list::PEERS => (
            it.torrent.as_ref().map_or(0.0, |t| t.peers as f64),
            String::new(),
        ),
        list::RATIO => (
            it.torrent.as_ref().map_or(0.0, |t| {
                crate::torrents::share_ratio(t.uploaded, it.downloaded)
            }),
            String::new(),
        ),
        _ => (0.0, it.description.to_lowercase()),
    }
}

/// Silk icons standing in for IDM's XP-era category icons.
/// A theme's icon set may replace them (`crate::themeicons`).
fn category_icon(c: Category) -> IconKey {
    let (need, silk) = match c {
        Category::Compressed => ("cat-compressed", "page_white_zip"),
        Category::Documents => ("cat-documents", "page_white"),
        Category::Music => ("cat-music", "music"),
        Category::Programs => ("cat-programs", "application_xp"),
        Category::Video => ("cat-video", "film"),
        Category::General => ("cat-documents", "page_white"),
        Category::Torrents => ("cat-torrents", "folder_link"),
        Category::Custom(_) => ("cat-folder", "folder"),
    };
    IconKey::Themed(need, silk)
}

impl App {
    /// For a run without a window (#157): downloads go to `downloads` when given (a server's
    /// share), and nothing waits for an answer from someone at the computer.
    pub fn headless(mut self, downloads: Option<PathBuf>) -> Self {
        self.headless = true;
        if let Some(d) = downloads {
            self.downloads = d;
        }
        // Nobody to answer Click'n'Load's question or tick Link grabber's packages.
        let _ = self.cnl.listen(false);
        self
    }

    /// Asked to quit (File → Exit, `/exit`, the Scheduler's "Exit when done").
    pub fn wants_exit(&self) -> bool {
        self.exiting || self.exit_requested
    }

    pub fn new(
        ctx: &egui::Context,
        rt: tokio::runtime::Handle,
        commands: std::sync::mpsc::Receiver<ipc::Incoming>,
        sink: ipc::Sink,
        loaded: store::Loaded,
        start: crate::Start,
    ) -> Self {
        // The look (#45): the chosen theme before anything is drawn.
        crate::theme::select(&crate::theme::startup_id(&loaded.settings.theme), ctx);
        crate::theme::apply(ctx);
        // The zoom main.rs started at (View → Interface size, #262).
        let zoom = crate::scale::startup(loaded.settings.interface_size);
        let mut settings = loaded.settings;
        if settings.limiter_on_startup {
            settings.speed_limit_on = true;
        }
        // The language (D-019): the installer's wizard says which on its first start (it always
        // leaves the value; take it so an upgrade's can't override a later choice); a portable
        // copy writes nothing to the registry. With neither, the first start asks.
        #[cfg(windows)]
        let installer = mdm_ipc::portable_dir()
            .is_none()
            .then(crate::win32::take_install_language)
            .flatten();
        #[cfg(not(windows))]
        let installer: Option<String> = None;
        if !settings.language_asked
            && let Some(code) = installer.filter(|c| crate::i18n::i18n().from_code(c).is_some())
        {
            // Windows' own language is kept as "follow Windows".
            settings.language = (code != crate::i18n::system_code()).then_some(code);
            settings.language_asked = true;
        }
        let ask_language = !settings.language_asked;
        crate::i18n::set_lang(settings.language.as_deref());
        crate::theme::language_fonts(ctx);
        crate::regional::set(&settings.regional);
        crate::webmark::set(settings.mark_downloads);
        if settings.proxy_mode == 2 {
            crate::pacfetch::refresh(&settings.proxy_script, &rt);
        }
        // "Launch on startup" is whatever the Run key says (the installer may have set it); on
        // Linux and macOS, the autostart entry / LaunchAgent.
        // An entry written before #226 (by versions before 0.5.0, or by their installer) starts MDM
        // with its window in front: written again, with the argument that starts it in the
        // notification area. Only an entry for this very program (not one a copy elsewhere
        // wrote), and never by a portable copy, which writes nothing outside its folder.
        #[cfg(windows)]
        {
            settings.launch_on_startup = crate::win32::run_at_startup();
            if settings.launch_on_startup && mdm_ipc::portable_dir().is_none() {
                crate::win32::update_run_at_startup();
            }
        }
        #[cfg(unix)]
        {
            settings.launch_on_startup = crate::unix::run_at_startup();
            if settings.launch_on_startup && mdm_ipc::portable_dir().is_none() {
                crate::unix::update_run_at_startup();
            }
            if mdm_ipc::portable_dir().is_none() && !crate::offscreen() {
                crate::unix::register_browsers();
            }
        }
        // The categories, for code that only has a `Category` (the list's own below too).
        settings.publish_categories();
        // The remote control (#141): off unless Options → Remote turned it on.
        let remote = crate::remote::start(&rt, sink, &settings);
        // Started with the session where a window can't be hidden (Wayland, #226): minimized.
        if start == crate::Start::Minimized {
            tracing::info!("started with the session: minimized (a window can't be hidden here)");
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        let mut app = Self::assemble(
            ctx,
            rt,
            commands,
            store::Loaded { settings, ..loaded },
            start,
        );
        app.zoom = zoom;
        app.lang_picker = ask_language.then(|| crate::langpick::Picker::new(true, None));
        app.remote = remote;
        // Click'n'Load (#37): off unless Options → Archives and Link grabber turned it on.
        if let Err(why) = app.cnl.listen(app.settings.cnl_on) {
            // i18n: title of the notice when Click'n'Load couldn't start
            app.notice = Some((crate::i18n::mark("Click'n'Load"), why));
        }
        app.rss = crate::rss::Store::path()
            .map(|p| crate::rss::Store::load(&p))
            .unwrap_or_default();
        app
    }

    /// The window's state from `loaded`, setting nothing outside it: no global, nothing of the
    /// system's, no file read ([`App::new`] does those first). The menus' test builds one.
    fn assemble(
        ctx: &egui::Context,
        rt: tokio::runtime::Handle,
        commands: std::sync::mpsc::Receiver<ipc::Incoming>,
        loaded: store::Loaded,
        start: crate::Start,
    ) -> Self {
        let settings = loaded.settings;
        let mut queues = loaded.queues;
        if queues.len() < 2 {
            // The built-in queues' names are saved in English and translated where shown
            // (`t(&queue.name)`; a name the user typed is not in the catalog and stays).
            queues = vec![
                Queue::new(crate::i18n::mark("Main download queue"), false),
                Queue::new(crate::i18n::mark("Synchronization queue"), true),
            ];
        }
        for q in &mut queues {
            q.running = q.schedule.start_on_startup;
        }
        let (next_id, mut items) = (loaded.next_id, loaded.items);
        // D-021: torrents are a category of their own (lists from before had them by file
        // type); their files stay where they are.
        for i in &mut items {
            if i.torrent.is_some() {
                i.category = Category::Torrents;
            }
            // A user's category that is gone: General.
            if let Category::Custom(id) = i.category
                && crate::model::custom(id).is_none()
            {
                i.category = Category::General;
            }
        }
        let mut list = list::ListState::default();
        if let Some(cols) = loaded.columns {
            list.apply(cols);
        }
        let downloads = dirs::download_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
            .unwrap_or_else(|| PathBuf::from("."));
        // The Categories pane as it was left, and the node shown (if it still exists).
        let [all_open, unfinished_open, finished_open, queues_open] = settings.branches;
        let known = |c: Option<Category>| c.is_none_or(|c| Category::all().contains(&c));
        let view = match settings.view {
            Some(View::Queue(q)) if q < queues.len() => View::Queue(q),
            Some(v @ View::Cat(c)) if known(Some(c)) => v,
            Some(v @ (View::Unfinished(c) | View::Finished(c))) if known(c) => v,
            _ => View::All,
        };
        let show_categories = settings.show_categories;
        // Click'n'Load (#37): made here, listening only once `new` starts it (Options →
        // Archives and Link grabber turns it on).
        let cnl = crate::cnl::Cnl::new(rt.clone(), ctx.clone());
        let deferred_torrent_queue: Vec<u64> = items
            .iter()
            .filter(|i| i.status == Status::Complete && i.rename_on_complete.is_some())
            .map(|i| i.id)
            .collect();
        App {
            rt,
            items,
            next_id,
            selection: Default::default(),
            view,
            show_categories,
            all_open,
            unfinished_open,
            finished_open,
            queues_open,
            maximized: None,
            a11y_lang: "",
            ui_ctx: ctx.clone(),
            tree_keys: false,
            move_rename_for: None,
            move_many_for: Vec::new(),
            list_view: view,
            add_url: None,
            about: None,
            updates: None,
            shortcuts: None,
            tell_friend: None,
            lang_picker: None,
            helpers: None,
            shown_existing: None,
            extension: None,
            extension_weighed: false,
            auto_update: None,
            raise_updates: false,
            raise_options: false,
            started: Instant::now(),
            update_checked_this_run: None,
            file_info: None,
            file_info_waiting: Default::default(),
            kept_answers: Default::default(),
            progress: Vec::new(),
            complete: Vec::new(),
            finishing: Vec::new(),
            groups_open: Default::default(),
            group_finished: Default::default(),
            summaries: Vec::new(),
            snooze: crate::finish::Snooze::from_saved(settings.complete_snoozed_until, unix_now()),
            snoozed_finished: 0,
            settings,
            speed_settings: None,
            scheduler: None,
            options: None,
            category_dlg: None,
            category_for_file_info: false,
            grabber: None,
            assistive_checked: None,
            extracts: Default::default(),
            queue_pick: None,
            drop_target: None,
            drops: Default::default(),
            after_stop: Vec::new(),
            confirm_category_delete: None,
            properties: Vec::new(),
            checksums: Vec::new(),
            batch: None,
            video_dlg: None,
            lookups: Default::default(),
            gallery_dlg: None,
            torrent_dlg: None,
            torrent_waiting: Default::default(),
            torrent_closed: false,
            torrent_settings: None,
            add_trackers: None,
            torrent_files: None,
            rss: Default::default(),
            rss_dlg: None,
            rss_busy: Default::default(),
            rss_events: Default::default(),
            torrent_resync: HashMap::new(),
            torrent_idle_since: None,
            accountant: Default::default(),
            account_at: Instant::now(),
            account_saved: Instant::now(),
            torrent_fetched: HashMap::new(),
            torrent_speed: (0.0, Instant::now()),
            torrent_cap: (None, Instant::now()),
            file_speed: (0.0, Instant::now(), 0),
            blocklist_busy: Default::default(),
            torrents_started: false,
            torrent_host: crate::torrents::Host::new(),
            remote: None,
            remote_torrents: Vec::new(),
            cnl,
            free_space: Default::default(),
            netwatch: Default::default(),
            debrid: Default::default(),
            debrid_from: 0,
            route: mdm_engine::Route::Any,
            adapter_paused: Vec::new(),
            helper_rates: HashMap::new(),
            helper_started: HashMap::new(),
            helper_own: HashMap::new(),
            limit_restarts: Vec::new(),
            find: None,
            last_find: None,
            notice: None,
            importing: Default::default(),
            save_failed: None,
            moving: Vec::new(),
            renaming_torrents: Vec::new(),
            deferred_torrent_queue,
            turn_off: None,
            exit_requested: false,
            global: std::sync::Arc::new(mdm_engine::limiter::Limiter::new(None)),
            host_limits: Default::default(),
            quota: Default::default(),
            limit_notice: None,
            batch_clip: None,
            tray: None,
            #[cfg(target_os = "linux")]
            tray_setup: None,
            tray_limiter: None,
            hotkey: None,
            tray_last_focused: None,
            // Started with the session (#226): hidden from the first frame.
            hidden: start == crate::Start::Session,
            trim_when_hidden: (start == crate::Start::Session)
                .then(|| Instant::now() + next_trim(0)),
            trims_since_hidden: 0,
            at_login: (start == crate::Start::Session).then(Instant::now),
            at_login_icon: None,
            at_login_checked: None,
            exiting: false,
            headless: false,
            confirm_delete: None,
            confirm_queue_delete: None,
            confirm_redownload: None,
            duplicate: None,
            duplicates_waiting: Default::default(),
            duplicate_cleared: false,
            downloads,
            dirty: false,
            secrets_confirmed: 0,
            last_save: Instant::now(),
            saver: store::Saver::new(),
            rss_saver: store::Saver::writing("rss saver", crate::rss::write),
            theme_checked: None,
            zoom: 1.0,
            fitted: false,
            // The built-ins until the watcher's first look at the themes folder (#719).
            theme_list: crate::themefile::list_in(None, None),
            theme_files_seen: 0,
            ticked: None,
            caption_set: false,
            // The system's title bar is the frame's own: nothing to apply for it.
            drawn_frame: Some(false),
            main_hwnd: None,
            list,
            icons: ShellIcons,
            open_menu: None,
            alt_down: false,
            alt_alone: false,
            popup: None,
            popup_serial: 0,
            last_layout: None,
            queues,
            failures: HashMap::new(),
            stopped_by_hand: Default::default(),
            clock: None,
            wake: Default::default(),
            commands,
        }
    }

    fn item(&mut self, id: u64) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// The one download single-download commands act on (Properties, Open…).
    fn selected_item(&self) -> Option<&Item> {
        self.selection
            .one()
            .and_then(|id| self.items.iter().find(|i| i.id == id))
    }

    /// The downloads a command acts on: those selected, in list order; a right-clicked one
    /// outside the selection alone.
    fn targets(&self, target: Option<u64>) -> Vec<u64> {
        match target {
            Some(t) if !self.selection.contains(t) => vec![t],
            _ => self.selection.ids().to_vec(),
        }
    }

    /// The downloads the selection-wide commands would act on.
    fn chosen(&self, target: Option<u64>) -> Vec<&Item> {
        self.targets(target)
            .into_iter()
            .filter_map(|id| self.items.iter().find(|i| i.id == id))
            .collect()
    }

    /// An empty list's message, centred in its rows' area `body` (English): an icon, what is
    /// empty, why, and the one thing to do about it (clear the filters, show every download,
    /// add one). Returns that command when it is clicked.
    fn empty_state(&self, ui: &Ui, body: egui::Rect) -> Option<Cmd> {
        let p = ui.painter();
        let filtered = self.list.filters.iter().any(Option::is_some) && !self.view_ids().is_empty();
        let (title, why, action, cmd) = if filtered {
            (
                // i18n: title in the empty download list, when the column filters hide every download
                "No downloads match the filters",
                // i18n: in the empty download list, under its title
                "The column filters hide every download in this view.",
                "Clear all filters",
                Cmd::ClearFilters,
            )
        } else if !self.items.is_empty() {
            (
                // i18n: title in the empty download list (this category or queue has no downloads)
                "No downloads here",
                // i18n: in the empty download list, under its title
                "This category or queue holds no downloads yet.",
                // i18n: link in the empty download list: go to the All Downloads view
                "Show all downloads",
                Cmd::ShowAll,
            )
        } else {
            (
                // i18n: title in the empty download list when there are no downloads at all
                "No downloads yet",
                // i18n: in the empty download list, under its title
                "The downloads you add, or drop on this window, show here.",
                // i18n: link in the empty download list: opens Add URL
                "Add new download",
                Cmd::AddUrl,
            )
        };
        let (title, why) = (crate::i18n::t(title), crate::i18n::t(why));
        let title_font = winui::tahoma_bold();
        let tw = winui::text_width_font(p, title_font.clone(), title);
        let ww = winui::text_width(p, why);
        let aw = winui::text_width(p, crate::i18n::t(action));
        // The block: icon 40, title, reason, link; in the middle, or under the header when
        // the list is short.
        let block_h = 40.0 + 14.0 + 16.0 + 8.0 + 13.0 + 14.0 + 13.0;
        let top = (body.center().y - block_h / 2.0).max(body.top() + 16.0);
        let cx = body.center().x;
        // A download glyph in the muted ink (outlined, as the toolbar's icons are).
        let ink = egui::Stroke::new(1.5, winui::MUTED());
        let c = winui::rect(egui::Rect::from_center_size(
            pos2(cx, top + 20.0),
            egui::vec2(1.0, 1.0),
        ))
        .center();
        p.circle_stroke(c, 19.0, ink);
        p.line_segment([c + egui::vec2(0.0, -10.0), c + egui::vec2(0.0, 9.0)], ink);
        p.line_segment([c + egui::vec2(-7.0, 2.0), c + egui::vec2(0.0, 9.0)], ink);
        p.line_segment([c + egui::vec2(7.0, 2.0), c + egui::vec2(0.0, 9.0)], ink);
        let y = top + 40.0 + 14.0 + 12.0;
        winui::text_font(p, title_font, cx - tw / 2.0, y, title, winui::TEXT());
        let y = y + 8.0 + 13.0;
        winui::text(p, cx - ww / 2.0, y, why, winui::MUTED());
        let y = y + 14.0 + 13.0;
        winui::link(ui, p, cx - aw / 2.0, y, action, "empty-action").then_some(cmd)
    }

    /// The strip under the list: how many downloads are shown and selected, how many run and
    /// how fast in all (beyond IDM, which has no status bar).
    fn status_strip(&self, ui: &Ui, o: Pos2, w: f32, h: f32, shown: usize) {
        let p = ui.painter();
        let top = o.y + h - STRIP_H;
        // Centred in the band it reads as: the strip and the 5 px gap above it, under the panes.
        let baseline = winui::baseline_in(top - 5.0, STRIP_H + 5.0);
        let n = shown as u64;
        let mut left = crate::i18n::tn(n, "{n} download", "{n} downloads");
        let selected = self.selection.ids().len() as u64;
        if selected > 1 {
            left += "   ";
            left += &crate::i18n::tn(selected, "{n} selected", "{n} selected");
        }
        winui::fit(
            ui,
            p,
            o.x + 8.0,
            baseline,
            &left,
            winui::MUTED(),
            o.x + w / 2.0,
            "strip-left",
        );
        // A list file Import is reading (#805), before what runs.
        let reading = self.importing.pending().next().map(|path| {
            let file = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            crate::i18n::fmt(
                // i18n: the status strip while Tasks → Import reads a file; {file} is its name
                crate::i18n::t("Reading {file}..."),
                &[("file", &crate::i18n::isolate(&file))],
            )
        });
        let right = match (reading, self.activity()) {
            (Some(r), Some(a)) => Some(format!("{r}   {a}")),
            (r, a) => r.or(a),
        };
        if let Some(right) = right {
            winui::text_right(p, o.x + w - 10.0, baseline, &right, winui::MUTED());
        }
    }

    /// "2 downloading, 1.25 MB/sec" while downloads run (the status strip, the tray's tip).
    fn activity(&self) -> Option<String> {
        let running: Vec<&Item> = self.items.iter().filter(|i| i.live.is_some()).collect();
        if running.is_empty() {
            return None;
        }
        let speed: f64 = running.iter().map(|i| i.rate()).sum();
        Some(crate::i18n::fmt(
            // i18n: {count} = "2 downloading"; {speed} = all of them together, like "1.25 MB/sec"
            crate::i18n::t("{count}, {speed}"),
            &[
                (
                    "count",
                    &crate::i18n::tn(running.len() as u64, "{n} downloading", "{n} downloading"),
                ),
                ("speed", &model::rate(speed)),
            ],
        ))
    }

    /// The list's rows as drawn: the view's downloads in the list's order that pass the column
    /// filters (Find, Select all and scrolling count these, so they match what is shown).
    fn shown_ids(&self) -> Vec<u64> {
        if self.list.filters.iter().all(Option::is_none) {
            return self.view_ids();
        }
        self.list_rows().0
    }

    /// The rows the list draws ([`Self::shown_ids`]) with their cells.
    fn list_rows(&self) -> (Vec<u64>, Vec<list::Row>) {
        let queued: std::collections::HashSet<u64> = self
            .queues
            .iter()
            .flat_map(|q| q.items.iter().copied())
            .collect();
        self.view_items()
            .into_iter()
            .map(|it| (it.id, self.row(it, &queued)))
            .filter(|(_, r)| list::matches(&self.list, r))
            .unzip()
    }

    /// One download's row in the list.
    /// `queued`: the downloads in a queue (worked out once for all the rows).
    fn row(&self, it: &Item, queued: &std::collections::HashSet<u64>) -> list::Row {
        list::Row {
            status_group: match it.status {
                // i18n: a download's status (state it is in, not a command): being downloaded now
                Status::Downloading => "Downloading",
                // i18n: a download's status (state it is in, not a command): stopped by the user
                Status::Stopped => "Stopped",
                // i18n: a download's status (state it is in, not a command): waiting in a queue for its turn
                Status::Queued => "Waiting",
                // i18n: a download's status (state it is in, not a command): finished, the whole file is there
                Status::Complete => "Complete",
                // i18n: a download's status (state it is in, not a command): it failed
                Status::Error => "Error",
            },
            icon: file_icon(&it.file_name),
            queued: queued.contains(&it.id),
            cells: [
                it.file_name.clone(),
                String::new(),
                it.size.map_or(String::new(), model::size_list),
                if self.extracts.extracting(it.id) {
                    // i18n: a download's status: its archive is being extracted (#37)
                    crate::i18n::t("Extracting...").to_owned()
                } else {
                    it.status_text()
                },
                it.time_left(),
                // Every byte in (#736): no rate, as once it is complete.
                if it.live.is_some() && !it.past_last_byte() {
                    model::rate(it.rate())
                } else {
                    String::new()
                },
                it.last_try
                    .as_deref()
                    .map_or(String::new(), model::date_text),
                it.error.clone().unwrap_or_else(|| it.description.clone()),
                it.added.as_deref().map_or(String::new(), model::date_text),
                // A torrent through a debrid account is a torrent too (#44).
                if it.torrent.is_some() || it.remote.is_some() {
                    // i18n: where a download comes from: BitTorrent (shown on a small colored label)
                    "Torrent".into()
                } else {
                    // i18n: where a download comes from: a web or FTP server (shown on a small colored label)
                    "Web".into()
                },
                // A torrent's own numbers (#357), in their slots' order (list::PEERS, RATIO, then
                // UPLOAD, added last so saved layouts keep their meaning): the peers it has, what
                // it gave back against what it took (as the General tab works it out), what it sends.
                it.torrent
                    .as_ref()
                    .map_or(String::new(), |t| crate::i18n::integer(t.peers as u64)),
                it.torrent.as_ref().map_or(String::new(), |t| {
                    crate::torrents::share_ratio_text(t.uploaded, it.downloaded)
                }),
                it.torrent
                    .as_ref()
                    .map_or(String::new(), |t| model::rate(t.up_speed)),
            ],
            // A short word and a percentage in the cell while checked or joined (#736): the
            // whole words in its tip and for a screen reader.
            status_whole: (!self.extracts.extracting(it.id))
                .then(|| it.finishing(false))
                .flatten(),
        }
    }

    /// The view's downloads in the list's order, before the column filters.
    fn view_ids(&self) -> Vec<u64> {
        self.view_items().iter().map(|i| i.id).collect()
    }

    /// [`Self::view_ids`]'s downloads themselves (each sort key worked out once, not at
    /// every comparison).
    fn view_items(&self) -> Vec<&Item> {
        let shown = self.items.iter().filter(|i| self.in_view(self.view, i));
        let Some((col, asc)) = self.list.sort else {
            return shown.collect();
        };
        let queued: std::collections::HashSet<u64> = self
            .queues
            .iter()
            .flat_map(|q| q.items.iter().copied())
            .collect();
        let mut keyed: Vec<((f64, String), &Item)> = shown
            .map(|i| (sort_key(i, col, queued.contains(&i.id)), i))
            .collect();
        keyed.sort_by(|(ka, _), (kb, _)| {
            let ord = ka.0.total_cmp(&kb.0).then_with(|| ka.1.cmp(&kb.1));
            if asc { ord } else { ord.reverse() }
        });
        keyed.into_iter().map(|(_, i)| i).collect()
    }

    /// Does `view` (a node of the Categories tree) hold this download?
    fn in_view(&self, view: View, i: &Item) -> bool {
        let in_cat = |c: Option<Category>| c.is_none_or(|c| i.category == c);
        match view {
            View::All => true,
            View::Cat(c) => i.category == c,
            View::Unfinished(c) => i.status != Status::Complete && in_cat(c),
            View::Finished(c) => i.status == Status::Complete && in_cat(c),
            View::Queue(q) => self.queues.get(q).is_some_and(|q| q.items.contains(&i.id)),
        }
    }

    // ---- actions ---------------------------------------------------------------------------

    fn add(&mut self, fi: &mut FileInfo, start: Start) {
        // Only a folder other than the category's becomes its new one (an unchanged folder
        // stays "the default", following Downloads).
        if let Some((cat, dir)) = fi.remembered() {
            self.settings
                .keep_category_dir(cat, std::path::Path::new(&dir), &self.downloads);
        }
        let id = self.push_item(
            fi.url.clone(),
            fi.dest(),
            fi.category,
            fi.description.clone(),
            fi.size(),
            fi.login.clone(),
            fi.context.clone(),
            if fi.context.referer.is_some() || fi.context.cookies.is_some() {
                crate::i18n::mark("from the browser")
            } else {
                crate::i18n::mark("with Add URL")
            },
        );
        self.name_guessed(id, fi.name_guessed());
        self.set_direct(id, fi.direct);
        self.number_at_start(id);
        self.selection.set(id);
        if matches!(start, Start::Now) {
            // Later: the answer is closed (its download starts some other time).
            if let Some(k) = fi.take_kept() {
                self.kept_answers.insert(id, k);
            }
            self.start(id, true);
        } else {
            // "Download Later": the main download queue (IDM's default), or the one picked.
            self.queue_later(vec![id], self.settings.queue_panel_later);
        }
        self.dirty = true;
    }

    /// Options → Downloads → "Start downloading immediately while displaying Download File
    /// Info": once the address answered (name and size known) and its folder exists, the
    /// download starts behind the dialog; the dialog's answer then keeps, moves or drops it.
    fn start_early(&mut self, fi: &mut FileInfo) {
        if !self.settings.start_immediately || fi.early.is_some() || !fi.probed() {
            return;
        }
        // Once Save As has been looked at off the window's thread (#743): its name numbered in
        // its folder, and the folder there. A folder still to be created waits for the dialog's
        // question; a look still under way, for its answer.
        if fi.folder_found() != Some(true) {
            return;
        }
        let dest = fi.dest();
        let id = self.push_item(
            fi.url.clone(),
            dest,
            fi.category,
            fi.description.clone(),
            fi.size(),
            fi.login.clone(),
            fi.context.clone(),
            if fi.context.referer.is_some() || fi.context.cookies.is_some() {
                crate::i18n::mark("from the browser")
            } else {
                crate::i18n::mark("with Add URL")
            },
        );
        self.set_direct(id, fi.direct);
        self.number_at_start(id);
        fi.early = Some(id);
        if let Some(k) = fi.take_kept() {
            self.kept_answers.insert(id, k);
        }
        self.start(id, false);
    }

    /// Download File Info closed with Start / Later on a download that started early.
    fn adopt_early(&mut self, fi: &FileInfo, id: u64, start: Start) {
        if let Some((cat, dir)) = fi.remembered() {
            self.settings
                .keep_category_dir(cat, std::path::Path::new(&dir), &self.downloads);
        }
        let dest = fi.dest();
        let now = matches!(start, Start::Now);
        let Some(item) = self.item(id) else { return };
        item.category = fi.category;
        item.description = fi.description.clone();
        let moved = item.dest != dest;
        self.selection.set(id);
        if moved {
            // Another name or folder than the one it started at: the data so far goes and it
            // starts over there.
            self.stop(id);
            self.after_stop.push((id, Some((dest, now))));
        } else if now {
            let (live, complete) = self
                .items
                .iter()
                .find(|i| i.id == id)
                .map_or((false, false), |i| {
                    (i.live.is_some(), i.status == Status::Complete)
                });
            if complete {
                // It finished behind the dialog: its box now.
                if self.settings.show_complete {
                    self.complete_window(id);
                }
            } else if !live {
                self.start(id, true);
            } else if !self.progress.iter().any(|p| p.id == id) {
                self.progress
                    .push(Progress::new(id, self.settings.show_complete));
            }
        } else {
            // Later: paused, its data kept for the queue to resume.
            self.stop(id);
        }
        if !now {
            self.queue_later(vec![id], self.settings.queue_panel_later);
        }
        self.dirty = true;
    }

    /// Carry out `after_stop` for downloads that have stopped.
    fn finish_stopped(&mut self) {
        if self.after_stop.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.after_stop);
        for (id, then) in pending {
            if self.item(id).is_some_and(|i| i.live.is_some()) {
                self.after_stop.push((id, then));
                continue;
            }
            let Some(item) = self.item(id) else { continue };
            let old = item.dest.clone();
            match then {
                None => {
                    // Cancelled: a file that finished meanwhile goes too, with its windows. On
                    // another thread: a slow or stuck disk took the window with it (#423).
                    let complete = item.status == Status::Complete;
                    self.rt.spawn_blocking({
                        let old = old.clone();
                        move || remove_files_of(&old, complete)
                    });
                    self.complete.retain(|c| c.id != id);
                    self.delete(id);
                }
                Some((dest, start)) => {
                    // A restart under another name: the old files go off the window thread
                    // (#423). Under the same name they must be gone before the new start.
                    let complete = item.status == Status::Complete;
                    if dest == old {
                        remove_files_of(&old, complete);
                    } else {
                        let old = old.clone();
                        self.rt
                            .spawn_blocking(move || remove_files_of(&old, complete));
                    }
                    let item = self.item(id).expect("the download just found");
                    if let Some(name) = dest.file_name() {
                        item.file_name = name.to_string_lossy().into_owned();
                    }
                    item.dest = dest;
                    item.downloaded = 0;
                    item.status = Status::Stopped;
                    // Download File Info's place, given without waiting for the disk (#743).
                    item.number_at_start = true;
                    if start {
                        self.start(id, true);
                    }
                }
            }
            self.dirty = true;
        }
    }

    /// Downloads added with Later (or by a batch): into the main queue, or the queue chosen in
    /// the selection panel when `ask` (Options → Downloads).
    fn queue_later(&mut self, ids: Vec<u64>, ask: bool) {
        if ids.is_empty() {
            return;
        }
        if ask && self.queue_pick.is_none() {
            let names = self.queues.iter().map(|q| q.name.clone()).collect();
            self.queue_pick = Some((ids, dialogs::QueuePick::new(names)));
        } else {
            self.queues[0].items.extend(ids);
        }
        self.dirty = true;
    }

    /// Downloads added together (a playlist, a batch, Link grabber's links) become a group
    /// (#766): one summary once all are done. A single one stays alone.
    fn group(&mut self, ids: &[u64]) {
        let Some(g) = crate::finish::group_of(ids) else {
            return;
        };
        let ids: std::collections::HashSet<u64> = ids.iter().copied().collect();
        for i in self.items.iter_mut().filter(|i| ids.contains(&i.id)) {
            i.group = Some(g);
        }
        self.dirty = true;
    }

    #[allow(clippy::too_many_arguments)]
    fn push_item(
        &mut self,
        url: url::Url,
        dest: PathBuf,
        category: Category,
        description: String,
        size: Option<u64>,
        login: Option<(String, String)>,
        context: ipc::Command,
        source: &str,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut item = Item::new(id, url, dest, category);
        item.note(crate::i18n::fmt(
            crate::i18n::t("Added {source}"),
            &[("source", crate::i18n::t(source))],
        ));
        item.size = size;
        item.description = description;
        item.login = login;
        // The browser had started it: the host is already sending the file (#44).
        item.direct = crate::debrid::browser_started(&context);
        item.referer = context.referer.clone().filter(|r| !r.is_empty());
        item.context = context;
        self.items.push(item);
        self.dirty = true;
        id
    }

    /// Download `id` skips the debrid accounts, or goes through them again (#44).
    fn set_direct(&mut self, id: u64, direct: bool) {
        if let Some(item) = self.item(id) {
            item.direct = direct;
        }
    }

    /// Download `id`'s name was given without waiting for its folder to be looked at (Download
    /// File Info, #743): it starts under it, or the next free numbered name if it is taken by
    /// then (`Item::number_at_start`).
    fn number_at_start(&mut self, id: u64) {
        if let Some(item) = self.item(id) {
            item.number_at_start = true;
        }
    }

    /// Download `id`'s name is only its address's guess (`guessed`): the server names it when
    /// it starts (`Item::name_from_url`).
    fn name_guessed(&mut self, id: u64, guessed: bool) {
        if guessed && let Some(item) = self.item(id) {
            item.name_from_url = true;
        }
    }

    /// Each frame (#35): the route connections take, from the adapter watch. When it changes,
    /// everything running stops (its connections were held to the route before, or to none;
    /// sockets that are bound can't leak, this is so they don't wait for a timeout) and the
    /// torrent session with it; what was stopped so starts again once the route lets it.
    /// Each frame (#44): the debrid accounts set up again when they, or MDM's network settings
    /// (the API calls take the same proxy, DNS and adapter), changed; then the services' host
    /// lists read, off the window's thread. No account: no request at all.
    fn debrid_tick(&mut self) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for a in self.settings.debrid_accounts.iter().filter(|a| a.enabled) {
            (a.id, a.service, &a.secret).hash(&mut h);
        }
        format!("{:?}", crate::settings::own_net().proxy).hash(&mut h);
        self.settings.dns().hash(&mut h);
        self.route.hash(&mut h);
        let from = h.finish();
        if from == self.debrid_from {
            return;
        }
        self.debrid_from = from;
        let accounts: Vec<mdm_debrid::Account> = self
            .settings
            .debrid_accounts
            .iter()
            .filter(|a| a.enabled)
            .filter_map(crate::settings::DebridAccount::account)
            .collect();
        let Ok(http) = crate::debrid::http() else {
            return;
        };
        let any = !accounts.is_empty();
        self.debrid.set_accounts(accounts, http);
        if any {
            let debrid = self.debrid.clone();
            self.rt.spawn(async move {
                if let Err(e) = debrid.refresh_hosts().await {
                    tracing::warn!("debrid host lists: {e:?}");
                }
            });
        }
    }

    /// Which account downloads go through (#44), for the windows that show and change it.
    pub fn debrid_route(&self) -> crate::debrid::Route {
        crate::debrid::Route::new(&self.debrid, &self.settings)
    }

    fn network_adapter_tick(&mut self, ctx: &egui::Context) {
        let route = self.netwatch.route(&self.settings.net_adapter, ctx);
        crate::settings::share_route(route.clone());
        if route != self.route {
            let before = std::mem::replace(&mut self.route, route.clone());
            self.route_changed(&before);
        }
        if matches!(self.route, mdm_engine::Route::Down { .. }) || self.adapter_paused.is_empty() {
            return;
        }
        // Started again once their jobs have ended.
        let ready: Vec<u64> = self
            .adapter_paused
            .iter()
            .copied()
            .filter(|id| self.items.iter().any(|i| i.id == *id && i.live.is_none()))
            .collect();
        self.adapter_paused
            .retain(|id| self.items.iter().any(|i| i.id == *id) && !ready.contains(id));
        for id in ready {
            self.start(id, false);
        }
    }

    fn route_changed(&mut self, before: &mdm_engine::Route) {
        let note = match &self.route {
            mdm_engine::Route::Down { label } => crate::i18n::fmt(
                // i18n: event log entry: the download stopped because the chosen network adapter (a VPN's, usually) went away; {adapter} its name
                crate::i18n::t("Stopped: {adapter} is not connected; it goes on when it is back"),
                &[("adapter", label)],
            ),
            // i18n: event log entry: the download stopped to start again through the network adapter as it is now (a VPN reconnected, another adapter chosen)
            _ => crate::i18n::t("Stopped: the network adapter changed; starting again").to_owned(),
        };
        let mut stopped = Vec::new();
        for i in &mut self.items {
            if let Some(l) = &i.live {
                l.dl.cancel();
                i.note(note.clone());
                stopped.push(i.id);
            }
        }
        for id in stopped {
            if !self.adapter_paused.contains(&id) {
                self.adapter_paused.push(id);
            }
        }
        // The torrent session's sockets were bound as things were.
        if !before.is_any() || !self.route.is_any() {
            self.torrent_host.stop_now(&self.rt);
        }
        self.dirty = true;
    }

    /// Every 2 s (#40): Torrent settings for the next torrent session, and the running one stopped
    /// once no torrent has needed it for a minute (none running or sharing, no Add torrent open),
    /// when Torrent settings say so or their network settings changed.
    /// What each torrent sent and how long it shared, added to its `TorrentSpec` (librqbit
    /// counts only the current run); and the own upload limit of those sharing without a `Run`
    /// (a `Run` sets its own). Saved with the list at most every 10 s.
    fn torrent_accounts(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.account_at).as_secs();
        // Whole seconds only: the rest carries to the next tick.
        if dt > 0 {
            self.account_at += Duration::from_secs(dt);
        }
        let Some(t) = self.torrent_host.get() else {
            // No session: no check is running, whatever the last numbers said.
            self.torrent_resync.clear();
            for i in self.items.iter_mut() {
                i.torrent_check = None;
            }
            crate::torrents::clear_live(self.items.iter_mut().filter_map(|i| i.torrent.as_mut()));
            return;
        };
        let limits = self.settings.sharing_limits();
        let mut changed = false;
        let mut hashes = std::collections::HashSet::new();
        for i in self.items.iter_mut() {
            let Some(spec) = i.torrent.as_mut() else {
                continue;
            };
            hashes.insert(spec.hash.clone());
            // As last read: the window's thread never waits on librqbit (#693).
            let Some(st) = t.seen(&spec.hash) else {
                // Out of the session (released when it finished): no check is running.
                i.torrent_check = None;
                spec.peers = 0;
                spec.up_speed = 0.0;
                continue;
            };
            // Keep the session's in-memory checking progress visible for stopped/complete
            // torrents too; normal download progress is restored when initialization ends.
            i.torrent_check = (st.phase == mdm_torrent::Phase::Checking)
                .then_some((st.checked.unwrap_or(st.done), st.total));
            let sent = self.accountant.sent(&spec.hash, st.uploaded);
            if sent > 0 {
                spec.uploaded += sent;
                changed = true;
            }
            // The peers it has now, for the list's "Peers" column (#357). Not a reason to save:
            // it changes as peers come and go, and the next tick brings the list up to date.
            spec.peers = st.peers_live;
            spec.up_speed = st.up_speed;
            if st.phase == mdm_torrent::Phase::Finished && dt > 0 {
                spec.seeded_secs += dt;
                changed = true;
            }
            // Torrent settings' seeding limits: a finished torrent over one stops being shared
            // (paused; it stays so after a restart, `complete_torrents`), with or without a Run.
            let mut stopped = None;
            if st.phase == mdm_torrent::Phase::Finished
                && let Some(why) = crate::torrents::sharing_limit_reached(spec, st.done, limits)
            {
                let (t2, hash) = (t.clone(), spec.hash.clone());
                self.rt.spawn(async move {
                    let _ = t2.remove(&hash, false).await;
                });
                stopped = Some(why);
                changed = true;
            }
            let (hash, up) = (spec.hash.clone(), spec.upload_limit);
            let stop_after_recheck = !self.settings.torrent_keep_sharing
                || crate::torrents::sharing_limit_reached(spec, st.done, limits).is_some();
            if i.live.is_none() {
                let _ = t.set_torrent_limits(&hash, None, up);
            }
            if let Some(why) = stopped {
                i.note(sharing_stopped_note(why));
            }
            // After a Recheck or a change of files (`torrent_outcomes`), once the session has
            // checked it: the list's numbers are the session's. One not running in the list
            // takes its standing from them; one no longer complete is held paused (shared, it
            // would fetch what is missing out of sight).
            if st.phase != mdm_torrent::Phase::Checking
                && !self.torrent_host.busy(&hash)
                && let Some(report) = self.torrent_resync.remove(&hash)
            {
                if report {
                    i.note(crate::i18n::fmt(
                        // i18n: event log entry after Recheck: {have} and {total} are sizes ("1.2 GB of 4 GB there")
                        crate::i18n::t("Checked: {have} of {total} there"),
                        &[
                            ("have", &model::size(st.done)),
                            ("total", &model::size(st.total)),
                        ],
                    ));
                }
                if i.live.is_none() {
                    i.downloaded = st.done;
                    i.size = Some(st.total);
                    let whole = st.total > 0 && st.done >= st.total;
                    if whole {
                        i.status = Status::Complete;
                        if report && i.live.is_none() && stop_after_recheck {
                            let t2 = t.clone();
                            self.rt.spawn(async move {
                                let _ = t2.remove(&hash, false).await;
                            });
                        }
                    } else if i.status == Status::Complete {
                        i.status = Status::Stopped;
                        let t2 = t.clone();
                        self.rt.spawn(async move {
                            let _ = t2.pause(&hash).await;
                        });
                    }
                }
                self.dirty = true;
            }
        }
        self.accountant.keep(&hashes);
        if changed && self.account_saved.elapsed() >= Duration::from_secs(10) {
            self.account_saved = now;
            self.dirty = true;
        }
    }

    fn torrent_tick(&mut self) {
        let cfg = crate::torrents::state_dir().and_then(|d| self.settings.torrent_config(d));
        self.torrent_host.set_config(cfg);
        self.torrent_host
            .set_upload(self.settings.torrent_upload_limit());
        self.torrent_outcomes();
        self.torrent_accounts();
        self.rss_tick();
        if !self.settings.torrent_blocklist.trim().is_empty() {
            crate::torrents::refresh_blocklist(
                &self.rt,
                &self.settings.torrent_blocklist,
                &self.blocklist_busy,
            );
        }
        let sharing = self.settings.torrent_keep_sharing
            && self
                .items
                .iter()
                .any(|i| i.torrent.is_some() && i.status == Status::Complete);
        // A torrent being checked (a Recheck of one that isn't running, its action still going,
        // or its result not read yet) needs the session too: stopped as idle after a minute,
        // the check died where it was and the list said "Checking 17%" for good.
        let checking = !self.torrent_resync.is_empty()
            || self.items.iter().any(|i| {
                i.torrent_check.is_some()
                    || i.torrent
                        .as_ref()
                        .is_some_and(|t| self.torrent_host.busy(&t.hash))
            });
        let busy = sharing
            || checking
            || self.torrent_dlg.is_some()
            || self
                .items
                .iter()
                .any(|i| i.torrent.is_some() && i.live.is_some());
        if busy {
            self.torrent_idle_since = None;
            return;
        }
        let since = *self.torrent_idle_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= Duration::from_secs(60) {
            self.torrent_host
                .stop_if_idle(&self.rt, true, self.settings.torrent_stop_idle);
        }
    }

    /// `/show <window>` (mdm_ipc::Command::show): open one of the windows by name, as its menu
    /// command does. Unknown names do nothing.
    fn show_named(&mut self, name: &str, ctx: &egui::Context) {
        self.show_window(ctx);
        let first = self.items.first().map(|i| i.id);
        let last = self.items.last().map(|i| i.id);
        let (window, tab) = name.split_once(':').unwrap_or((name, ""));
        match window {
            "addurl" => self.run(Cmd::AddUrl, None, ctx),
            // MDM_CRASH_TEST=1 only (crash.rs): the crash reports' own check.
            "crash-test" => crate::crash::test(tab),
            "batch" => self.run(Cmd::Batch, None, ctx),
            // Tasks → Add batch download from clipboard (#709's checks: off-screen, the
            // clipboard is MDM_TEST_CLIPBOARD's file).
            "batch-clipboard" => self.run(Cmd::BatchClipboard, None, ctx),
            "find" => self.run(Cmd::Find, None, ctx),
            "scheduler" => self.run(Cmd::Scheduler, None, ctx),
            "about" => self.run(Cmd::About, None, ctx),
            // Update now's end-to-end tests (feature `test-update`, never in a release): Update
            // now pressed in the window `/show updates` opened (against MDM_UPDATE_FEED).
            #[cfg(feature = "test-update")]
            "update-now" => {
                if let Some(u) = &mut self.updates {
                    u.press_update_now();
                }
            }
            // The main window into the tray, as closing it does (tests and #72's measurements).
            "tray" => self.hide_to_tray(ctx),
            // Off-screen test copies: the drop target's menu, as a right-click on it opens it
            // (#208's checks; a posted click has no pointer position by the time it is seen).
            "drop-target-menu" if crate::offscreen() => {
                if let Some(at) = self.drop_target.as_ref().map(|d| d.center()) {
                    self.drop_target_menu(at, ctx);
                }
            }
            // The main window minimized, not to the tray (tests of #162).
            "minimized" => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            // Off-screen test copies: the interface zoomed (`zoom:1.5`) while running, as a
            // display's scale changing does to every window (#89's checks).
            "zoom" if crate::offscreen() => {
                if let Ok(z) = tab.parse::<f32>()
                    && (0.5..=4.0).contains(&z)
                {
                    ctx.set_zoom_factor(z);
                }
            }
            // The system's graphics API's device lost on purpose (Direct3D, Vulkan, Metal); MDM
            // starts again (renderer.rs).
            #[cfg(gpu)]
            "renderer-lost" if crate::offscreen() => crate::renderer::test_device_lost(),
            "helpers" => self.helpers = Some(crate::helpers::Helpers::new()),
            "torrent-settings" => self.run(Cmd::TorrentSettings, None, ctx),
            // The first download's: Add trackers (a torrent), its right-click menu.
            "add-trackers" if first.is_some() => self.run(Cmd::TorrentAddTrackers, first, ctx),
            // rss:rules opens it on its Download rules tab.
            "rss" => self.run(Cmd::RssFeeds(tab == "rules"), None, ctx),
            // menu:torrent opens it with its Torrent submenu open. A number (menu:3) is the
            // menu bar's, below, with downloads in the list or not.
            "menu" if first.is_some() && !tab.starts_with(|c: char| c.is_ascii_digit()) => {
                if let Some(id) = first {
                    self.selection.set(id);
                }
                self.open_popup(Popup::Item(first), Pos2::new(240.0, 150.0));
                let sub = self
                    .item_entries(first)
                    .iter()
                    .position(|e| matches!(e, MenuEntry::Sub("Torrent", ..)));
                if let (Some(i), "torrent") = (sub, tab) {
                    let id = format!("popup-{}", self.popup_serial);
                    chrome::open_sub_at(ctx, &id, i);
                }
            }
            // category:Music / category-delete:Music — the Categories right-click menu's
            // Properties and Delete for a category by name (#93; the docs' shots and checks);
            // category-remove:Music opens Delete with "Remove its downloads" chosen.
            "category" | "category-delete" | "category-remove" => {
                if let Some(c) = Category::all()
                    .into_iter()
                    // `/show` arrives in lower case (mdm_ipc::parse_args).
                    .find(|c| c.key().eq_ignore_ascii_case(tab) || c.shown().to_lowercase() == tab)
                {
                    let cmd = if window == "category" {
                        Cmd::CategoryProps(c)
                    } else {
                        Cmd::DeleteCategory(c)
                    };
                    self.run(cmd, None, ctx);
                    if window == "category-remove" {
                        self.confirm_category_delete =
                            self.confirm_category_delete.take().map(|d| d.removing());
                    }
                }
            }
            "video-page" => self.run(Cmd::VideoPage, None, ctx),
            // Download video from a page → Helpers... (#51).
            "video-helpers" => {
                self.run(Cmd::VideoPage, None, ctx);
                if let Some(v) = &mut self.video_dlg {
                    v.open_helpers();
                }
            }
            // Download video from a page with a sample video's subtitles (#68).
            // `video-subs:flac`: with that choice picked (screenshots of the lower choices).
            "video-subs" => {
                self.run(Cmd::VideoPage, None, ctx);
                if let Some(v) = &mut self.video_dlg {
                    v.show_sample();
                    if !tab.is_empty() {
                        v.pick(tab);
                    }
                }
            }
            // A page whose videos need ffmpeg (#351): without it the list has only the sound.
            // `video-adaptive:runtime`: yt-dlp said it had no JavaScript runtime (#852).
            "video-adaptive" => {
                self.run(Cmd::VideoPage, None, ctx);
                if let Some(v) = &mut self.video_dlg {
                    v.show_sample_adaptive(tab == "runtime");
                }
            }
            // A sample playlist (#69), its quality by its place in the list: `video-playlist:7`.
            "video-playlist" => {
                self.run(Cmd::VideoPage, None, ctx);
                if let Some(v) = &mut self.video_dlg {
                    v.show_sample_list(tab);
                }
            }
            "existing-files" => {
                // In General's folder (a test's own, set in its data folder), not the system's
                // Downloads, whose path names the user.
                let dir = self
                    .settings
                    .category_dir(Category::General, &self.downloads)
                    .join("Big Buck Bunny");
                self.shown_existing = Some(crate::dialogs::ExistingFiles::new(&[
                    dir.join("big_buck_bunny_1080p.mkv"),
                    dir.join("subtitles.srt"),
                    dir.join("poster.jpg"),
                ]));
            }
            "extension" => self.extension = Some(crate::extension::Extension::new()),
            // The Safari row's "How to Add" steps, open (screenshots).
            "safari-steps" => {
                let mut offer = crate::extension::Extension::new();
                offer.open_safari_steps();
                self.extension = Some(offer);
            }
            "language" => self.run(Cmd::ChooseLanguage, None, ctx),
            "move-rename" => self.run(Cmd::MoveRename, None, ctx),
            "shortcuts" => self.run(Cmd::Shortcuts, None, ctx),
            // The Help menu's windows and Download a gallery, for screenshots (#192). updates
            // asks GitHub, as Help → Check for updates does.
            "tell-friend" => self.run(Cmd::TellFriend, None, ctx),
            "updates" => self.run(Cmd::CheckUpdates, None, ctx),
            "checksum" if first.is_some() => self.run(Cmd::VerifyChecksum, first, ctx),
            "gallery" => {
                let dir = self
                    .settings
                    .category_dir(Category::General, &self.downloads);
                self.gallery_dlg = Some(crate::gallerydlg::GalleryDialog::new(
                    dir,
                    "https://commons.wikimedia.org/wiki/Category:Sunsets",
                ));
            }
            "speedlimit" => self.run(Cmd::LimiterSettings, None, ctx),
            "properties" if first.is_some() => self.run(Cmd::Properties, first, ctx),
            "delete" if first.is_some() => self.run(Cmd::Remove, first, ctx),
            "progress" => {
                if let Some(id) = last
                    && !self.progress.iter().any(|p| p.id == id)
                {
                    self.progress
                        .push(Progress::new(id, self.settings.show_complete));
                }
                // progress:peers and so on: a torrent's details tab (the docs' screenshots).
                if let Some(id) = last
                    && let Some(p) = self.progress.iter_mut().find(|p| p.id == id)
                {
                    p.show_tab(tab);
                }
            }
            // View → Theme → Add a theme file, as its dialog answers (#45):
            // `theme-add:<path>` (tests of a scheme's import without the system's dialog).
            "theme-add" => {
                self.add_theme_file(std::path::Path::new(tab), ctx);
            }
            // Tasks → Import, as its dialog answers: `import:<path>` (tests of a large or wrong
            // file without the system's dialog, #805).
            "import" => self.import_file(std::path::Path::new(tab)),
            // View → Theme → a theme, as the menu picks it (#45): `theme:native` (tests of the
            // live switch without a click).
            "theme" => {
                let id = crate::chrome::label(tab);
                self.run(Cmd::Theme(id), None, ctx);
            }
            // A menu of the menu bar open (menu:0 is Tasks), for screenshots; menu:3:draw opens
            // View with its Draw with submenu (#89).
            "menu" => {
                self.popup = None;
                let (index, sub) = tab.split_once(':').unwrap_or((tab, ""));
                let index = index.parse().unwrap_or(0);
                self.open_menu = Some(index);
                // menu:3:theme opens View → Theme (#45), menu:3:size View → Interface size (#262).
                let wanted = match sub {
                    "draw" => "Draw with",
                    "theme" => "Theme",
                    "size" => "&Interface size",
                    _ => "",
                };
                let at = self
                    .menu_entries(index)
                    .iter()
                    .position(|e| matches!(e, MenuEntry::Sub(name, ..) if *name == wanted));
                if let Some(at) = at {
                    chrome::open_sub_at(ctx, &format!("menu-{index}"), at);
                }
            }
            // Every download selected and the right-click menu open on them (Move/Rename, #333).
            // The list's right-click menu on blank space (#926), for screenshots.
            "context" if tab == "blank" => {
                self.open_menu = None;
                self.open_popup(Popup::Item(None), egui::pos2(360.0, 220.0));
            }
            "context" if tab == "all" => {
                self.run(Cmd::SelectAll, None, ctx);
                self.open_menu = None;
                if let Some(id) = self.shown_ids().first().copied() {
                    self.open_popup(Popup::Item(Some(id)), egui::pos2(360.0, 220.0));
                }
            }
            // A finished archive's right-click menu (Extract, #37), for screenshots.
            "context" => {
                let archive = self
                    .items
                    .iter()
                    .rev()
                    .find(|i| {
                        i.status == Status::Complete
                            && crate::extractq::archive_name(&i.file_name)
                            && mdm_extract::volume_key(&i.file_name).is_none_or(|v| v.is_first())
                    })
                    .or(self.items.last())
                    .map(|i| i.id);
                self.open_menu = None;
                if let Some(id) = archive {
                    self.selection.set(id);
                    self.open_popup(Popup::Item(Some(id)), egui::pos2(360.0, 220.0));
                }
            }
            // Link grabber with a page's links, looked up (the docs' screenshots, #37).
            "grabber" => {
                self.show_import(Vec::new(), String::new());
                if let Some(g) = &mut self.grabber {
                    g.sample();
                }
            }
            "options" => {
                let page = match tab {
                    "types" => 1,
                    "saveto" => 2,
                    "downloads" => 3,
                    "connection" => 4,
                    "proxy" => 5,
                    "logins" => 6,
                    "accounts" => 7,
                    "archives" => 10,
                    "sounds" => 8,
                    "regional" => 9,
                    "remote" | "pair" => 11,
                    _ => 0,
                };
                self.run(Cmd::Options, None, ctx);
                self.options = self.options.take().map(|o| o.on_page(page));
                self.raise_options = true;
                // General → Download panels → Edit... (the video button's settings).
                if tab == "panels" {
                    self.options = self.options.take().map(|o| o.with_panels());
                }
                // A window over a page (screenshots, #192): options:test, options:account.
                self.options = self.options.take().map(|o| o.with_window(tab));
                // Remote → Pair a device... (the docs' screenshots, #141).
                if tab == "pair" {
                    self.options = self.options.take().map(|o| o.with_pairing());
                }
            }
            _ => {}
        }
    }

    /// A command from the command line, a second launch, or the browser (ipc.rs).
    fn handle_command(&mut self, cmd: ipc::Command, ctx: &egui::Context) {
        if cmd.exit {
            self.run(Cmd::Exit, None, ctx);
            return;
        }
        // Without a window (#157): an address, magnet or .torrent is added as a paired device's
        // is (no Download File Info, no Add torrent); nothing else opens anything.
        if self.headless {
            if cmd.start_queue {
                self.start_queue(0);
            }
            if cmd.url.is_some() {
                let answer = self.remote_add(cmd);
                tracing::info!("headless: added: {answer}");
            }
            return;
        }
        // The startup entry's (#226): this start began in the tray (main.rs), or MDM already
        // ran; either way the window stays where it is.
        if cmd.startup {
            return;
        }
        if let Some(name) = cmd.show.as_deref() {
            self.show_window(ctx);
            self.show_named(name, ctx);
            return;
        }
        // The download button over a Google Drive file's player: the file itself, as it was
        // uploaded (segmented and resumable), not the player's re-encoded streams (#34).
        let mut cmd = cmd;
        if let Some(file) = drive_file_behind_player(&cmd) {
            cmd.video_page = None;
            cmd.url = Some(file);
            // Drive names its file itself; its page's title ("… - Google Drive") doesn't.
            cmd.video_title = None;
        }
        // The button over a video that plays a plain file (#703): downloaded as any file, at
        // once and without a window (its page the referrer), as Download File Info's Start would.
        if cmd.video_direct
            && let Some(file) = cmd.video_page.take()
        {
            cmd.url = Some(file);
            cmd.silent = true;
        }
        // The browser's download button over a video: the Download video window, looking the
        // page up at once. The main window stays where it is (hidden, minimized): only the
        // dialog opens, as its window is its own.
        if let Some(page) = cmd.video_page.as_deref() {
            self.show_for_dialog(ctx);
            let page = page.to_owned();
            self.video_dlg = Some(self.video_dialog(Some((&page, cmd))));
            return;
        }
        // Its menu: never on this site.
        if let Some(host) = cmd.panel_hide_site.as_deref() {
            self.settings.video_panel_sites =
                mdm_ipc::capture::add_site(&self.settings.video_panel_sites, host);
            self.dirty = true;
            return;
        }
        // Its "Turn on": on this site again.
        if let Some(host) = cmd.panel_show_site.as_deref() {
            self.settings.video_panel_sites =
                mdm_ipc::capture::remove_site(&self.settings.video_panel_sites, host);
            self.dirty = true;
            return;
        }
        // Its "Show" after a download was added: the main window, that download selected.
        if let Some(address) = cmd.show_download.as_deref() {
            self.show_window(ctx);
            self.select_download_of(address);
            return;
        }
        // The browser's right-click menu: a page's (or a selection's) links, to tick in Import.
        if !cmd.links.is_empty() {
            self.show_for_dialog(ctx);
            let found = cmd
                .links
                .iter()
                .map(|url| crate::transfer::Imported {
                    url: url.clone(),
                    referer: cmd.referer.clone(),
                    cookies: page_data_for(url, cmd.referer.as_deref(), cmd.cookies.as_deref()),
                    user_agent: cmd.user_agent.clone(),
                    private: cmd.private,
                    ..Default::default()
                })
                .collect();
            // i18n: in Import, where the addresses came from: a page in the browser
            let source = crate::i18n::t("Links from the browser").to_owned();
            self.show_import(found, source);
            return;
        }
        // The browser's toolbar button: this site's downloads stay with the browser.
        if let Some(host) = cmd.exclude_site.as_deref() {
            self.settings.excluded_sites =
                mdm_ipc::capture::add_site(&self.settings.excluded_sites, host);
            self.dirty = true;
            return;
        }
        if cmd.start_queue {
            self.start_queue(0);
        }
        // A magnet link or a .torrent file (browser, command line, file association): Add
        // torrent shows what it holds.
        if let Some(m) = cmd.url.as_deref().filter(|u| {
            mdm_torrent::Source::magnet(u).is_some()
                || (u.to_ascii_lowercase().ends_with(".torrent")
                    && std::path::Path::new(u.trim_matches('"')).is_file())
        }) {
            let m = m.trim().to_owned();
            self.show_for_dialog(ctx);
            self.open_torrent_dialog(Some(m), ctx);
            return;
        }
        let url = cmd
            .url
            .as_deref()
            .and_then(|u| url::Url::parse(u.trim()).ok())
            .filter(mdm_engine::supported);
        let Some(url) = url else {
            if !cmd.start_queue {
                // A plain second launch brings the window to the front, as IDM does.
                self.show_window(ctx);
            }
            return;
        };
        // A .torrent on the web (#39; the browser's download of one): Add torrent fetches it with
        // the browser's request (its cookies) and shows what it holds. Silent (`/n`, a script's
        // command line) asks nothing: the .torrent itself is downloaded, as any file.
        let final_url = cmd
            .final_url
            .as_deref()
            .and_then(|u| url::Url::parse(u).ok());
        let torrent = |u: &url::Url| {
            crate::torrents::is_torrent_url(u, cmd.name.as_deref(), cmd.mime.as_deref())
        };
        if !cmd.silent && (torrent(&url) || final_url.as_ref().is_some_and(torrent)) {
            let parts = self.request_parts(&url, None, &cmd, crate::debrid::browser_started(&cmd));
            self.show_for_dialog(ctx);
            self.open_torrent_dialog_with(
                Some(url.to_string()),
                Some(crate::torrents::Remote { url, parts }),
                ctx,
            );
            return;
        }
        if !self.duplicate_gate(&url, || Pending::Command(Box::new(cmd.clone()))) {
            return;
        }
        // Options → Downloads → "Show start download dialog" off: start (or only queue) at once.
        let skip_dialog = !self.settings.show_start_dialog;
        if cmd.later || cmd.silent || skip_dialog {
            let guessed = cmd.name.is_none();
            let name = cmd
                .name
                .clone()
                .unwrap_or_else(|| crate::model::browser_guess(&url, &cmd));
            let name = mdm_engine::naming::sanitize(&name);
            // The automation API's `category` (by its key) and `queue` (by name), when given.
            let category = cmd
                .category
                .as_deref()
                .and_then(|k| Category::all().into_iter().find(|c| c.key() == k))
                .unwrap_or_else(|| Category::of(&name));
            let queue = cmd
                .queue
                .as_deref()
                .and_then(|n| self.queues.iter().position(|q| q.name == n))
                .unwrap_or(0);
            let dir = cmd
                .dir
                .clone()
                .map(PathBuf::from)
                .unwrap_or_else(|| self.settings.category_dir(category, &self.downloads));
            let dest = mdm_engine::naming::destination(&dir, &name);
            let later = cmd.later || (skip_dialog && self.settings.only_add_to_queue);
            let source = if cmd.referer.is_some() || cmd.cookies.is_some() {
                crate::i18n::mark("from the browser")
            } else {
                crate::i18n::mark("from the command line")
            };
            let size = cmd.size;
            let id = self.push_item(url, dest, category, String::new(), size, None, cmd, source);
            self.name_guessed(id, guessed);
            if later {
                self.queues[queue].items.push(id);
            } else {
                self.start(id, true);
            }
        } else {
            self.open_file_info(dialogs::NewUrl { url, login: None }, cmd, ctx);
        }
    }

    /// The headers and options `url`'s requests go with: the download's own `login`, else the
    /// one Options → Sites Logins has for its address, else an FTP address's own (its resume
    /// state keeps the address without the password); the browser's Referer, Cookie,
    /// User-Agent and form (`context`); Options' connections, proxy and User-Agent.
    ///
    /// Through the first debrid account whose service takes its host (#44), unless turned off
    /// or `direct` (`Item::direct`).
    pub fn request_parts(
        &self,
        url: &url::Url,
        login: Option<&(String, String)>,
        context: &ipc::Command,
        direct: bool,
    ) -> RequestParts {
        let site_login = self.settings.site_login(url);
        let url_login = mdm_engine::ftp::is_ftp(url)
            .then(|| url_login(url))
            .flatten();
        let mut headers = auth_headers(login.or(site_login.as_ref()).or(url_login.as_ref()));
        let post_type = context.post.as_ref().and(context.post_type.as_ref());
        let referer = context
            .referer
            .as_deref()
            .and_then(|r| sent_referrer(url, r));
        for (name, v) in [
            (reqwest::header::REFERER, &referer),
            // A video's file or stream, with the Origin its page's player sent (#900).
            (reqwest::header::ORIGIN, &context.origin),
            (reqwest::header::COOKIE, &context.cookies),
            (reqwest::header::CONTENT_TYPE, &post_type.cloned()),
        ] {
            if let Some(v) = v.as_deref().and_then(|v| HeaderValue::from_str(v).ok()) {
                headers.insert(name, v);
            }
        }
        // The browser's own User-Agent when it handed the download over, else the one set in
        // Options, else the automatic one (the default browser's, #668).
        let own = self.settings.own_user_agent();
        let given = context.user_agent.as_deref().or(own);
        let opts = Options {
            max_connections: self.settings.connections_for(url),
            proxy: self.settings.proxy_for(context),
            dns: self.settings.dns(),
            route: self.route.clone(),
            // Through a debrid account when one takes this host (#44).
            unlocker: self.debrid_route().unlocker(url, direct),
            global: Some(self.global.clone()),
            host_limits: Some(self.host_limits.clone()),
            ignore_mtime: self.settings.ignore_mtime,
            server_time: self.settings.set_creation_date,
            ftp_passive: self.settings.ftp_passive,
            user_agent: given.map_or_else(mdm_engine::agent::automatic, str::to_owned),
            automatic_agent: given.is_none(),
            // A browser's User-Agent (handed over, or the automatic one) may be refused on this
            // connection (#372): the one set in Options, else MDM's former one (Internet
            // Explorer 11's, which such bot checks let through), is tried once then. One the
            // user set is the user's choice: no other.
            agent_fallback: match (context.user_agent.is_some(), own) {
                (true, Some(own)) => Some(own.to_owned()),
                (_, None) => Some(mdm_engine::agent::IE11_USER_AGENT.to_owned()),
                (false, Some(_)) => None,
            },
            // Where the browser's own download of it ended: asked when the link itself is
            // refused (#630, a one-time or session-bound token the browser already used).
            browser_final: context
                .final_url
                .as_deref()
                .and_then(|u| url::Url::parse(u).ok())
                .filter(|f| f != url && mdm_engine::supported(f)),
            post: context.post.clone().map(|body| body.into_bytes().into()),
            on_file_created: Some(mark_hook(url, context.referer.as_deref())),
            // The browser's own download's type: a web page instead is an error (#533).
            expected_type: context.mime.clone(),
            // And the size it announced: larger than any waiting page, a page in its place is
            // the site's (#701).
            expected_size: context.size,
            // The page a media file was played on (#800): a file whose address names nothing
            // (a script, a bare id) is named by its title when the engine names it.
            page_title: context.video_title.clone(),
            ..Options::default()
        };
        RequestParts { headers, opts }
    }

    /// Before a download starts: into a free place of its own if its own is another row's
    /// (`place_to_take`). The name stays the address's guess if it was one.
    fn claim_place(&mut self, id: u64) {
        let target = self
            .items
            .iter()
            .find(|i| i.id == id)
            .and_then(|i| place_to_take(i, &self.items));
        if let (Some(target), Some(item)) = (target, self.item(id)) {
            let guessed = item.name_from_url;
            take_place(item, target);
            item.name_from_url = guessed;
            self.dirty = true;
        }
    }

    /// Start or resume a download; `show` opens its progress dialog (IDM's default).
    fn start(&mut self, id: u64, show: bool) {
        self.stopped_by_hand.remove(&id);
        let rt = self.rt.clone();
        let Some(item) = self.item(id) else { return };
        // A finished gallery may start again: it fetches the files not there yet (#58).
        if item.live.is_some() || (item.status == Status::Complete && item.gallery.is_none()) {
            return;
        }
        // The download limit is reached: it starts when the window has room again (a video or a
        // gallery too, #708: their bytes count toward it).
        if self.settings.limits_on && self.quota.blocked {
            if !self.quota.held.contains(&id) {
                self.quota.held.push(id);
            }
            if let Some(item) = self.item(id) {
                item.status = Status::Queued;
                item.note(crate::i18n::t(
                    "Waiting: the download limit is reached (Scheduler)",
                ));
            }
            self.dirty = true;
            return;
        }
        // A video from a page: yt-dlp, not the engine.
        if self.item(id).is_some_and(|i| i.video.is_some()) {
            self.start_video(id, show);
            self.share_with_new_helper(id);
            return;
        }
        // An image gallery: gallery-dl.
        if self.item(id).is_some_and(|i| i.gallery.is_some()) {
            self.start_gallery(id, show);
            self.share_with_new_helper(id);
            return;
        }
        if self.item(id).is_some_and(|i| i.torrent.is_some()) {
            self.start_torrent(id, show);
            return;
        }
        // A torrent through a debrid account (#44).
        if self.item(id).is_some_and(|i| i.remote.is_some()) {
            self.start_remote(id, show);
            return;
        }
        let Some((url, login, context, own_limit, checksum, direct)) = self.item(id).map(|i| {
            (
                i.url.clone(),
                i.login.clone(),
                i.context.clone(),
                i.own_limit,
                i.checksum.clone(),
                i.direct,
            )
        }) else {
            return;
        };
        // The browser's proxy needs its VPN extension's own login (#42): said, never bypassed.
        if let Some(why) = self.settings.browser_proxy_refused(&context) {
            if let Some(item) = self.item(id) {
                item.status = Status::Error;
                item.error = Some(why);
                item.last_try = Some(model::now_stamp());
                // A failure of its own, not the last run's (`Item::got_web_page`).
                item.snap = None;
            }
            self.dirty = true;
            return;
        }
        let RequestParts { headers, mut opts } =
            self.request_parts(&url, login.as_ref(), &context, direct);
        // Download File Info's open answer for it, if it has one (#28); used once.
        let kept = self.kept_answers.remove(&id);
        // The file's own remembered limit, else the Speed Limiter's.
        opts.speed_limit = own_limit.or(self.settings.speed_limit());
        opts.checksum = checksum;
        // A new download from Download File Info has nothing of its own on the disk yet: nothing
        // to resume, and no place to claim from another row (the engine takes the next free
        // name as it starts). Its start never waits for the disk on the window's thread (#743).
        let fresh = self.item(id).is_some_and(|i| i.number_at_start);
        if !fresh {
            self.claim_place(id);
        }
        let Some(item) = self.item(id) else { return };
        let _guard = rt.enter();
        // Resume whenever there is something to resume. If that fails, say why and keep the
        // data: starting over would overwrite it, and that is only ever the user's choice.
        let state = state_path(&item.dest);
        let dl = if !fresh && state.exists() {
            match Download::resume(state, headers, opts) {
                Ok(dl) => dl,
                Err(e) => {
                    item.status = Status::Error;
                    item.error = Some(crate::errors::describe(&e));
                    item.last_try = Some(model::now_stamp());
                    item.snap = None;
                    self.dirty = true;
                    return;
                }
            }
        } else {
            // Nothing to resume. A missing folder is created by the engine; a removed drive is
            // reported without touching the progress shown for the data on it.
            if fresh || item.dest.parent().is_none_or(std::path::Path::is_dir) {
                item.downloaded = 0;
            }
            // A name nobody gave (the address's guess) is the server's to give: into the folder,
            // under its Content-Disposition name (#28); the row follows the file made. A name
            // given before its folder was looked at is numbered on there if it is taken (#743).
            let dest = match item.dest.parent() {
                Some(dir) if item.name_from_url => Destination::Dir(dir.to_owned()),
                _ if fresh => Destination::Free(item.dest.clone()),
                _ => Destination::File(item.dest.clone()),
            };
            Download::start_with(
                Request {
                    url: item.url.clone(),
                    headers,
                    dest,
                },
                opts,
                kept,
            )
        };
        item.note(if item.downloaded > 0 {
            // i18n: event log entry, past tense: the download resumed (just now)
            crate::i18n::t("Resumed")
        } else {
            // i18n: event log entry, past tense: the download started (just now)
            crate::i18n::t("Started")
        });
        item.live = Some(Live::new(model::Job::File(dl), item.downloaded));
        item.status = Status::Downloading;
        item.error = None;
        item.last_try = Some(model::now_stamp());
        self.dirty = true;
        if show && !self.progress.iter().any(|p| p.id == id) {
            self.progress
                .push(Progress::new(id, self.settings.show_complete));
        }
    }

    /// Apply the Speed Limiter's "for one file" setting to every running download, file or
    /// torrent; one with a remembered limit of its own keeps it, as when it starts. Videos and
    /// galleries start again at their new limit (`reapply_helper_limits`).
    fn apply_speed_limit(&mut self) {
        let limit = self.settings.speed_limit();
        for i in &self.items {
            if let Some(l) = &i.live {
                l.dl.set_speed_limit(i.own_limit.or(limit));
            }
        }
        self.reapply_helper_limits();
    }

    /// The limits a video or a gallery (#708) is under, as a file download's are set: its own
    /// limit (set in its progress window, else remembered), else the Speed Limiter's for one
    /// file; and the limit for all downloads together, when on. A change of these starts it
    /// again (`reapply_helper_limits`).
    fn helper_limits(&self, id: u64) -> (Option<u64>, Option<u64>) {
        let own = self
            .helper_own
            .get(&id)
            .copied()
            .unwrap_or_else(|| self.items.iter().find(|i| i.id == id)?.own_limit);
        let s = &self.settings;
        (
            own.or(s.speed_limit()),
            s.global_limit_on.then(|| s.global_limit_kb.max(1) * 1024),
        )
    }

    /// The speed limit a video or a gallery (#708) starts with, bytes/s: the lower of its limit
    /// for one file (`helper_limits`) and an even share, with the other videos and galleries
    /// running, of the limit for all downloads together as file downloads have it now
    /// (`apply_global_limit`: what torrents leave of it).
    fn helper_rate(&self, id: u64) -> Option<u64> {
        let (one, _) = self.helper_limits(id);
        let others = self
            .items
            .iter()
            .filter(|i| i.id != id && i.live.as_ref().is_some_and(|l| l.dl.is_helper()))
            .count() as u64;
        helper_rate(one, self.global.get(), others)
    }

    /// The speed limits changed (the Speed Limiter turned on or off, its settings, a download's
    /// own limit): every running video and gallery whose limit is now another is stopped and
    /// started again with it (#708). yt-dlp continues its partial file, gallery-dl skips the
    /// files already saved, as when the network adapter changes.
    fn reapply_helper_limits(&mut self) {
        let changed: Vec<u64> = self
            .items
            .iter()
            .filter(|i| i.live.as_ref().is_some_and(|l| l.dl.is_helper()))
            .filter(|i| self.helper_rates.get(&i.id).copied() != Some(self.helper_limits(i.id)))
            .map(|i| i.id)
            .collect();
        self.restart_helpers(changed);
    }

    /// A video or a gallery `id` has started under the limit for all downloads (#708): the
    /// others running that were started with more than their even share now are started again
    /// with it, so together they keep to the limit. Starting several at once (a queue, a
    /// playlist) restarts only the ones started a moment before, still looking their page up;
    /// one that ends leaves the others as they are (under the limit).
    fn share_with_new_helper(&mut self, id: u64) {
        if self.global.get().is_none() || !self.item(id).is_some_and(|i| i.live.is_some()) {
            return;
        }
        let over: Vec<u64> = self
            .items
            .iter()
            .filter(|i| i.id != id && i.live.as_ref().is_some_and(|l| l.dl.is_helper()))
            .filter(|i| {
                let now = self.helper_rate(i.id);
                match (self.helper_started.get(&i.id).copied().flatten(), now) {
                    (Some(had), Some(now)) => had > now,
                    (None, Some(_)) => true,
                    _ => false,
                }
            })
            .map(|i| i.id)
            .collect();
        self.restart_helpers(over);
    }

    /// Stop the videos and galleries `ids` to start them again with their speed limit now
    /// (`limit_restart_tick`).
    fn restart_helpers(&mut self, ids: Vec<u64>) {
        for id in ids {
            if let Some(i) = self.items.iter_mut().find(|i| i.id == id)
                && let Some(l) = &i.live
            {
                l.dl.cancel();
                i.note(
                    // i18n: event log entry: a video or gallery download stopped to start again at the new speed limit (its helper program takes a limit only as it starts)
                    crate::i18n::t("Stopped: the speed limit changed; starting again with it")
                        .to_owned(),
                );
            }
            if !self.limit_restarts.contains(&id) {
                self.limit_restarts.push(id);
            }
        }
    }

    /// Videos and galleries stopped for a new speed limit, started again once their jobs have
    /// ended (#708).
    fn limit_restart_tick(&mut self) {
        if self.limit_restarts.is_empty() {
            return;
        }
        let ready: Vec<u64> = self
            .limit_restarts
            .iter()
            .copied()
            .filter(|id| self.items.iter().any(|i| i.id == *id && i.live.is_none()))
            .collect();
        for &id in &ready {
            self.start(id, false);
        }
        self.limit_restarts
            .retain(|id| self.items.iter().any(|i| i.id == *id) && !ready.contains(id));
    }

    fn stop(&mut self, id: u64) {
        if let Some(l) = self.item(id).and_then(|i| i.live.as_ref()) {
            l.dl.cancel();
        }
    }

    /// The user's Stop (or Pause): it stays stopped, even in a queue that is running (#341).
    fn stop_by_hand(&mut self, id: u64) {
        self.stop(id);
        self.stopped_by_hand.insert(id);
    }

    fn stop_all(&mut self) {
        let running: Vec<u64> = self
            .items
            .iter()
            .filter(|i| i.live.is_some())
            .map(|i| i.id)
            .collect();
        for id in running {
            self.stop_by_hand(id);
        }
    }

    /// Take a download off the list; its files stay.
    fn delete(&mut self, id: u64) {
        self.remove_downloads(&[id], false);
    }

    /// Take downloads off the list (not while they run; see [`Self::remove_download`]). When
    /// the selection goes with them, the row that comes after them is selected, as in Explorer,
    /// so Delete can be pressed again.
    fn remove_downloads(&mut self, ids: &[u64], files: bool) {
        let before = self.shown_ids();
        self.extracts.forget(ids);
        for id in ids {
            self.remove_download(*id, files);
        }
        let after = self.shown_ids();
        self.selection.after_removal(&before, &after);
    }

    /// Download `id` went through a debrid account and ended (done, or taken off the list): the
    /// service forgets its link (TorBox's list of web downloads, #391). A paused one keeps it,
    /// to go on with.
    fn release_link(&self, id: u64) {
        let Some(i) = self.items.iter().find(|i| i.id == id) else {
            return;
        };
        if !self.settings.debrid_on
            || i.direct
            || i.remote.is_some()
            || i.torrent.is_some()
            || i.video.is_some()
            || i.gallery.is_some()
        {
            return;
        }
        let (debrid, url) = (self.debrid.clone(), i.url.clone());
        self.rt.spawn(async move {
            if let Err(e) = debrid.release(&url).await {
                tracing::warn!("debrid: the link stays in the service's list: {e:?}");
            }
        });
    }

    /// Take a download off the list (not while it runs); with `files`, its file goes from disk
    /// too: a finished file, or the data so far (a torrent's files when its session runs).
    fn remove_download(&mut self, id: u64, files: bool) {
        if self.item(id).is_some_and(|i| i.live.is_none()) {
            self.release_link(id);
            if let Some(spec) = self.item(id).and_then(|i| i.torrent.clone()) {
                self.torrent_host.forget(&self.rt, &spec.hash, files);
            } else if files
                && let Some(i) = self.items.iter().find(|i| i.id == id)
                // A gallery's folder stays (it may hold other files; #58).
                && i.gallery.is_none()
            {
                // The file itself: one that can't go (open in a player) is said, not skipped
                // silently; the row still leaves the list.
                if i.status == Status::Complete
                    && let Err(e) = std::fs::remove_file(&i.dest)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    dialogs::tell(
                        crate::i18n::mark("Delete"),
                        crate::i18n::fmt(
                            crate::i18n::t("{file} could not be deleted from the disk: {error}"),
                            &[
                                ("file", &crate::i18n::isolate(&i.file_name)),
                                ("error", &e.to_string()),
                            ],
                        ),
                    );
                }
                let _ = std::fs::remove_file(mdm_engine::naming::part_path(&i.dest));
                let _ = std::fs::remove_file(state_path(&i.dest));
                // A video's streams and their parts, kept for Resume (an unfinished run, a
                // failed merge): they go with it.
                for f in video_leftovers(&self.items, id, false) {
                    let _ = std::fs::remove_file(f);
                }
            }
            self.items.retain(|i| i.id != id);
            for q in &mut self.queues {
                q.items.retain(|i| *i != id);
            }
            self.progress.retain(|p| p.id != id);
            self.selection.forget(id);
            self.dirty = true;
        }
    }

    /// Delete / Remove / Delete All Completed: asks first (unless "Don't show this dialog
    /// again" was ticked) whether only the list entries go or the files too; `files` ticks that
    /// box (Shift+Delete). Running downloads are left alone.
    fn ask_delete(&mut self, ids: Vec<u64>, files: bool) {
        let gone: Vec<&Item> = ids
            .iter()
            .filter_map(|id| self.items.iter().find(|i| i.id == *id && i.live.is_none()))
            .collect();
        let Some(first) = gone.first() else { return };
        // Shift+Delete ticks "also the files" this once; only a plain Delete's answer is
        // remembered (else one Shift+Delete made every later Delete erase files).
        let shift = files;
        let files = files || self.settings.delete_files_too;
        if self.settings.confirm_delete {
            let one = gone.len() == 1;
            let mut ask = dialogs::ConfirmDelete::new(
                gone.iter().map(|i| i.id).collect(),
                first.file_name.clone(),
                one && first.status != Status::Complete,
                files,
            );
            ask.remember = !shift;
            self.confirm_delete = Some(ask);
        } else {
            let ids: Vec<u64> = gone.into_iter().map(|i| i.id).collect();
            self.remove_downloads(&ids, files);
        }
    }

    fn delete_completed(&mut self) {
        let done = self
            .items
            .iter()
            .filter(|i| i.status == Status::Complete)
            .map(|i| i.id)
            .collect();
        self.ask_delete(done, false);
    }

    /// The global cap's rate now: on, and (if asked) only while the user is active (input in
    /// the last minute).
    ///
    /// Torrents share it with file downloads (D-012, "one limit for all downloads together"),
    /// evenly: each side may take half of it, and more when the other uses less (`share`).
    /// Both sides' speeds are measured each frame: the torrents' as the torrent session's
    /// download limit, file downloads' as the global limiter they wait on.
    fn apply_global_limit(&mut self) {
        let s = &self.settings;
        #[cfg(windows)]
        let active = crate::win32::idle_seconds() < 60;
        #[cfg(not(windows))]
        let active = true;
        let rate = (s.global_limit_on && (!s.global_only_active || active))
            .then(|| s.global_limit_kb.max(1) * 1024);
        let torrents = self.torrent_speed();
        let files = self.file_speed();
        // The torrents' limit moves in eighths of the cap, at most every 3 s: librqbit starts
        // each new limit with a second's burst, so following the other side's speed closely
        // let torrents run far past their share (measured: 773 KB/s for a 400 KB/s half).
        let want = rate.map(|cap| {
            let step = (cap / 8).max(1);
            (share(cap, files) / step * step).max(1024)
        });
        let (had, when) = self.torrent_cap;
        // Down at once (the other side has started: give way now), up only 3 s after the last
        // change.
        let moved = match (had, want) {
            (Some(a), Some(b)) => b < a || (b > a && when.elapsed() >= Duration::from_secs(3)),
            (a, b) => a != b,
        };
        if moved {
            self.torrent_cap = (want, Instant::now());
        }
        self.torrent_host.set_cap(self.torrent_cap.0);
        let for_files = rate.map(|cap| share(cap, torrents));
        if self.global.get() != for_files {
            self.global.set(for_files);
        }
    }

    /// File downloads' speed together, bytes/s, smoothed over about two seconds: the bytes that
    /// went through the global limiter, which every file download waits on. (Their progress
    /// rates read high and low by a quarter, which tipped the split.)
    fn file_speed(&mut self) -> f64 {
        let now = Instant::now();
        let taken = self.global.taken();
        let (speed, at, before) = self.file_speed;
        let dt = now.duration_since(at).as_secs_f64();
        if dt <= 0.0 {
            return speed;
        }
        let new = taken.saturating_sub(before) as f64;
        let k = 1.0 - (-dt / 2.0).exp();
        let mut speed = speed + (new / dt - speed) * k;
        // None running: 0 at once, so the torrents take the whole cap back in one step.
        if !self
            .items
            .iter()
            .any(|i| matches!(i.live.as_ref().map(|l| &l.dl), Some(model::Job::File(_))))
        {
            speed = 0.0;
        }
        self.file_speed = (speed, now, taken);
        speed
    }

    /// The torrents' download speed together, bytes/s, smoothed over about two seconds (from
    /// the bytes librqbit says each fetched; it counts from 0 again each time a torrent
    /// resumes, so a smaller count is all new).
    fn torrent_speed(&mut self) -> f64 {
        let now = Instant::now();
        let mut seen = HashMap::new();
        let mut new = 0u64;
        for i in &self.items {
            let Some(model::Job::Torrent(run)) = i.live.as_ref().map(|l| &l.dl) else {
                continue;
            };
            let Some(fetched) = run.fetched() else {
                continue;
            };
            if let Some(&before) = self.torrent_fetched.get(&i.id) {
                new += fetched.checked_sub(before).unwrap_or(fetched);
            }
            seen.insert(i.id, fetched);
        }
        self.torrent_fetched = seen;
        let dt = now.duration_since(self.torrent_speed.1).as_secs_f64();
        self.torrent_speed.1 = now;
        if dt > 0.0 {
            let k = 1.0 - (-dt / 2.0).exp();
            self.torrent_speed.0 += (new as f64 / dt - self.torrent_speed.0) * k;
        }
        if self.torrent_fetched.is_empty() {
            self.torrent_speed.0 = 0.0;
        }
        self.torrent_speed.0
    }

    /// Progress of all running downloads together (bytes done / bytes in all), None when
    /// nothing runs or no size is known. For the taskbar button's progress, which only Windows
    /// has (macOS and Linux docks have no common way to show one).
    #[cfg_attr(not(windows), allow(dead_code))]
    fn overall_progress(&self) -> Option<f64> {
        let (done, total) = self
            .items
            .iter()
            .filter(|i| i.live.is_some())
            .filter_map(|i| i.size.map(|s| (i.downloaded, s)))
            .fold((0u64, 0u64), |(d, t), (di, si)| (d + di.min(si), t + si));
        (total > 0).then(|| done as f64 / total as f64)
    }

    /// The main window into the tray: downloads, queues and schedules go on. Wayland can't hide a
    /// window, so there it goes to the dock / task list instead.
    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "linux")]
        let wayland = crate::unix::on_wayland();
        #[cfg(not(target_os = "linux"))]
        let wayland = false;
        if wayland {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        self.hidden = true;
        self.start_trimming();
    }

    /// Hidden, the window's memory sits unused until it shows again: give it back to the system
    /// (#72), on the schedule of `next_trim`. Windows pages it back in from RAM when it's touched
    /// again. Both ways into the tray (closing the window, and Minimize when it goes there).
    fn start_trimming(&mut self) {
        self.trims_since_hidden = 0;
        self.trim_when_hidden = Some(Instant::now() + next_trim(0));
    }

    /// The window back from the tray (or from minimized), in front.
    fn show_window(&mut self, ctx: &egui::Context) {
        // Asked for (a second launch, the tray icon): no start with the session to settle.
        self.at_login = None;
        self.trim_when_hidden = None;
        self.hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        if !crate::offscreen() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        ctx.request_repaint();
    }

    /// Global shortcut (#558): show and focus the window when hidden, minimized, or behind
    /// another application; otherwise hide it to the notification area (or minimize it).
    fn toggle_window(&mut self, ctx: &egui::Context) {
        let minimized = ctx.input(|i| i.viewport().minimized == Some(true));
        #[cfg(target_os = "macos")]
        let active_and_visible =
            !self.hidden && !minimized && crate::unix::macos_is_active_and_visible();
        #[cfg(not(target_os = "macos"))]
        let active_and_visible =
            !self.hidden && !minimized && ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if !active_and_visible {
            self.show_window(ctx);
            #[cfg(target_os = "macos")]
            if !crate::offscreen() {
                crate::unix::macos_activate_and_show();
            }
        } else if self.tray.is_some() && self.tray_icon_now() != crate::tray::Icon::Gone {
            self.hide_to_tray(ctx);
            #[cfg(target_os = "macos")]
            if !crate::offscreen() {
                crate::unix::macos_hide_window();
            }
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }

    /// Before a handoff's dialog: the main window stays where it is (hidden, minimized) and
    /// only the dialog opens, as dialogs are windows of their own.
    fn show_for_dialog(&mut self, ctx: &egui::Context) {
        let minimized = ctx.input(|i| i.viewport().minimized == Some(true));
        if !self.hidden && !minimized {
            self.show_window(ctx);
        }
    }

    /// What the shell says of the tray icon (Linux: there when a tray host is, asked each time:
    /// the host may come and go). On Windows and macOS the icon itself is asked: the shell can
    /// accept it and still show nothing, and a window hidden with no icon to click has no way
    /// back. Windows' answer comes from a thread of its own and is never waited for (#747);
    /// macOS's is a second old at most, for the per-frame minimize path.
    fn tray_icon(&self) -> crate::tray::Icon {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Some(tray) = &self.tray {
            return tray.icon();
        }
        Self::tray_host()
    }

    /// [`tray_icon`] for the close decision: on macOS asked afresh, as an icon can disappear
    /// between the cached answer and the close, and hiding then would be a ghost window.
    fn tray_icon_now(&self) -> crate::tray::Icon {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Some(tray) = &self.tray {
            return tray.icon_now();
        }
        Self::tray_host()
    }

    /// Whether a tray shows icons at all, as [`crate::tray::available`] says.
    fn tray_host() -> crate::tray::Icon {
        if crate::tray::available() {
            crate::tray::Icon::There
        } else {
            crate::tray::Icon::Gone
        }
    }

    /// Started with the session (#226): once a second, until it is known, where the window goes
    /// ([`at_login`]): it stays hidden in the notification area, or shows minimized.
    fn settle_at_login(&mut self, ctx: &egui::Context) {
        let Some(started) = self.at_login else {
            return;
        };
        ctx.request_repaint_after(Duration::from_secs(1));
        if self
            .at_login_checked
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        self.at_login_checked = Some(Instant::now());
        if self.tray.is_some() && self.at_login_icon.is_none() {
            self.at_login_icon = Some(Instant::now());
        }
        let icon = self.at_login_icon.map(|t| t.elapsed());
        let since = started.elapsed();
        let shown = icon.is_some() && self.tray_icon() == crate::tray::Icon::There;
        match at_login(since, icon, shown) {
            AtLogin::Wait => {}
            AtLogin::Tray => {
                tracing::info!(
                    "started with the session: in the notification area ({} ms on)",
                    since.as_millis()
                );
                self.at_login = None;
            }
            AtLogin::Minimize => {
                tracing::info!(
                    "started with the session: no notification area within {} s of the icon: minimized ({} ms on)",
                    TRAY_GRACE.as_secs(),
                    since.as_millis()
                );
                self.at_login = None;
                self.trim_when_hidden = None;
                self.hidden = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }
    }

    /// The tray icon: created once, its commands run, and closing the window hides it there.
    fn tray(&mut self, ctx: &egui::Context) {
        // Off-screen test copies have none, but for MDM_TEST_TRAY=1 (#226's check: a start with
        // the session shows the icon, and no window).
        let wanted =
            !crate::offscreen() || std::env::var_os("MDM_TEST_TRAY").is_some_and(|v| v == "1");
        if self.tray.is_none() && self.tray_limiter.is_none() && wanted {
            // Linux: made beside the window, which never waits for the session bus (#828); until
            // it is, the window behaves as with no tray host.
            #[cfg(target_os = "linux")]
            {
                self.tray_setup =
                    Some(crate::tray::Setup::start(ctx, self.settings.speed_limit_on));
            }
            #[cfg(not(target_os = "linux"))]
            {
                self.tray = crate::tray::Tray::new(ctx, self.settings.speed_limit_on);
                if self.tray.is_some() {
                    tracing::info!("tray icon shown");
                }
            }
            // The limiter the menu is made with (set again below once it changes).
            self.tray_limiter = Some(self.settings.speed_limit_on);
        }
        #[cfg(target_os = "linux")]
        if let Some(setup) = &mut self.tray_setup {
            match setup.poll() {
                crate::tray::Came::Waiting => {}
                crate::tray::Came::Up(tray) => {
                    tracing::info!("tray icon shown");
                    self.tray = Some(tray);
                    self.tray_setup = None;
                }
                crate::tray::Came::Failed => self.tray_setup = None,
            }
        }
        if !crate::offscreen() {
            let spec = self
                .options
                .as_ref()
                .map_or(self.settings.show_hide_hotkey.as_str(), |o| {
                    o.draft_hotkey()
                })
                .to_owned();
            let recording = self
                .options
                .as_ref()
                .is_some_and(|o| o.is_recording_hotkey());
            let hk = self
                .hotkey
                .get_or_insert_with(|| crate::hotkey::GlobalHotkey::new(ctx, &spec));
            hk.apply(ctx, &spec);
            let taken = hk.is_taken();
            if let Some(o) = &mut self.options {
                o.set_hotkey_taken(taken);
            }
            if hk.take_fired() && !recording {
                self.toggle_window(ctx);
            }
        }
        if let Some(t) = &self.tray
            && self.tray_limiter != Some(self.settings.speed_limit_on)
        {
            t.set_limiter(self.settings.speed_limit_on);
            self.tray_limiter = Some(self.settings.speed_limit_on);
        }
        if let (Some(t), Some(q)) = (&self.tray, self.queues.first()) {
            t.set_queue(q.running);
        }
        if let Some(t) = &self.tray {
            let mut tip = match self.activity() {
                Some(a) => format!("{}\n{a}", crate::MAIN_TITLE),
                None => crate::MAIN_TITLE.to_owned(),
            };
            // Download complete snoozed (#741): what finished meanwhile.
            if self.snoozed_finished > 0 && self.snooze.active(unix_now()) {
                tip.push('\n');
                tip.push_str(&dialogs::finished_count(self.snoozed_finished));
            }
            t.set_tooltip(&tip);
            // "Downloads in progress": each running download, to show its progress window (#63).
            let running: Vec<(u64, String)> = self
                .items
                .iter()
                .filter(|i| i.live.is_some())
                .map(|i| (i.id, i.file_name.clone()))
                .collect();
            t.set_downloads(&running);
        }
        self.settle_at_login(ctx);
        // Hidden in the notification area, and the shell, running and answering, has said for
        // some seconds that it has no icon of MDM's (lost, and not added back): the window comes
        // back to the taskbar, minimized. Never on no answer, or while Explorer isn't running
        // (it gives the icon back as it starts again).
        #[cfg(windows)]
        if self.hidden && self.at_login.is_none() && self.tray.as_ref().is_some_and(|t| t.rescue())
        {
            tracing::info!(
                "hidden, and the shell has no tray icon of MDM's: back in the taskbar, minimized"
            );
            self.hidden = false;
            self.trim_when_hidden = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        // When the window was last focused, for Toggle below. Kept every frame:
        // by the time a tray click arrives the taskbar already took the focus.
        if ctx.input(|i| i.viewport().focused) == Some(true) {
            self.tray_last_focused = Some(Instant::now());
        }
        let cmds = self.tray.as_ref().map(|t| t.poll()).unwrap_or_default();
        for c in cmds {
            use crate::tray::TrayCmd;
            tracing::info!("tray: {c:?}");
            let cmd = match c {
                TrayCmd::Show => {
                    self.show_window(ctx);
                    continue;
                }
                TrayCmd::Toggle => {
                    let minimized = ctx.input(|i| i.viewport().minimized == Some(true));
                    // Behind other windows it still reads as shown: only hide when
                    // frontmost, otherwise bring it forward. On Windows the tray
                    // click itself takes the focus first, so a window focused just
                    // now still counts as in front (#520).
                    let focus = ctx.input(|i| i.viewport().focused);
                    let since_focused = self.tray_last_focused.map(|t| t.elapsed());
                    // With the icon under the taskbar's ^, the flyout took the focus well
                    // before the click: Windows is asked whether anything covers the window.
                    #[cfg(windows)]
                    let in_front = crate::win32::main_window_in_front() == Some(true);
                    // Elsewhere focus is all there is to go by (sway, KDE's tray).
                    #[cfg(not(windows))]
                    let in_front = false;
                    if tray_toggle_hide(self.hidden, minimized, focus, since_focused)
                        || (!self.hidden && !minimized && in_front)
                    {
                        self.hide_to_tray(ctx);
                    } else {
                        self.show_window(ctx);
                    }
                    continue;
                }
                TrayCmd::AddUrl => Cmd::AddUrl,
                TrayCmd::Batch => Cmd::Batch,
                TrayCmd::BatchClipboard => Cmd::BatchClipboard,
                TrayCmd::LimiterOn => Cmd::LimiterOn,
                TrayCmd::LimiterOff => Cmd::LimiterOff,
                TrayCmd::Scheduler => Cmd::Scheduler,
                TrayCmd::StartQueue => Cmd::StartQueue(0),
                TrayCmd::StopQueue => Cmd::StopQueue(0),
                TrayCmd::Options => Cmd::Options,
                TrayCmd::Exit => Cmd::Exit,
                TrayCmd::ShowDownload(id) => {
                    self.open_item(id);
                    continue;
                }
            };
            self.run(cmd, None, ctx);
        }
        // Closing hides the window in the tray; downloads and queues go on. When the shell says it
        // has no icon (Windows, macOS: a tray that exists, and the shell says it has none) the
        // window goes to the taskbar or the Dock instead: still reachable, downloads still going.
        // No answer from the shell is not such a "no" (#750, `tray::on_close`). Everywhere else
        // closing closes, as any window's does: Options' "Keep running when closing the window"
        // off, no tray at all, or Linux with no tray host (#59: a hidden window couldn't come
        // back, and one that merely minimized could never be closed).
        if !self.exiting && ctx.input(|i| i.viewport().close_requested()) {
            use crate::tray::OnClose;
            let keep = self.settings.close_to_tray;
            let tray = self.tray.is_some();
            // The shell's answer, only when it matters (macOS: asked afresh).
            let icon = if keep && tray {
                self.tray_icon_now()
            } else {
                crate::tray::Icon::Gone
            };
            let taskbar = cfg!(not(target_os = "linux"));
            match crate::tray::on_close(keep, tray, icon, taskbar) {
                OnClose::Hide => {
                    tracing::info!("close: hidden to the notification area (tray icon: {icon:?})");
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.hide_to_tray(ctx);
                }
                OnClose::Minimize => {
                    tracing::info!("close: no visible tray icon; minimizing to the taskbar");
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    crate::dialogs::minimize(ctx, self.main_hwnd);
                }
                OnClose::Quit => {
                    tracing::info!("close: quitting");
                    self.exiting = true;
                }
            }
        }
        // Options → General → "Minimize to the notification area" (#63): Minimize hides the
        // window there too, where a tray shows the icon. Wayland can't hide a window: there it
        // stays minimized, in the dock, as it was.
        #[cfg(target_os = "linux")]
        let can_hide = !crate::unix::on_wayland();
        #[cfg(not(target_os = "linux"))]
        let can_hide = true;
        let wanted = self.tray.is_some()
            && self.settings.minimize_to_tray
            && can_hide
            && !self.exiting
            && self.tray_icon() != crate::tray::Icon::Gone;
        // Windows: the minimize command itself is caught (win32), so the window goes straight to
        // the tray, never minimized (an idle app runs no frame while minimized: it wouldn't see).
        #[cfg(windows)]
        let minimize = crate::win32::main_minimize_to_tray(ctx, wanted);
        // Elsewhere: on the change to minimized (brought back, the window may still read
        // minimized for a frame or two). It is seen at the next frame the app runs; macOS doesn't
        // report it at all while running (egui#3494), so there Minimize stays in the Dock.
        #[cfg(not(windows))]
        let minimize = {
            let minimized = ctx.input(|i| i.viewport().minimized == Some(true));
            let was = egui::Id::unique("main-was-minimized");
            let was_minimized = ctx.data(|d| d.get_temp::<bool>(was)).unwrap_or(false);
            ctx.data_mut(|d| d.insert_temp(was, minimized));
            wanted && minimized && !was_minimized
        };
        if minimize && !self.hidden {
            tracing::info!("minimized to the notification area");
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.hidden = true;
            self.start_trimming();
        }
        // Hidden, the window gets no paint messages: keep the clock (queues, scheduler,
        // download limits) running.
        if self.hidden {
            ctx.request_repaint_after(Duration::from_secs(1));
            if self.trim_when_hidden.is_some_and(|t| Instant::now() >= t) {
                trim_memory();
                self.trims_since_hidden += 1;
                self.trim_when_hidden = Some(Instant::now() + next_trim(self.trims_since_hidden));
            }
        }
    }

    /// While an assistive technology is listening (a screen reader running, #131), every window
    /// builds its accessibility tree with each frame, so the first time one is read it answers
    /// with its contents rather than an empty window (egui keeps the last tree for that). Asked
    /// once a second; never turned off here (the platform says when nobody listens any more).
    fn follow_assistive_technology(&mut self, ctx: &egui::Context) {
        if self
            .assistive_checked
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        self.assistive_checked = Some(Instant::now());
        #[cfg(windows)]
        let listening = crate::win32::assistive_technology_listening();
        #[cfg(not(windows))]
        let listening = crate::unix::assistive_technology_listening();
        if listening {
            ctx.enable_accesskit();
        }
    }

    /// Options → General → "Offer to download links copied to the clipboard": a newly copied
    /// link to a listed file type (Options → File types) is handled as if a browser had sent
    /// it, so Download File Info opens with it (unless that window is turned off). Only read,
    /// never written; on by default.
    fn watch_clipboard(&mut self, ctx: &egui::Context) {
        // Looked at off the window's thread (clipboard.rs): the program that owns the clipboard
        // may hang, and a read that waits for it took the window with it (#418). The watch wakes
        // the window only for a newly copied text: an idle MDM draws no frame for it (#847).
        let on = self.settings.clipboard_autostart;
        crate::clipboard::watch(on);
        let Some(text) = crate::clipboard::copied().filter(|_| on) else {
            return;
        };
        // Several links (or words) are not one address: the address parser drops line breaks,
        // and made one address of a copied list (#709). They are for Tasks → Add batch download
        // from clipboard.
        if text.contains(char::is_whitespace) {
            return;
        }
        let Ok(url) = url::Url::parse(&text) else {
            return;
        };
        let name = mdm_engine::naming::file_name(None, &url);
        if mdm_engine::supported(&url) && self.settings.clipboard_type(&name) {
            let cmd = ipc::Command {
                url: Some(text),
                ..Default::default()
            };
            self.handle_command(cmd, ctx);
        }
    }

    /// Open (or bring back) the File Properties of a download.
    fn properties(&mut self, id: u64) {
        if self.properties.iter().any(|p| p.id == id) {
            dialogs::raise(
                &self.ui_ctx,
                dialogs::dialog_viewport(&format!("properties-{id}")),
            );
            return;
        }
        let route = self.debrid_route();
        if let Some(item) = self.items.iter().find(|i| i.id == id) {
            self.properties
                .push(crate::properties::Properties::new(item, route));
        }
    }

    /// File Properties → OK.
    fn apply_properties(&mut self, c: crate::properties::Changes) {
        let Some(item) = self.item(c.id) else { return };
        item.description = c.description;
        item.login = c.login;
        item.set_referer(c.referer);
        // "Through …" changed (#44): the next start goes the new way. The service's link is
        // dropped from the resume state: directly, it continues from the host's own address;
        // through the account, from a new link the service gives. The size and validators
        // still have to match.
        if let Some(direct) = c.direct {
            item.direct = direct;
            let state = mdm_engine::state::state_path(&item.dest);
            if let Ok(mut saved) = mdm_engine::state::ResumeState::load(&state) {
                saved.final_url = saved.url.clone();
                let _ = saved.write(&state);
            }
            item.note(if direct {
                // i18n: event log: File Properties: the download no longer goes through the debrid account
                crate::i18n::t("Set to download directly, not through the debrid account")
                    .to_owned()
            } else {
                // i18n: event log: File Properties: the download goes through the debrid account again
                crate::i18n::t("Set to download through the debrid account").to_owned()
            });
        }
        if let Some(url) = c.address {
            // Refresh download address: the resume state follows, so the download continues
            // from the new address (its size and validators still have to match).
            let state = mdm_engine::state::state_path(&item.dest);
            if let Ok(mut saved) = mdm_engine::state::ResumeState::load(&state) {
                saved.url = url.clone();
                saved.final_url = url.clone();
                let _ = saved.write(&state);
            }
            // Where the browser's download of the old address ended isn't this one's (#630).
            item.context.final_url = None;
            item.note(crate::i18n::fmt(
                crate::i18n::t("Address changed to {host}"),
                &[("host", &crate::i18n::ltr(url.host_str().unwrap_or("?")))],
            ));
            item.url = url;
            item.error = None;
            if item.status == Status::Error {
                item.status = Status::Stopped;
            }
        }
        // Moved into the folder it is already in: nothing to do (it became "a (1).zip").
        let move_to = c
            .move_to
            .filter(|dir| item.dest.parent() != Some(dir.as_path()))
            .map(|dir| mdm_engine::naming::destination(&dir, &item.file_name));
        self.dirty = true;
        if let Some(target) = move_to {
            self.start_move(c.id, target);
        }
    }

    /// Move a download to `target` (its name too): a finished file moves on a thread of its
    /// own (a copy to another drive takes a while), one not started just takes the new place.
    /// A failure says why.
    fn start_move(&mut self, id: u64, target: PathBuf) {
        let Some(item) = self.item(id) else { return };
        if target == item.dest {
            return;
        }
        if item.status != Status::Complete {
            if item.downloaded == 0 {
                take_place(item, target);
                self.dirty = true;
            }
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let from = item.dest.clone();
        let to = target.clone();
        item.note(crate::i18n::t("Moving the file..."));
        std::thread::spawn(move || {
            let _ = tx.send(move_file(&from, &to));
        });
        self.moving.push((id, target, rx));
    }

    /// Moves that finished: the download takes its new place, or the user is told why not.
    fn poll_moves(&mut self) {
        let mut done = Vec::new();
        self.moving.retain(|(id, target, rx)| match rx.try_recv() {
            Ok(r) => {
                done.push((*id, target.clone(), r));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
        for (id, target, r) in done {
            match r {
                Ok(()) => {
                    if let Some(item) = self.item(id) {
                        take_place(item, target.clone());
                    }
                    for c in &mut self.complete {
                        if c.id == id {
                            c.path = target.clone();
                        }
                    }
                    self.dirty = true;
                }
                Err(e) => {
                    if let Some(item) = self.item(id) {
                        item.note(crate::i18n::t("The file could not be moved"));
                    }
                    self.notice = Some((
                        crate::i18n::mark("Move/Rename"),
                        crate::i18n::fmt(
                            crate::i18n::t("The file could not be moved: {error}"),
                            &[("error", &e)],
                        ),
                    ));
                }
            }
        }
    }

    /// Pure in-memory stamp of everything that can change a progress window's
    /// `rename_disabled_reason`, so disk checks never run per frame while idle.
    fn progress_rename_guard_stamp(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.downloads.hash(&mut h);
        self.settings.torrent_keep_sharing.hash(&mut h);
        let mut cat_dirs: Vec<_> = self.settings.category_dirs.iter().collect();
        cat_dirs.sort_unstable();
        cat_dirs.hash(&mut h);
        for c in &self.settings.custom_categories {
            c.id.hash(&mut h);
            c.name.hash(&mut h);
            c.folder.hash(&mut h);
        }
        for b in &self.settings.builtin_categories {
            b.category.hash(&mut h);
            b.deleted.hash(&mut h);
        }
        self.settings.folder_history.hash(&mut h);
        for r in &self.rss.rules {
            r.folder.hash(&mut h);
        }
        for i in &self.items {
            i.id.hash(&mut h);
            i.dest.hash(&mut h);
            i.file_name.hash(&mut h);
            (i.status == Status::Complete).hash(&mut h);
            i.gallery.is_some().hash(&mut h);
            i.preexisting_folder.hash(&mut h);
            if let Some(t) = &i.torrent {
                t.folder.hash(&mut h);
                t.files.hash(&mut h);
            }
            if let Some(r) = &i.remote {
                r.folder.hash(&mut h);
            }
        }
        h.finish()
    }

    /// Apply any confirmed deferred rename (`Item::rename_on_complete`) once download `id`
    /// completes. Returns `true` when an asynchronous torrent rename is still in flight (or
    /// queued behind a busy `torrent_host`), so archive auto-extraction waits for the final path.
    fn apply_deferred_rename(&mut self, id: u64) -> bool {
        let confirmed = self
            .progress
            .iter_mut()
            .find(|p| p.id == id)
            .and_then(Progress::take_confirmed_rename);
        if let Some(confirmed) = confirmed
            && let Some(item) = self.item(id)
        {
            item.rename_on_complete = confirmed;
            self.dirty = true;
        }
        let Some(desired) = self
            .items
            .iter()
            .find(|i| i.id == id)
            .and_then(|i| i.rename_on_complete.clone())
        else {
            return false;
        };
        self.start_deferred_rename(id, desired)
    }

    fn start_deferred_rename(&mut self, id: u64, desired: String) -> bool {
        let Some((from, is_dir, torrent_spec, disabled)) =
            self.items.iter().find(|i| i.id == id).map(|item| {
                let from = item.dest.clone();
                let is_dir = crate::deferred_rename::is_folder_target(item);
                let torrent_spec = item.torrent.clone();
                let disabled = deferred_rename_disabled_reason_with(
                    item,
                    &self.downloads,
                    &self.settings,
                    &self.rss,
                    &self.items,
                );
                (from, is_dir, torrent_spec, disabled)
            })
        else {
            return false;
        };
        if let Some(reason_key) = disabled {
            if let Some(item) = self.item(id) {
                item.rename_on_complete = None;
                item.note(crate::i18n::t(reason_key));
            }
            self.dirty = true;
            return false;
        }
        let Some((initial_to, hold)) =
            crate::deferred_rename::reserve_target_path(&from, &desired, is_dir)
        else {
            if let Some(item) = self.item(id) {
                item.rename_on_complete = None;
            }
            self.dirty = true;
            return false;
        };
        if let Some(spec) = torrent_spec {
            let (tx, rx) = std::sync::mpsc::channel();
            let known = self.session_hashes();
            match self.torrent_host.rename_finished(
                &self.rt,
                known,
                &spec,
                from.clone(),
                initial_to,
                hold,
                is_dir,
                self.settings.torrent_keep_sharing,
                tx,
            ) {
                Ok(()) => {
                    // Clear before the background rename moves the folder so an abrupt exit
                    // never leaves a stale `rename_on_complete` pointing at the old `dest`.
                    if let Some(item) = self.item(id) {
                        item.rename_on_complete = None;
                    }
                    self.renaming_torrents.push((id, from, rx));
                    self.dirty = true;
                    return true;
                }
                Err(hold) => {
                    drop(hold);
                    // Keep `item.rename_on_complete` persisted in `downloads.json` while waiting
                    // for `torrent_host` so quitting MDM before it starts does not lose it.
                    if !self.deferred_torrent_queue.contains(&id) {
                        self.deferred_torrent_queue.push(id);
                    }
                    self.dirty = true;
                    return true;
                }
            }
        }
        match crate::deferred_rename::rename_reserved_or_number(&from, initial_to, hold, is_dir) {
            Ok(actual_to) => {
                if let Some(item) = self.item(id) {
                    item.rename_on_complete = None;
                    apply_renamed_paths_to_pending_mark(item, &from, &actual_to, is_dir);
                    take_place(item, actual_to);
                }
                self.dirty = true;
            }
            Err(e) => {
                let err = e.to_string();
                let msg = crate::i18n::fmt(
                    crate::i18n::t("The file could not be moved: {error}"),
                    &[("error", &err)],
                );
                if let Some(item) = self.item(id) {
                    item.rename_on_complete = None;
                    item.note(msg.clone());
                }
                self.notice = Some((crate::i18n::mark("Move/Rename"), msg));
                self.dirty = true;
            }
        }
        false
    }

    fn apply_finished_torrent_rename(
        &mut self,
        id: u64,
        from: &std::path::Path,
        r: Result<PathBuf, String>,
        sharing_error: Option<String>,
        notify_ui: bool,
    ) {
        match r {
            Ok(actual_to) => {
                if let Some(item) = self.item(id) {
                    item.rename_on_complete = None;
                    let is_dir = crate::deferred_rename::is_folder_target(item);
                    apply_renamed_paths_to_pending_mark(item, from, &actual_to, is_dir);
                    take_place(item, actual_to.clone());
                }
                for c in &mut self.complete {
                    if c.id == id {
                        c.path = actual_to.clone();
                    }
                }
                self.dirty = true;
            }
            Err(e) => {
                let msg = crate::i18n::fmt(
                    crate::i18n::t("The file could not be moved: {error}"),
                    &[("error", &e)],
                );
                if let Some(item) = self.item(id) {
                    item.rename_on_complete = None;
                    item.note(msg.clone());
                }
                if notify_ui {
                    self.notice = Some((crate::i18n::mark("Move/Rename"), msg));
                }
                self.dirty = true;
            }
        }
        if let Some(why) = sharing_error
            && let Some(item) = self.item(id)
        {
            item.note(crate::i18n::fmt(
                // i18n: event log entry on a finished torrent when re-attaching it to keep sharing failed; {error} = why
                crate::i18n::t("Sharing stopped: {error}"),
                &[("error", &why)],
            ));
            self.dirty = true;
        }
        self.flush_pending_mark(id);
        if notify_ui {
            self.extracts.finished(id, &self.items, &self.settings);
        }
    }

    /// Retry queued deferred torrent renames once `torrent_host` is free, and apply finished ones.
    fn poll_deferred_torrent_renames(&mut self) {
        if !self.deferred_torrent_queue.is_empty() {
            let queued = std::mem::take(&mut self.deferred_torrent_queue);
            for id in queued {
                let busy = self
                    .items
                    .iter()
                    .find(|i| i.id == id)
                    .and_then(|i| i.torrent.as_ref())
                    .is_some_and(|spec| self.torrent_host.busy(&spec.hash));
                if busy {
                    if !self.deferred_torrent_queue.contains(&id) {
                        self.deferred_torrent_queue.push(id);
                    }
                } else if !self.renaming_torrents.iter().any(|(rid, _, _)| *rid == id) {
                    let torrent_renaming = self.apply_deferred_rename(id);
                    if !torrent_renaming {
                        self.flush_pending_mark(id);
                        self.extracts.finished(id, &self.items, &self.settings);
                    }
                }
            }
        }

        let mut done = Vec::new();
        self.renaming_torrents
            .retain(|(id, from, rx)| match rx.try_recv() {
                Ok(r) => {
                    done.push((*id, from.clone(), r));
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(_) => false,
            });
        for (id, from, (r, sharing_error)) in done {
            self.apply_finished_torrent_rename(id, &from, r, sharing_error, true);
        }
    }

    fn flush_pending_mark(&mut self, id: u64) {
        let Some(pm) = self.item(id).and_then(|i| i.pending_mark.take()) else {
            return;
        };
        let files: Vec<PathBuf> = pm
            .main_file
            .into_iter()
            .chain(pm.side_files)
            .chain(pm.gallery_files)
            .collect();
        if !files.is_empty() {
            mark_finished(files, pm.url, pm.referer);
        }
    }

    /// Select the next download matching `q` after the selection: in the list as shown, else
    /// in all downloads (switching the view to All Downloads).
    fn find_next(&mut self, q: &crate::find::Query) {
        let shown = self.shown_ids();
        let found = q
            .next(&shown, &self.items, self.selection.focus())
            .or_else(|| {
                self.view = View::All;
                q.next(&self.shown_ids(), &self.items, self.selection.focus())
            });
        match found {
            Some(id) => {
                self.selection.set(id);
                self.list.scroll_to = self.shown_ids().iter().position(|x| *x == id);
            }
            None => {
                self.notice = Some((
                    crate::i18n::mark("Find file"),
                    crate::i18n::fmt(
                        crate::i18n::t("Cannot find \"{text}\""),
                        &[("text", q.text.trim())],
                    ),
                ))
            }
        }
    }

    /// Download video from a page, opened for a page (looked up at once) or from the menu: Save
    /// to is the Video category's folder, the Music category's while a sound-only choice is
    /// chosen (#611; not when the user deleted Music).
    fn video_dialog(&self, page: Option<(&str, mdm_ipc::Command)>) -> crate::videodlg::VideoDialog {
        crate::videodlg::refresh_ytdlp(&self.rt);
        let dir = self.settings.category_dir(Category::Video, &self.downloads);
        let music = (!Category::Music.deleted())
            .then(|| self.settings.category_dir(Category::Music, &self.downloads));
        match page {
            Some((page, cmd)) => crate::videodlg::VideoDialog::for_page(dir, page, cmd),
            None => crate::videodlg::VideoDialog::new(dir),
        }
        .with_subs(self.settings.video_subs.clone())
        .with_chapters(self.settings.video_chapters)
        .with_music_folder(music)
    }

    /// Download video from a page → Download Now / Later; or the browser's quality menu's
    /// choice (`from_menu`), which never asks for a queue ([`video_goes`]). Through the
    /// duplicate check first ([`Self::add_video_checked`]).
    fn add_video(&mut self, add: crate::videodlg::VideoAdd, from_menu: bool) {
        let folder = add
            .dest
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.downloads.clone());
        let name = add
            .dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "video".into());
        // yt-dlp takes any `<stem>.*` there as its own: a stem no file and no other download
        // in the folder has, whatever the extension (#32).
        let items = &self.items;
        let dest = mdm_engine::naming::stem_destination(&folder, &name, |stem| {
            items.iter().any(|i| {
                i.dest.parent() == Some(folder.as_path())
                    && i.dest
                        .file_stem()
                        .is_some_and(|s| s.to_string_lossy().to_lowercase() == stem.to_lowercase())
            })
        });
        let id = self.push_item(
            add.page,
            dest,
            add.category,
            String::new(),
            add.size,
            None,
            add.context,
            crate::i18n::mark("from a page (yt-dlp)"),
        );
        let goes = video_goes(add.start_now, from_menu, self.settings.queue_panel_later);
        if let Some(item) = self.item(id) {
            item.video = Some(model::VideoSpec {
                selector: add.selector,
                subs: add.subs,
                parts: Vec::new(),
            });
            // The quality menu's look-up goes with this new download only, started now and while
            // fresh: one queued for later, or added on a duplicate question answered late,
            // starts with a look-up of its own (the links will have expired).
            item.preloaded = kept_lookup(add.preloaded, goes, Instant::now());
        }
        self.selection.set(id);
        match goes {
            VideoGoes::Start => self.start(id, true),
            VideoGoes::AskQueue => self.queue_later(vec![id], true),
            VideoGoes::MainQueue => self.queue_later(vec![id], false),
        }
        self.dirty = true;
    }

    /// The newest download of `address` (a video's page as it was looked up, or its file),
    /// selected and scrolled to, in All Downloads when the view shown hides it (the button's
    /// "Show", #703).
    fn select_download_of(&mut self, address: &str) {
        let looked = mdm_tools::pages::video_address(address.trim());
        let found = self
            .items
            .iter()
            .rev()
            .find(|i| i.url.as_str() == address.trim() || i.url.as_str() == looked)
            .map(|i| i.id);
        let Some(id) = found else {
            return;
        };
        if !self.shown_ids().contains(&id) {
            self.view = View::All;
        }
        self.selection.set(id);
        self.list.scroll_to = self.shown_ids().iter().position(|x| *x == id);
    }

    /// The browser's quality menu over a video (crates/nmhost, videolookup.rs): a look-up
    /// started (answered at once, yt-dlp on a thread of its own), a question about one, or the
    /// choice made there, added as Download video's Download Now adds it: MDM's window stays
    /// where it is, the download's progress window opens (Options' "only add to the queue":
    /// into the main queue instead, with no queue panel either).
    fn quality_menu(&mut self, cmd: &ipc::Command) -> ipc::api::VideoLookupAnswer {
        use ipc::api::{VideoLookupAnswer, lookup};
        if menu_needs_window(cmd, self.headless) {
            return VideoLookupAnswer::of(lookup::USE_WINDOW);
        }
        let now = Instant::now();
        if let Some(ask) = &cmd.video_lookup {
            crate::videodlg::refresh_ytdlp(&self.rt);
            let Some(tool) = mdm_tools::tools_dir()
                .as_deref()
                .and_then(mdm_tools::installed)
            else {
                return VideoLookupAnswer::failed(lookup::NO_YTDLP, None);
            };
            return self.lookups.start(ask, cmd, tool.exe, self.rt.clone(), now);
        }
        if let Some(token) = &cmd.video_poll {
            return self.lookups.poll(token, now);
        }
        let Some(pick) = &cmd.video_pick else {
            return VideoLookupAnswer::of(lookup::USE_WINDOW);
        };
        let folder = self.settings.category_dir(Category::Video, &self.downloads);
        // A sound-only choice saves in Music, as in the Download video window (#611).
        let music = (!Category::Music.deleted())
            .then(|| self.settings.category_dir(Category::Music, &self.downloads));
        let start_now = !self.settings.only_add_to_queue;
        let add = self.lookups.pick(pick, cmd.private, now).and_then(|p| {
            crate::videolookup::video_add(
                p,
                &folder,
                music.as_deref(),
                start_now,
                &self.settings.video_subs,
            )
        });
        let Some(add) = add else {
            return VideoLookupAnswer::of(lookup::USE_WINDOW);
        };
        // Its category's folder, made when it isn't there yet (the window asks first; the
        // choice was made in the browser).
        if let Some(dir) = add.dest.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Already in the list in this quality: Options' "If a duplicate download link is added".
        let step = self.add_video_checked(add, true);
        VideoLookupAnswer::of(menu_answer(step, start_now))
    }

    /// A playlist's videos (#69), one download each, into its folder: names picked with the
    /// folder read once (naming::Stems), added at once and saved once, then queued (Download
    /// Now starts the main queue, so its "N at the same time" applies).
    fn add_playlist(&mut self, add: crate::playlistdlg::PlaylistAdd) {
        let started = Instant::now();
        let folder = add.folder;
        let in_use = self
            .items
            .iter()
            .filter(|i| i.dest.parent() == Some(folder.as_path()))
            .filter_map(|i| i.dest.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let mut stems = mdm_engine::naming::Stems::new(&folder, in_use);
        self.items.reserve(add.entries.len());
        let mut ids = Vec::with_capacity(add.entries.len());
        for (page, title) in add.entries {
            let stem = mdm_engine::naming::sanitize(&title);
            let dest = stems.pick(&format!("{stem}.{}", add.ext));
            let id = self.push_item(
                page,
                dest,
                add.category,
                String::new(),
                None,
                None,
                add.context.clone(),
                // i18n: where a download came from, in its log: a playlist or channel, by yt-dlp
                crate::i18n::mark("from a playlist (yt-dlp)"),
            );
            // push_item adds at the end: no search through the list for each one.
            if let Some(item) = self.items.last_mut() {
                item.video = Some(model::VideoSpec {
                    selector: add.selector.clone(),
                    subs: Default::default(),
                    parts: Vec::new(),
                });
            }
            ids.push(id);
        }
        let Some(&first) = ids.first() else { return };
        self.group(&ids);
        self.selection.set(first);
        if add.start_now {
            self.queues[0].items.extend(ids.iter().copied());
            self.start_queue(0);
        } else {
            self.queue_later(ids.clone(), self.settings.queue_panel_later);
        }
        self.dirty = true;
        eprintln!(
            "playlist: {} downloads added in {} ms",
            ids.len(),
            started.elapsed().as_millis()
        ); // not translated: a log line
    }

    /// The speed limit a video or gallery starts with now (`helper_rate`); the limits it is under
    /// are kept to tell a change (`reapply_helper_limits`). A limit set in its progress window lasts until it is stopped:
    /// a start other than for a new limit forgets it.
    fn helper_starts(&mut self, id: u64) -> Option<u64> {
        // Started again for a new limit (by `limit_restart_tick`, or its queue first): done.
        if let Some(at) = self.limit_restarts.iter().position(|x| *x == id) {
            self.limit_restarts.remove(at);
        } else {
            self.helper_own.remove(&id);
        }
        self.helper_rates.insert(id, self.helper_limits(id));
        let rate = self.helper_rate(id);
        self.helper_started.insert(id, rate);
        rate
    }

    /// Start a video item: yt-dlp (installed and verified) finds it, MDM's engine downloads its
    /// streams (#769).
    fn start_video(&mut self, id: u64, show: bool) {
        self.claim_place(id);
        let tools = mdm_tools::tools_dir();
        let tool = tools.as_deref().and_then(mdm_tools::installed);
        let ffmpeg = tools.as_deref().and_then(mdm_tools::ffmpeg::installed);
        let rate = self.helper_starts(id);
        // The engine downloads the streams with what a file from the page gets (#769): Options'
        // connections, proxy, network adapter and DNS, the limit for all downloads (shared with
        // every download, not a helper's share), the hosts' learned limits; the video's own
        // speed limit, else the Speed Limiter's for one file. `rate` is yt-dlp's when it
        // downloads the video itself.
        let one_file = self.helper_limits(id).0;
        let engine = self.items.iter().find(|i| i.id == id).map(|i| {
            let mut opts = self.request_parts(&i.url, None, &i.context, true).opts;
            opts.speed_limit = one_file;
            mdm_tools::streams::Engine {
                handle: self.rt.clone(),
                opts,
            }
        });
        let Some(item) = self.item(id) else { return };
        let Some(spec) = item.video.clone() else {
            return;
        };
        let Some(tool) = tool else {
            item.status = Status::Error;
            item.error = Some(
                crate::i18n::t(
                    "yt-dlp is not installed (Tasks → Download video from a page installs it)",
                )
                .into(),
            );
            self.dirty = true;
            return;
        };
        let folder = item.dest.parent().map(PathBuf::from).unwrap_or_default();
        let stem = item
            .dest
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "video".into());
        let _ = std::fs::create_dir_all(&folder);
        // The quality menu's look-up, for this first run only, while it is fresh (a run started
        // again, or later than the menu keeps its look-ups, reads the page afresh).
        let preloaded = item
            .preloaded
            .take()
            .filter(|p| p.slot.born().elapsed() < crate::videolookup::TTL);
        let started = mdm_tools::video::VideoJob::start_with(
            &tool.exe,
            item.url.as_str(),
            &spec.selector,
            &folder,
            &stem,
            rate,
            ffmpeg.as_ref().map(|f| f.exe.as_path()),
            item.size,
            &crate::videodlg::from_browser(&item.context),
            &spec.subs,
            preloaded,
            engine,
        );
        match started {
            Ok(job) => {
                item.note(crate::i18n::fmt(
                    crate::i18n::t("Started with yt-dlp {version} ({format})"),
                    &[("version", &tool.version), ("format", &spec.selector)],
                ));
                let url = item.url.clone();
                item.live = Some(Live::new(model::Job::Video { job, url }, item.downloaded));
                // This run's stream addresses are new ones (signed, they expire): the windows
                // show the page until the engine has the first (#795).
                item.file_url = None;
                item.status = Status::Downloading;
                item.error = None;
                item.last_try = Some(model::now_stamp());
            }
            Err(e) => {
                item.status = Status::Error;
                item.error = Some(e.to_string());
            }
        }
        self.dirty = true;
        if show && !self.progress.iter().any(|p| p.id == id) {
            self.progress
                .push(Progress::new(id, self.settings.show_complete));
        }
    }

    /// Download a gallery → Download Now / Later (#58): one row for the gallery, its folder its
    /// own.
    fn add_gallery(&mut self, add: crate::gallerydlg::GalleryAdd) {
        let preexisting = crate::deferred_rename::folder_has_any_entries(&add.folder);
        let id = self.push_item(
            add.page,
            add.folder,
            Category::General,
            String::new(),
            None,
            None,
            Default::default(),
            // i18n: where a download came from, in its log: an image gallery, by gallery-dl
            crate::i18n::mark("from a gallery (gallery-dl)"),
        );
        if let Some(item) = self.items.last_mut() {
            item.file_name = add.name;
            item.gallery = Some(model::GallerySpec { limit: add.limit });
            item.preexisting_folder = preexisting;
        }
        self.selection.set(id);
        if add.start_now {
            self.start(id, true);
        } else {
            self.queue_later(vec![id], self.settings.queue_panel_later);
        }
        self.dirty = true;
    }

    /// Start (or start again) a gallery: gallery-dl, installed and verified, or the person's.
    fn start_gallery(&mut self, id: u64, show: bool) {
        let tool = mdm_tools::tools_dir()
            .as_deref()
            .and_then(mdm_tools::gallery::installed);
        let rate = self.helper_starts(id);
        // The engine downloads the files gallery-dl lists (#769), as a file from the page would
        // be (Options' connections, proxy, network adapter, DNS, the limit for all downloads),
        // under the gallery's own limit, else the Speed Limiter's for one file.
        let one_file = self.helper_limits(id).0;
        let engine = self.items.iter().find(|i| i.id == id).map(|i| {
            let mut opts = self.request_parts(&i.url, None, &i.context, true).opts;
            opts.speed_limit = one_file;
            mdm_tools::streams::Engine {
                handle: self.rt.clone(),
                opts,
            }
        });
        let Some(item) = self.item(id) else { return };
        let Some(spec) = item.gallery.clone() else {
            return;
        };
        let Some(tool) = tool else {
            item.status = Status::Error;
            item.error = Some(
                crate::i18n::t(
                    // i18n: a gallery download's error: the helper program it needs is missing
                    "gallery-dl is not installed (Add URL with a gallery's address installs it)",
                )
                .into(),
            );
            self.dirty = true;
            return;
        };
        let ask = mdm_tools::gallery::Ask {
            limit: spec.limit,
            rate,
        };
        let started = mdm_tools::gallery::GalleryJob::start(
            &tool,
            item.url.as_str(),
            &item.dest,
            &ask,
            &crate::videodlg::from_browser(&item.context),
            engine,
        );
        match started {
            Ok(job) => {
                item.note(crate::i18n::fmt(
                    // i18n: a gallery download's log: {version} = gallery-dl's version
                    crate::i18n::t("Started with gallery-dl {version}"),
                    &[("version", &tool.version)],
                ));
                let url = item.url.clone();
                // A gallery counts what it saves this run.
                item.downloaded = 0;
                item.size = None;
                item.live = Some(Live::new(model::Job::Gallery { job, url }, 0));
                item.status = Status::Downloading;
                item.error = None;
                item.last_try = Some(model::now_stamp());
            }
            Err(e) => {
                item.status = Status::Error;
                item.error = Some(e.to_string());
            }
        }
        self.dirty = true;
        if show && !self.progress.iter().any(|p| p.id == id) {
            self.progress
                .push(Progress::new(id, self.settings.show_complete));
        }
    }

    /// The info hashes of every torrent in the list (also used to prevent duplicate adds).
    fn torrent_hashes(&self) -> std::collections::HashSet<String> {
        self.items
            .iter()
            .filter_map(|i| i.torrent.as_ref().map(|t| t.hash.clone()))
            .collect()
    }

    /// The hashes librqbit should keep or restore in its session.
    fn session_hashes(&self) -> std::collections::HashSet<String> {
        let limits = self.settings.sharing_limits();
        self.items
            .iter()
            .filter(|i| {
                i.status != Status::Complete
                    || (self.settings.torrent_keep_sharing
                        && i.torrent.as_ref().is_some_and(|spec| {
                            crate::torrents::sharing_limit_reached(spec, i.downloaded, limits)
                                .is_none()
                        }))
            })
            .filter_map(|i| i.torrent.as_ref().map(|t| t.hash.clone()))
            .collect()
    }

    /// The list's finished torrents (what "keep sharing" shares).
    /// The finished torrents to share (Torrent settings' "Keep sharing"): those that haven't
    /// reached a seeding limit.
    fn complete_torrents(&self) -> Vec<String> {
        let limits = self.settings.sharing_limits();
        self.items
            .iter()
            .filter(|i| i.status == Status::Complete)
            .filter_map(|i| i.torrent.as_ref().map(|t| (t, i.downloaded)))
            .filter(|(t, done)| crate::torrents::sharing_limit_reached(t, *done, limits).is_none())
            .map(|(t, _)| t.hash.clone())
            .collect()
    }

    fn open_torrent_dialog(&mut self, source: Option<String>, ctx: &egui::Context) {
        self.open_torrent_dialog_with(source, None, ctx);
    }

    /// Add torrent for a .torrent on the web (#39): fetched with `remote`'s request. A magnet
    /// link goes straight to the debrid account when torrents go through it (#44): Add torrent
    /// would ask peers for what it holds.
    fn open_torrent_dialog_with(
        &mut self,
        source: Option<String>,
        remote: Option<crate::torrents::Remote>,
        ctx: &egui::Context,
    ) {
        if remote.is_none()
            && self.torrent_account().is_some()
            && let Some(magnet) = source
                .as_deref()
                .and_then(mdm_torrent::Source::magnet)
                .and_then(|s| match s {
                    mdm_torrent::Source::Magnet(m) => url::Url::parse(&m).ok(),
                    mdm_torrent::Source::File(_) => None,
                })
        {
            let dir = self
                .settings
                .category_dir(Category::Torrents, &self.downloads);
            self.add_remote_torrent(magnet, dir, Category::Torrents, true, None);
            self.show_window(ctx);
            return;
        }
        if let Some(open) = &self.torrent_dlg {
            // One Add torrent at a time: a torrent handed over meanwhile waits its turn (#507).
            // Add torrent… from the menu with it open (no source) has nothing to wait for.
            if let Some(source) = source
                && open.source() != source
                && !self.torrent_waiting.iter().any(|(s, _)| *s == source)
            {
                tracing::info!(
                    "Add torrent is open: a torrent waits ({} waiting)",
                    self.torrent_waiting.len() + 1
                );
                self.torrent_waiting.push_back((source, remote));
            }
            return;
        }
        self.torrent_dlg = Some(crate::torrentdlg::TorrentDialog::new(
            source,
            remote,
            self.torrent_host.clone(),
            self.session_hashes(),
            self.downloads.clone(),
            self.settings.category_dirs.clone(),
            &self.rt,
            ctx,
        ));
    }

    /// The debrid account torrents go to (#44): Options → Accounts' "Torrents and magnet links
    /// through the first account", with an account there.
    fn torrent_account(&self) -> Option<(u64, mdm_debrid::ServiceKind)> {
        (self.settings.debrid_on && self.settings.debrid_torrents)
            .then(|| self.debrid.torrent_account())
            .flatten()
    }

    /// A torrent through the debrid account (#44): a row whose files come over HTTP once the
    /// service has them, into `dir` (a torrent of several files in a folder of its own there).
    fn add_remote_torrent(
        &mut self,
        magnet: url::Url,
        dir: PathBuf,
        category: Category,
        start_now: bool,
        renamed: Option<String>,
    ) {
        let Some((account, service)) = self.torrent_account() else {
            return;
        };
        let name = renamed
            .as_deref()
            .map(mdm_engine::naming::sanitize)
            .filter(|n| !n.is_empty())
            .or_else(|| {
                magnet
                    .query_pairs()
                    .find(|(k, _)| k == "dn")
                    .map(|(_, v)| mdm_engine::naming::sanitize(&v))
                    .filter(|n| !n.is_empty())
            })
            .unwrap_or_else(|| "torrent".to_owned());
        let target = dir.join(&name);
        let preexisting = crate::deferred_rename::folder_has_any_entries(&target);
        let id = self.push_item(
            magnet,
            target,
            category,
            String::new(),
            None,
            None,
            Default::default(),
            crate::i18n::mark("from a magnet link"),
        );
        if let Some(item) = self.item(id) {
            item.preexisting_folder = preexisting;
            item.note(crate::i18n::fmt(
                // i18n: event log: a torrent goes through the user's debrid account, not to peers; {service} its name ("Real-Debrid")
                crate::i18n::t(
                    "Through {service}: the service downloads the torrent, then MDM its files",
                ),
                &[("service", service.name())],
            ));
            item.remote = Some(model::RemoteSpec {
                account,
                transfer: None,
                folder: dir,
                renamed,
            });
        }
        self.selection.set(id);
        if start_now {
            self.start(id, true);
        } else {
            self.queue_later(vec![id], self.settings.queue_panel_later);
        }
        self.dirty = true;
    }

    /// Start (or continue) a torrent through its debrid account (#44).
    fn start_remote(&mut self, id: u64, show: bool) {
        let limit = self.settings.speed_limit();
        let Some((url, spec, own)) = self
            .item(id)
            .and_then(|i| Some((i.url.clone(), i.remote.clone()?, i.own_limit)))
        else {
            return;
        };
        // The files' requests: Options' connections, proxy, DNS and adapter; the service's own
        // unlocking is the torrent job's (not a host's link).
        let mut net = self
            .request_parts(&url, None, &Default::default(), true)
            .opts;
        net.speed_limit = own.or(limit);
        let rt = self.rt.clone();
        let _guard = rt.enter();
        let job = mdm_debrid::torrents::RemoteTorrent::start(
            &self.debrid,
            mdm_debrid::torrents::Order {
                account: spec.account,
                magnet: url,
                transfer: spec.transfer.clone(),
                folder: spec.folder.clone(),
                renamed: spec.renamed.clone(),
                net,
                poll: std::time::Duration::from_secs(5),
            },
        );
        let Some(item) = self.item(id) else { return };
        item.note(if spec.transfer.is_some() {
            crate::i18n::t("Resumed")
        } else {
            crate::i18n::t("Started")
        });
        item.live = Some(Live::new(model::Job::Remote(job), item.downloaded));
        item.status = Status::Downloading;
        item.error = None;
        item.last_try = Some(model::now_stamp());
        self.dirty = true;
        if show && !self.progress.iter().any(|p| p.id == id) {
            self.progress
                .push(Progress::new(id, self.settings.show_complete));
        }
    }

    /// Add torrent → Download Now / Later. Through the debrid account when torrents go there
    /// (#44): the .torrent's magnet link (its hash, name and trackers), all its files. The
    /// download added, if one was (not when the debrid account takes it).
    fn add_torrent(&mut self, add: crate::torrentdlg::TorrentAdd) -> Option<u64> {
        let meta = &add.meta;
        crate::torrent_rename::on_add_torrent(meta);
        if self.torrent_account().is_some()
            && let Some(magnet) =
                mdm_torrent::magnet_link(&meta.torrent).and_then(|m| url::Url::parse(&m).ok())
        {
            let renamed = crate::torrent_rename::renamed_target(meta);
            self.add_remote_torrent(
                magnet,
                add.dir.clone(),
                add.category,
                add.start_now,
                renamed,
            );
            return None;
        }
        if let Err(e) = crate::torrents::save_torrent(&meta.info_hash, &meta.torrent) {
            self.notice = Some((
                crate::i18n::mark("Add torrent"),
                crate::i18n::fmt(
                    crate::i18n::t("The torrent could not be saved: {error}"),
                    &[("error", &e.to_string())],
                ),
            ));
            return None;
        }
        let folder = meta.folder_in(&add.dir);
        let dest = crate::torrents::dest_for(meta, &add.dir, add.files.as_deref());
        let preexisting =
            if dest == folder && crate::deferred_rename::folder_has_any_entries(&folder) {
                let allowed: Vec<PathBuf> = meta
                    .files
                    .iter()
                    .enumerate()
                    .filter(|(i, f)| {
                        f.padding || add.files.as_ref().is_none_or(|chosen| chosen.contains(i))
                    })
                    .map(|(_, f)| folder.join(&f.path))
                    .collect();
                crate::deferred_rename::folder_has_untracked_files(&folder, &allowed)
            } else {
                false
            };
        let size = meta
            .files
            .iter()
            .enumerate()
            .filter(|(i, f)| !f.padding && add.files.as_ref().is_none_or(|c| c.contains(i)))
            .map(|(_, f)| f.size)
            .sum();
        let id = self.push_item(
            crate::torrents::magnet_url(&meta.info_hash, &meta.name),
            dest,
            add.category,
            String::new(),
            Some(size),
            None,
            Default::default(),
            if add.from_magnet {
                crate::i18n::mark("from a magnet link")
            } else {
                crate::i18n::mark("from a torrent file")
            },
        );
        if let Some(item) = self.item(id) {
            item.preexisting_folder = preexisting;
            item.torrent = Some(crate::torrents::TorrentSpec {
                hash: meta.info_hash.clone(),
                files: add.files.clone(),
                folder,
                uploaded: 0,
                seeded_secs: 0,
                peers: 0,
                up_speed: 0.0,
                upload_limit: None,
                in_order: add.in_order,
                ends_first: true,
            });
        }
        self.selection.set(id);
        if add.start_now {
            self.start(id, !add.quiet);
        } else if let Some(q) = add.queue.filter(|q| *q < self.queues.len()) {
            self.queues[q].items.push(id);
        } else {
            self.queue_later(vec![id], self.settings.queue_panel_later);
        }
        self.dirty = true;
        Some(id)
    }

    /// Right-click → Torrent → Download in order (`in_order`), or its first and last pieces
    /// first (#39): kept with the torrent, and applied now if it runs (the next piece each peer
    /// is asked for follows it).
    fn toggle_torrent_order(&mut self, id: u64, in_order: bool) {
        let Some(item) = self.item(id) else { return };
        let Some(spec) = item.torrent.as_mut() else {
            return;
        };
        if in_order {
            spec.in_order = !spec.in_order;
        } else {
            spec.ends_first = !spec.ends_first;
        }
        let order = spec.order();
        if let Some(model::Job::Torrent(run)) = item.live.as_ref().map(|l| &l.dl) {
            run.set_order(order);
        }
        self.dirty = true;
    }

    /// Start a torrent: the session adds or resumes it.
    fn start_torrent(&mut self, id: u64, show: bool) {
        let auto_share = self.settings.torrent_keep_sharing;
        let known = self.session_hashes();
        let limit = self.settings.speed_limit();
        let (host, rt) = (self.torrent_host.clone(), self.rt.clone());
        let Some(item) = self.item(id) else { return };
        // An explicit Start on a completed download means seed it, even when automatic
        // keep sharing is off. It remains in the session until Stop or a sharing limit.
        let share = auto_share
            || item
                .size
                .is_some_and(|size| size > 0 && item.downloaded >= size);
        // Its own remembered limit first, as a file download's (#39).
        let limit = item.own_limit.or(limit);
        let Some(spec) = item.torrent.clone() else {
            return;
        };
        let run =
            crate::torrents::Run::start(&host, &rt, &spec, item.url.clone(), known, limit, share);
        item.note(if item.downloaded > 0 {
            // i18n: event log entry, past tense: the download resumed (just now)
            crate::i18n::t("Resumed")
        } else {
            // i18n: event log entry, past tense: the download started (just now)
            crate::i18n::t("Started")
        });
        item.live = Some(Live::new(model::Job::Torrent(run), item.downloaded));
        item.status = Status::Downloading;
        item.error = None;
        item.last_try = Some(model::now_stamp());
        self.dirty = true;
        if show && !self.progress.iter().any(|p| p.id == id) {
            self.progress
                .push(Progress::new(id, self.settings.show_complete));
        }
    }

    /// A torrent action (#39) on download `id`, in the background; what it did goes in the
    /// download's event log (`torrent_outcomes`). One at a time per torrent.
    /// RSS (#39), every 2 s: what fetches and look-ups sent back (new articles through the
    /// rules, the torrents they took added), then the feeds that are due fetched.
    fn rss_tick(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let events = std::mem::take(&mut *self.rss_events.lock().unwrap());
        let mut save = false;
        for ev in events {
            match ev {
                RssEvent::Fetched(id, res) => {
                    self.rss_busy.remove(&id);
                    save = true;
                    match res {
                        Ok(parsed) => {
                            for (rule, article) in self.rss.fetched(id, parsed, now) {
                                self.rss_look_up(rule, id, article);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("rss: feed {id}: {e}");
                            self.rss.failed(id, e, now);
                        }
                    }
                }
                RssEvent::Looked {
                    rule,
                    feed,
                    title,
                    res,
                } => self.rss_add(rule, feed, &title, res),
            }
        }
        for id in self.rss.due(now) {
            self.rss_fetch(id);
        }
        if save {
            self.save_rss();
        }
        if let Some(Err(e)) = self.rss_saver.outcome() {
            tracing::warn!("rss: not saved: {e}");
        }
    }

    /// On the RSS saver's thread; how it went is logged at the next tick (`rss_tick`).
    fn save_rss(&self) {
        let Some(path) = crate::rss::Store::path() else {
            return;
        };
        match self.rss.encode() {
            Ok(json) => self.rss_saver.queue(path, json),
            Err(e) => tracing::warn!("rss: {} not saved: {e}", path.display()),
        }
    }

    /// Fetch feed `id` in the background (once at a time), with MDM's own connection settings.
    fn rss_fetch(&mut self, id: u64) {
        let Some(feed) = self.rss.feeds.iter().find(|f| f.id == id) else {
            return;
        };
        if !self.rss_busy.insert(id) {
            return;
        }
        let url = feed.url.clone();
        let out = self.rss_events.clone();
        self.rt.spawn(async move {
            let res = match url::Url::parse(&url) {
                Ok(url) => {
                    let remote = crate::torrents::Remote {
                        url,
                        parts: crate::app::RequestParts {
                            headers: Default::default(),
                            opts: crate::settings::own_net(),
                        },
                    };
                    match crate::torrents::fetch_small(remote, crate::rss::MAX_FEED).await {
                        Ok(Some(bytes)) => crate::rss::parse(&String::from_utf8_lossy(&bytes)),
                        // i18n: RSS: a feed bigger than MDM reads (5 MB)
                        Ok(None) => Err(crate::i18n::t("larger than a feed can be").to_owned()),
                        Err(e) => Err(e),
                    }
                }
                Err(e) => Err(e.to_string()),
            };
            out.lock().unwrap().push(RssEvent::Fetched(id, res));
        });
    }

    /// A rule took an article: what its torrent holds is looked up (a magnet asks the swarm, a
    /// .torrent's address is fetched), then `rss_add` adds it.
    fn rss_look_up(&mut self, rule: u64, feed: u64, article: crate::rss::Article) {
        let (host, known, out) = (
            self.torrent_host.clone(),
            self.session_hashes(),
            self.rss_events.clone(),
        );
        self.rt.spawn(async move {
            let res = async {
                let src = match mdm_torrent::Source::magnet(&article.torrent) {
                    Some(m) => m,
                    None => {
                        let url =
                            url::Url::parse(article.torrent.trim()).map_err(|e| e.to_string())?;
                        let remote = crate::torrents::Remote {
                            url,
                            parts: crate::app::RequestParts {
                                headers: Default::default(),
                                opts: crate::settings::own_net(),
                            },
                        };
                        mdm_torrent::Source::File(crate::torrents::fetch_torrent(remote).await?)
                    }
                };
                let t = host.session(known).await?;
                t.metadata(&src, std::time::Duration::from_secs(180), None)
                    .await
                    .map_err(|e| crate::errors::torrent(&e))
            }
            .await;
            out.lock().unwrap().push(RssEvent::Looked {
                rule,
                feed,
                title: article.title,
                res,
            });
        });
    }

    /// Add what a rule took, as Add torrent's Download Now (or Later) would: every file, to the
    /// rule's folder (else the Torrents category's). Not when the list has it, or files of its
    /// names are already in the folder (#40: nothing is written into them unasked).
    fn rss_add(
        &mut self,
        rule: u64,
        feed: u64,
        title: &str,
        res: Result<mdm_torrent::Meta, String>,
    ) {
        let Some(r) = self.rss.rules.iter().find(|r| r.id == rule).cloned() else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let title_i = crate::i18n::isolate(title);
        let meta = match res {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("rss: rule {:?}: {title}: {e}", r.name);
                self.rss.note(
                    now,
                    crate::i18n::fmt(
                        // i18n: RSS feeds window, under the rules: a rule's article could not be added, with why
                        crate::i18n::t("{title}: not added: {error}"),
                        &[("title", &title_i), ("error", &e)],
                    ),
                );
                self.save_rss();
                return;
            }
        };
        if self.torrent_hashes().contains(&meta.info_hash) {
            self.rss.note(
                now,
                crate::i18n::fmt(
                    // i18n: RSS feeds window, under the rules: a rule's article is a torrent the list already has
                    crate::i18n::t("{title}: already in the list"),
                    &[("title", &title_i)],
                ),
            );
            self.save_rss();
            return;
        }
        let dir = if r.folder.trim().is_empty() {
            self.settings
                .category_dir(Category::Torrents, &self.downloads)
        } else {
            PathBuf::from(r.folder.trim())
        };
        if !meta.existing_in(&dir).is_empty() {
            tracing::warn!(
                "rss: rule {:?}: {title}: its files are already in {}",
                r.name,
                dir.display()
            );
            self.rss.note(
                now,
                crate::i18n::fmt(
                    // i18n: RSS feeds window, under the rules: not added, because files of its names are already in the folder (MDM never writes into them unasked)
                    crate::i18n::t(
                        "{title}: not added: files of its names are already in {folder}",
                    ),
                    &[
                        ("title", &title_i),
                        ("folder", &crate::i18n::ltr(&dir.display().to_string())),
                    ],
                ),
            );
            self.save_rss();
            return;
        }
        let hash = meta.info_hash.clone();
        let from_magnet = false;
        self.add_torrent(crate::torrentdlg::TorrentAdd {
            meta,
            dir,
            files: None,
            category: Category::Torrents,
            start_now: !r.later,
            from_magnet,
            // RSS rules download as torrents usually do: the rarest pieces first.
            in_order: false,
            queue: None,
            quiet: false,
        });
        let feed = self
            .rss
            .feeds
            .iter()
            .find(|f| f.id == feed)
            .map(|f| f.name().to_owned())
            .unwrap_or_default();
        self.rss.note(
            now,
            crate::i18n::fmt(
                // i18n: RSS feeds window, under the rules: {title} was added by the rule {rule}
                crate::i18n::t("{title}: added by {rule}"),
                &[
                    ("title", &title_i),
                    ("rule", &crate::i18n::isolate(&r.name)),
                ],
            ),
        );
        self.save_rss();
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|i| i.torrent.as_ref().is_some_and(|t| t.hash == hash))
        {
            item.note(crate::i18n::fmt(
                // i18n: event log entry: an RSS download rule added this torrent; {rule} is the rule's name, {feed} the feed's
                crate::i18n::t("Added by the RSS rule {rule} from {feed}"),
                &[
                    ("rule", &crate::i18n::isolate(&r.name)),
                    ("feed", &crate::i18n::isolate(&feed)),
                ],
            ));
        }
    }

    fn torrent_act(&mut self, id: u64, act: crate::torrents::Act) {
        let known = self.session_hashes();
        let Some(spec) = self
            .items
            .iter()
            .find(|i| i.id == id)
            .and_then(|i| i.torrent.clone())
        else {
            return;
        };
        if self.torrent_host.busy(&spec.hash) {
            return;
        }
        self.torrent_host.act(&self.rt, known, &spec, act);
    }

    /// Files ticked or unticked in a torrent's Files tab (#39): the torrent downloads `files`
    /// from now on (`all`: every one), `size` together. Kept with it (`TorrentSpec::files`); the
    /// session changes now. One that isn't running in the list and gains files is held paused
    /// (Resume fetches them): nothing downloads out of sight.
    fn choose_files(&mut self, id: u64, files: Vec<usize>, all: bool, size: u64) {
        let Some(item) = self.item(id) else { return };
        let live = item.live.is_some();
        let Some(spec) = item.torrent.as_mut() else {
            return;
        };
        let before: Option<Vec<usize>> = spec.files.clone();
        let gains = match &before {
            None => false,
            Some(had) => files.iter().any(|f| !had.contains(f)),
        };
        spec.files = if all { None } else { Some(files.clone()) };
        if spec.files == before {
            return;
        }
        item.size = Some(size);
        let n = files.len() as u64;
        // i18n: event log entry: the files ticked in a torrent's Files tab changed; {n} of them are ticked now
        let note = crate::i18n::tn(
            n,
            "{n} file chosen to download",
            "{n} files chosen to download",
        );
        item.note(note);
        self.dirty = true;
        self.torrent_act(
            id,
            crate::torrents::Act::Choose {
                files,
                pause: !live && gains,
            },
        );
    }

    /// The torrent actions done since the last tick, in their downloads' event logs; after a
    /// Recheck or a change of files, the list's numbers follow the session (`torrent_resync`).
    fn torrent_outcomes(&mut self) {
        use crate::torrents::{Act, Done};
        for o in self.torrent_host.take_outcomes() {
            let Some(item) = self
                .items
                .iter_mut()
                .find(|i| i.torrent.as_ref().is_some_and(|t| t.hash == o.hash))
            else {
                continue;
            };
            let note = match (&o.act, o.res) {
                (_, Ok(Done::Rechecking)) => {
                    self.torrent_resync.insert(o.hash.clone(), true);
                    // i18n: event log entry: a torrent's files are being read and checked again (Recheck)
                    crate::i18n::t("Checking every piece again").to_owned()
                }
                (_, Ok(Done::Reannounced)) => {
                    // i18n: event log entry: the torrent's trackers were asked for peers again (Reannounce)
                    crate::i18n::t("Asked the trackers for peers again").to_owned()
                }
                (_, Ok(Done::TrackersAdded(0))) => {
                    // i18n: event log entry: Add trackers changed nothing, the torrent had them all
                    crate::i18n::t("No tracker added: it has them already").to_owned()
                }
                (_, Ok(Done::TrackersAdded(n))) => {
                    // i18n: event log entry: trackers were added to the torrent
                    crate::i18n::tn(n as u64, "{n} tracker added", "{n} trackers added")
                }
                (_, Ok(Done::Chosen)) => {
                    self.torrent_resync.insert(o.hash.clone(), false);
                    continue;
                }
                (act, Err(e)) => crate::i18n::fmt(
                    match act {
                        // i18n: event log entry: Recheck failed, with the reason
                        Act::Recheck => crate::i18n::t("Recheck failed: {error}"),
                        // i18n: event log entry: Reannounce failed, with the reason
                        Act::Reannounce => crate::i18n::t("Reannounce failed: {error}"),
                        // i18n: event log entry: Add trackers failed, with the reason
                        Act::AddTrackers(_) => {
                            crate::i18n::t("The trackers could not be added: {error}")
                        }
                        // i18n: event log entry: the files chosen in the Files tab could not be applied, with the reason
                        Act::Choose { .. } => {
                            crate::i18n::t("The files could not be chosen: {error}")
                        }
                    },
                    &[("error", &e)],
                ),
            };
            item.note(note);
            self.dirty = true;
        }
    }

    /// Add batch download from clipboard (#709), once the clipboard's text came (`None`: no
    /// text): its links in Link grabber (with those there already), a numbered address in Batch
    /// download, or a note that says why nothing opens.
    fn batch_from_clipboard(&mut self, text: Option<&str>, ctx: &egui::Context) {
        let why = match crate::clipbatch::read(text) {
            crate::clipbatch::Clip::Links(found) => {
                tracing::info!("add batch download from clipboard: {} links", found.len());
                // i18n: Link grabber: where the links came from: the clipboard (Tasks → Add batch download from clipboard)
                let source = crate::i18n::t("Clipboard").to_owned();
                self.show_import(found, source);
                dialogs::raise(ctx, dialogs::dialog_viewport("grabber"));
                return;
            }
            crate::clipbatch::Clip::Pattern(p) => {
                tracing::info!(
                    "add batch download from clipboard: the pattern {}",
                    p.address
                );
                if self.batch.is_none() {
                    self.batch = Some(crate::batch::Batch::with_pattern(p));
                }
                dialogs::raise(ctx, dialogs::dialog_viewport("batch"));
                return;
            }
            crate::clipbatch::Clip::Empty => crate::i18n::t(
                "The clipboard holds no text. Copy some links, or a text with links in it, and try again.",
            ),
            crate::clipbatch::Clip::NoLinks => crate::i18n::t(
                "No download links were found in the clipboard. Copy a text with http, https, ftp or magnet links in it, and try again.",
            ),
        };
        tracing::info!("add batch download from clipboard: nothing to download");
        self.notice = Some((
            // i18n: title of the note when Tasks → Add batch download from clipboard finds nothing to download
            crate::i18n::mark("Add batch download from clipboard"),
            why.to_owned(),
        ));
    }

    /// Batch download → OK: every address goes to the main download queue (IDM's default; its
    /// queue selection panel is not built yet).
    fn add_batch(&mut self, added: crate::batch::Added) {
        let mut ids = Vec::new();
        for url in added.urls {
            let name = mdm_engine::naming::file_name(None, &url);
            let category = Category::of(&name);
            let dir = self.settings.category_dir(category, &self.downloads);
            let dest = mdm_engine::naming::destination(&dir, &name);
            let id = self.push_item(
                url,
                dest,
                category,
                String::new(),
                None,
                added.login.clone(),
                Default::default(),
                crate::i18n::mark("by batch download"),
            );
            self.name_guessed(id, true);
            ids.push(id);
        }
        self.group(&ids);
        self.queue_later(ids, self.settings.queue_panel_batch);
        self.dirty = true;
    }

    /// The automation API's answer (a JSON line, shaped by `ipc::api`): `api` turns it on or
    /// off; `query: "list"` gives every download and the total speed, `action` acts on
    /// downloads ([`App::act`]) — only while it is on.
    ///
    /// A paired device's command (the remote control, #141) is answered while Options → Remote
    /// is on, whatever the automation API's switch: it may list, add and act, nothing else.
    fn answer(&mut self, cmd: &ipc::Command) -> String {
        use ipc::api::{ApiAnswer, DownloadInfo, ErrorAnswer, ListAnswer, line};
        // The browser's quality menu (its native host asks): whatever the automation API's
        // switch, and never for a paired device.
        if !cmd.remote
            && (cmd.video_lookup.is_some() || cmd.video_poll.is_some() || cmd.video_pick.is_some())
        {
            return line(&self.quality_menu(cmd));
        }
        if cmd.pair && !cmd.remote {
            return self.pair_answer();
        }
        if cmd.remote {
            if !self.settings.remote_on {
                return line(&ErrorAnswer {
                    error: "the remote control is off".into(),
                    code: Some("remote-off".into()),
                });
            }
            if cmd.url.is_some() || cmd.torrent_file.is_some() {
                return self.remote_add(cmd.clone());
            }
        } else if let Some(on) = cmd.api {
            self.settings.automation_api = on;
            self.dirty = true;
            return line(&ApiAnswer { api: on });
        } else if !self.settings.automation_api {
            return line(&ErrorAnswer {
                error: "the automation API is off (misterdownloadmanager-cli api on)".into(),
                code: Some(ipc::api::API_OFF.into()),
            });
        }
        if let Some(action @ ("start_queue" | "stop_queue")) = cmd.action.as_deref() {
            let at = cmd
                .queue
                .as_deref()
                .and_then(|n| self.queues.iter().position(|q| q.name == n));
            let Some(q) = at else {
                return line(&ErrorAnswer {
                    error: "no such queue".into(),
                    code: Some("unknown-queue".into()),
                });
            };
            if action == "start_queue" {
                self.start_queue(q);
            } else {
                self.stop_queue(q);
            }
            self.dirty = true;
            return line(&ipc::api::ActAnswer::default());
        }
        if let Some(action) = cmd.action.as_deref() {
            return match self.act(action, &cmd.ids) {
                Ok(done) => line(&done),
                Err(e) => line(&e),
            };
        }
        match cmd.query.as_deref() {
            Some("list") => {
                let downloads = self
                    .items
                    .iter()
                    .map(|i| DownloadInfo {
                        id: i.id,
                        file: i.file_name.clone(),
                        // A paired device sees no folder of the computer's, nor a password in
                        // an address.
                        url: if cmd.remote {
                            let mut u = i.url.clone();
                            let _ = u.set_password(None);
                            u.to_string()
                        } else {
                            i.url.to_string()
                        },
                        path: if cmd.remote {
                            String::new()
                        } else {
                            i.dest.to_string_lossy().into_owned()
                        },
                        status: format!("{:?}", i.status),
                        size: i.size,
                        downloaded: i.downloaded,
                        speed: if i.live.is_some() { i.rate() as u64 } else { 0 },
                        added: i.added.clone(),
                        category: i.category.key().into_owned(),
                        error: i.error.clone().filter(|_| i.status == Status::Error),
                    })
                    .collect();
                let speed: f64 = self
                    .items
                    .iter()
                    .filter(|i| i.live.is_some())
                    .map(|i| i.rate())
                    .sum();
                let queues = self
                    .queues
                    .iter()
                    .map(|q| ipc::api::QueueInfo {
                        name: q.name.clone(),
                        running: q.running,
                        ids: q.items.clone(),
                    })
                    .collect();
                let categories = Category::all()
                    .into_iter()
                    .map(|c| ipc::api::CategoryInfo {
                        key: c.key().into_owned(),
                        name: c.label().into_owned(),
                    })
                    .collect();
                line(&ListAnswer {
                    downloads,
                    speed: speed as u64,
                    queues,
                    categories,
                    free: self.free_space(),
                })
            }
            other => line(&ErrorAnswer {
                error: format!("unknown query {other:?}"),
                code: None,
            }),
        }
    }

    /// `{"pair": true}` (`misterdownloadmanager-cli pair`, #157): a new pairing code, the
    /// page's addresses and a QR code for a terminal; an error while the remote control is off
    /// or not listening.
    pub fn pair_answer(&mut self) -> String {
        use ipc::api::{ErrorAnswer, PairAnswer, line};
        let mut urls = self.remote.as_ref().map(|r| r.urls()).unwrap_or_default();
        // A container knows only its own address: the one people open, when given.
        if let Some(public) = crate::remote::public_url()
            && !urls.is_empty()
        {
            urls.insert(0, public);
        }
        let (Some(remote), Some(first)) = (&self.remote, urls.first()) else {
            return line(&ErrorAnswer {
                error: "the remote control isn't listening (Options → Remote)".into(),
                code: Some("remote-off".into()),
            });
        };
        let code = remote.start_pairing();
        line(&PairAnswer {
            qr: crate::remote::qr_text(&format!("{first}#pair={code}")),
            code,
            minutes: (mdm_remote::pairing::VALID.as_secs() / 60) as u32,
            urls,
        })
    }

    /// Free space where downloads go (General's folder), looked up off this thread at most every
    /// 10 s (a network share can be slow to answer).
    fn free_space(&mut self) -> Option<u64> {
        let (checked, free) = *self.free_space.lock().unwrap();
        if checked.is_none_or(|t| t.elapsed() >= Duration::from_secs(10)) {
            self.free_space.lock().unwrap().0 = Some(Instant::now());
            let dir = self
                .settings
                .category_dir(Category::General, &self.downloads);
            let slot = self.free_space.clone();
            self.rt.spawn_blocking(move || {
                // The folder may not exist yet: its nearest existing parent's volume.
                let at = dir
                    .ancestors()
                    .find(|d| d.is_dir())
                    .map(std::path::Path::to_owned);
                let free = at
                    .and_then(|d| mdm_engine::disk::volume_of(&d))
                    .map(|v| v.free);
                slot.lock().unwrap().1 = free;
            });
        }
        free
    }

    /// A download a paired device sent (#141): an address added as a silent command line's is,
    /// or a magnet link or .torrent file as Add torrent's Download Now with every file, in the
    /// category and queue it chose (by file type, and started now, when it chose none). No
    /// window opens. Answered with its id, or `pending` while a torrent's files are looked up.
    fn remote_add(&mut self, mut cmd: ipc::Command) -> String {
        use ipc::api::{AddAnswer, ErrorAnswer, line};
        let fail = |code: &str, error: &str| {
            line(&ErrorAnswer {
                error: error.into(),
                code: Some(code.into()),
            })
        };
        let category = match cmd.category.as_deref() {
            None => None,
            Some(key) => match Category::all().into_iter().find(|c| c.key() == key) {
                Some(c) => Some(c),
                None => return fail("unknown-category", "no such category"),
            },
        };
        let queue = match cmd.queue.as_deref() {
            None => None,
            Some(name) => match self.queues.iter().position(|q| q.name == name) {
                Some(q) => Some(q),
                None => return fail("unknown-queue", "no such queue"),
            },
        };
        // Later: in the queue chosen, else the main queue; else started now.
        let queue = cmd.later.then(|| queue.unwrap_or(0));
        let text = cmd.url.clone().unwrap_or_default();
        let torrent = match cmd.torrent_file.take() {
            Some(bytes) => Some((mdm_torrent::Source::File(bytes), false)),
            None => mdm_torrent::Source::magnet(text.trim()).map(|m| (m, true)),
        };
        if let Some((source, from_magnet)) = torrent {
            let slot = self.look_up_torrent(source);
            self.remote_torrents.push(crate::remote::Lookup {
                slot,
                category,
                queue,
                from_magnet,
                dir: None,
                group: None,
                tell: false,
            });
            return line(&AddAnswer {
                id: None,
                pending: true,
            });
        }
        let Some(url) = url::Url::parse(text.trim())
            .ok()
            .filter(mdm_engine::supported)
        else {
            return fail("unsupported", "not an address MDM downloads");
        };
        // Already in the list: no one at the computer to ask, so Options' choice, with "ask"
        // taken as "add it again" (under a numbered name).
        if let Some(dup) = self.items.iter().find(|i| i.url == url).map(|i| i.id) {
            match self.settings.duplicate_action {
                2 => self.overwrite(dup),
                3 => {
                    return line(&AddAnswer {
                        id: Some(dup),
                        pending: false,
                    });
                }
                _ => {}
            }
        }
        let name = mdm_engine::naming::sanitize(&mdm_engine::naming::file_name(None, &url));
        let category = category.unwrap_or_else(|| Category::of(&name));
        let dir = self.settings.category_dir(category, &self.downloads);
        let dest = mdm_engine::naming::destination(&dir, &name);
        cmd.remote = false;
        let id = self.push_item(
            url,
            dest,
            category,
            String::new(),
            None,
            None,
            cmd,
            // i18n: where a download came from (its Properties): a phone or computer paired in Options → Remote
            crate::i18n::mark("from a paired device"),
        );
        self.name_guessed(id, true);
        match queue {
            Some(q) => self.queues[q].items.push(id),
            None => self.start(id, false),
        }
        self.dirty = true;
        line(&AddAnswer {
            id: Some(id),
            pending: false,
        })
    }

    /// A torrent's files looked up on the runtime (a magnet link asks the swarm); the answer
    /// comes in the slot.
    fn look_up_torrent(&self, source: mdm_torrent::Source) -> crate::remote::Slot {
        let slot: crate::remote::Slot = Default::default();
        let (out, host, known) = (
            slot.clone(),
            self.torrent_host.clone(),
            self.session_hashes(),
        );
        self.rt.spawn(async move {
            let res = match host.session(known).await {
                Ok(t) => t
                    .metadata(&source, crate::torrentdlg::METADATA_WAIT, None)
                    .await
                    .map_err(|e| crate::errors::torrent(&e)),
                Err(e) => Err(e),
            };
            *out.lock().unwrap() = Some(res);
        });
        slot
    }

    /// The torrents a paired device sent, or Link grabber's magnet links (#709), whose files
    /// are known now: added with every file (into the category's folder, or the package's), as
    /// an RSS rule adds one; never over files already there.
    fn poll_remote_torrents(&mut self) {
        let mut ready = Vec::new();
        for l in std::mem::take(&mut self.remote_torrents) {
            let res = l.slot.lock().unwrap().take();
            match res {
                Some(res) => ready.push((res, l)),
                None => self.remote_torrents.push(l),
            }
        }
        for (res, l) in ready {
            let crate::remote::Lookup {
                category,
                queue,
                from_magnet,
                dir,
                group,
                tell,
                ..
            } = l;
            let meta = match res {
                Ok(meta) => meta,
                Err(e) => {
                    tracing::warn!("remote control: a torrent could not be read: {e}");
                    if tell {
                        self.notice = Some((
                            crate::i18n::mark("Link grabber"),
                            crate::i18n::fmt(
                                // i18n: Link grabber added a magnet link whose torrent could not be found; {error} says why
                                crate::i18n::t(
                                    "A magnet link's files could not be looked up: {error}",
                                ),
                                &[("error", &e)],
                            ),
                        ));
                    }
                    continue;
                }
            };
            if self.torrent_hashes().contains(&meta.info_hash) {
                tracing::info!("{}: this torrent is in the list already", meta.name);
                continue;
            }
            let category = category.unwrap_or_else(|| crate::torrentdlg::category_of(&meta));
            let dir = dir.unwrap_or_else(|| {
                crate::model::category_folder(
                    category,
                    &self.downloads,
                    &self.settings.category_dirs,
                )
            });
            if !meta.existing_in(&dir).is_empty() {
                tracing::warn!(
                    "remote control: {}: its files are already in {}",
                    meta.name,
                    dir.display()
                );
                continue;
            }
            let id = self.add_torrent(crate::torrentdlg::TorrentAdd {
                meta,
                dir,
                files: None,
                category,
                start_now: queue.is_none(),
                from_magnet,
                in_order: false,
                queue,
                quiet: true,
            });
            if let (Some(g), Some(id)) = (group, id)
                && let Some(i) = self.item(id)
            {
                i.group = Some(g);
            }
        }
    }

    /// The automation API's actions on downloads by id: pause one running, start one stopped,
    /// queued or failed (or a finished gallery), remove one (its file kept) or delete it with its file; one that runs
    /// is paused first by the caller.
    fn act(
        &mut self,
        action: &str,
        ids: &[u64],
    ) -> Result<ipc::api::ActAnswer, ipc::api::ErrorAnswer> {
        if !matches!(action, "pause" | "start" | "remove" | "delete") {
            return Err(ipc::api::ErrorAnswer {
                error: format!("unknown action {action:?}"),
                code: None,
            });
        }
        let mut answer = ipc::api::ActAnswer::default();
        for &id in ids {
            let Some(item) = self.item(id) else {
                answer.skipped.push(ipc::api::Skipped {
                    id,
                    why: "no such download".into(),
                });
                continue;
            };
            let running = item.live.is_some();
            // As Resume: a finished gallery may start again (#58).
            let startable = running || item.can_start();
            let why = match action {
                "pause" if !running => Some("not running"),
                "pause" => {
                    self.stop_by_hand(id);
                    None
                }
                "start" if running => Some("already running"),
                "start" if !startable => Some("complete"),
                "start" => {
                    self.start(id, false);
                    None
                }
                _ if running => Some("running: pause it first"),
                _ => {
                    self.remove_download(id, action == "delete");
                    None
                }
            };
            match why {
                Some(why) => answer.skipped.push(ipc::api::Skipped {
                    id,
                    why: why.into(),
                }),
                None => answer.done.push(id),
            }
        }
        Ok(answer)
    }

    /// Options → OK: keep the new settings and act on what changed.
    fn apply_options(&mut self, new: Settings) {
        new.publish_categories();
        // Portable mode writes nothing outside its folder (the option is off there).
        if new.launch_on_startup != self.settings.launch_on_startup
            && mdm_ipc::portable_dir().is_none()
        {
            #[cfg(windows)]
            crate::win32::set_run_at_startup(new.launch_on_startup);
            #[cfg(unix)]
            crate::unix::set_run_at_startup(new.launch_on_startup);
        }
        self.settings = new;
        crate::regional::set(&self.settings.regional);
        crate::webmark::set(self.settings.mark_downloads);
        // Options → Remote: listening started, stopped or moved.
        if let Some(r) = &self.remote {
            r.apply(crate::remote::config(&self.settings));
        }
        // Options → Archives and Link grabber: Click'n'Load turned on or off; off forgets the
        // sites' answers, so a blocked site can be asked again.
        if !self.settings.cnl_on {
            self.settings.cnl_sites.clear();
        }
        if let Err(why) = self.cnl.listen(self.settings.cnl_on && !self.headless) {
            // i18n: title of the notice when Click'n'Load couldn't start
            self.notice = Some((crate::i18n::mark("Click'n'Load"), why));
        }
        if self.settings.proxy_mode == 2 {
            crate::pacfetch::refresh(&self.settings.proxy_script, &self.rt);
        }
        self.apply_speed_limit();
        self.dirty = true;
    }

    /// Options → Sounds: play the sound for event `i` (complete, failed, queue started,
    /// queue stopped/finished) when it is on.
    fn sound(&self, i: usize) {
        if let Some((true, file)) = self.settings.sounds.get(i)
            && !file.is_empty()
        {
            crate::options::play_sound(file);
        }
    }

    /// Apply what the Scheduler asks for.
    fn scheduler_action(&mut self, a: crate::scheduler::Action) {
        use crate::scheduler::Action;
        let index = |queues: &[Queue], name: &str| queues.iter().position(|q| q.name == name);
        match a {
            Action::Apply { queues, limits } => {
                // A running queue the Scheduler deleted stops first (its downloads stay).
                for q in 0..self.queues.len() {
                    if self.queues[q].running
                        && !queues.iter().any(|n| n.name == self.queues[q].name)
                    {
                        self.stop_queue(q);
                    }
                }
                // Running state belongs to the app, not the draft.
                let running: Vec<String> = self
                    .queues
                    .iter()
                    .filter(|q| q.running)
                    .map(|q| q.name.clone())
                    .collect();
                // The list showing a queue follows it to its new place (All Downloads when
                // it is gone).
                if let View::Queue(i) = self.view {
                    let name = self.queues.get(i).map(|q| q.name.clone());
                    self.view = name
                        .and_then(|n| queues.iter().position(|q| q.name == n))
                        .map_or(View::All, View::Queue);
                }
                self.queues = queues;
                for q in &mut self.queues {
                    q.running = running.contains(&q.name);
                }
                self.settings.limits_on = limits.on;
                self.settings.limits_mb = limits.mb;
                self.settings.limits_hours = limits.hours;
                self.settings.limits_warn = limits.warn;
                self.dirty = true;
            }
            Action::Start(name) => {
                if let Some(q) = index(&self.queues, &name) {
                    self.start_queue(q);
                }
            }
            Action::Stop(name) => {
                if let Some(q) = index(&self.queues, &name) {
                    self.stop_queue(q);
                }
            }
        }
    }

    fn start_queue(&mut self, q: usize) {
        if !self.queues[q].running {
            self.sound(2);
        }
        self.queues[q].running = true;
        for id in &self.queues[q].items {
            self.stopped_by_hand.remove(id);
        }
        self.tick_queues();
    }

    fn stop_queue(&mut self, q: usize) {
        self.queues[q].running = false;
        for id in self.queues[q].items.clone() {
            self.stop(id);
        }
    }

    /// How a queue sees one of its downloads.
    /// Every download's standing for the queues, by id.
    fn members(&self, now: Instant) -> std::collections::HashMap<u64, Member> {
        self.items
            .iter()
            .map(|i| (i.id, self.member_of(i, now)))
            .collect()
    }

    fn member_of(&self, i: &Item, now: Instant) -> Member {
        let id = i.id;
        match i.status {
            Status::Stopped if self.stopped_by_hand.contains(&id) => Member::Held,
            Status::Queued | Status::Stopped => Member::Waiting,
            Status::Downloading => Member::Running,
            Status::Complete => Member::Complete,
            Status::Error => {
                let long_ago = now.checked_sub(queue::RETRY_DELAY).unwrap_or(now);
                let (at, tries) = self.failures.get(&id).copied().unwrap_or((long_ago, 0));
                Member::Failed { at, tries }
            }
        }
    }

    /// Scheduler times, then start what running queues allow (notes/ENGINE.md §5).
    fn tick_queues(&mut self) {
        use chrono::{Datelike, Timelike};
        // The local time with the offset the clock's thread keeps: asking Windows here held the
        // window for 8 s (#536). Until its first answer, schedules wait.
        let local = crate::clock::now_known();
        let now_s = local.map(|t| t.num_seconds_from_midnight());
        let prev = std::mem::replace(&mut self.clock, now_s).or(now_s);
        if let (Some(local), Some(now_s), Some(prev)) = (local, now_s, prev) {
            let today = local.date_naive();
            let weekday = today.weekday().num_days_from_sunday() as usize;
            let date = (today.year(), today.month(), today.day());
            for q in 0..self.queues.len() {
                let sch = self.queues[q].schedule.clone();
                if let Some(t) = sch.start_at
                    && sch.starts_on(date, weekday)
                    && queue::crossed(t, prev, now_s)
                {
                    self.queues[q].running = true;
                }
                if let Some(t) = sch.stop_at
                    && queue::crossed(t, prev, now_s)
                {
                    self.stop_queue(q);
                }
            }
        }
        let now = Instant::now();
        for q in 0..self.queues.len() {
            if !self.queues[q].running {
                continue;
            }
            // Each download's standing, looked up once (a linear search per queued id before).
            let members = self.members(now);
            let starts = queue::to_start(&self.queues[q], |id| member_in(&members, id), now);
            for id in starts {
                self.start(id, false);
            }
            let members = self.members(now);
            if queue::finished(&self.queues[q], |id| member_in(&members, id)) {
                self.queues[q].running = false;
                self.queue_done(q);
            }
        }
        // The next wake follows the schedules (set again when they change), and a queue that
        // wakes the computer keeps it awake while it runs (#142).
        self.wake.sync(&self.queues);
        let waking = self
            .queues
            .iter()
            .find(|q| q.running && q.schedule.wake)
            .map(|q| q.name.clone());
        self.wake
            .keep_awake(waking.is_some(), waking.as_deref().unwrap_or_default());
    }

    /// Scheduler → Download limits: stop at the limit, go on when the window has room.
    fn check_limits(&mut self, now: Instant) {
        self.quota
            .record(now, self.items.iter().map(|i| (i.id, i.downloaded)));
        let s = &self.settings;
        let limit = u64::from(s.limits_mb.max(1)) << 20;
        let window = Duration::from_secs(u64::from(s.limits_hours.max(1)) * 3600);
        let change = if s.limits_on {
            self.quota.check(now, limit, window)
        } else if self.quota.blocked {
            self.quota.blocked = false;
            Some(false)
        } else {
            None
        };
        match change {
            Some(true) => {
                let running: Vec<u64> = self
                    .items
                    .iter()
                    .filter(|i| i.live.is_some())
                    .map(|i| i.id)
                    .collect();
                for id in running {
                    self.stop(id);
                    self.quota.held.push(id);
                }
                for id in self.quota.held.clone() {
                    if let Some(item) = self.item(id) {
                        item.status = Status::Queued;
                    }
                }
                if self.settings.limits_warn {
                    let wait = self.quota.room_in(now, limit, window);
                    self.limit_notice = Some(dialogs::LimitNotice::new(
                        self.settings.limits_mb,
                        self.settings.limits_hours,
                        wait,
                    ));
                }
                self.dirty = true;
            }
            Some(false) => {
                for id in std::mem::take(&mut self.quota.held) {
                    if let Some(item) = self.item(id) {
                        item.status = Status::Stopped;
                    }
                    self.start(id, false);
                }
                self.dirty = true;
            }
            None => {}
        }
    }

    /// A queue ran out of work: its "when done" actions (Scheduler).
    fn queue_done(&mut self, q: usize) {
        self.sound(3);
        let sch = self.queues[q].schedule.clone();
        if let Some(path) = &sch.open_when_done {
            crate::launch::open(path);
        }
        self.when_done(sch.when_done());
    }

    /// A queue's, or a download's (its progress window's Options on completion), "when done":
    /// hang up, turn the computer off (a countdown first), or exit.
    fn when_done(&mut self, w: crate::queue::WhenDone) {
        // Ends dial-up (RAS) connections; nothing happens without one. Windows only: macOS and
        // Linux have no dial-up command to call.
        #[cfg(windows)]
        if w.hang_up {
            crate::launch::spawn("rasdial", move || {
                let _ = std::process::Command::new("rasdial")
                    .arg("/disconnect")
                    .spawn();
            });
        }
        if let Some(mode) = w.turn_off {
            self.turn_off = Some(dialogs::TurnOffCountdown::new(mode, w.force));
        } else if w.exit {
            self.exit_requested = true;
        }
    }

    /// Files dragged out of Download complete that the drop target moved (#869): the download
    /// follows its file where the shell said it went, or remembers it went where MDM couldn't
    /// learn. Only while the download still points where the drag took the file from.
    fn poll_dragged_out(&mut self) {
        for (id, from, out) in dialogs::dragged_out() {
            let Some(item) = self.item(id).filter(|i| i.dest == from) else {
                continue;
            };
            match &out {
                dialogs::DraggedOut::Moved(to) => {
                    tracing::info!("drag out: download {id} moved with its file");
                    take_place(item, to.clone());
                    let folder = to.parent().unwrap_or(to).display().to_string();
                    item.note(crate::i18n::fmt(
                        crate::i18n::t("Dragged out of Download complete and moved to {folder}"),
                        &[("folder", &folder)],
                    ));
                }
                dialogs::DraggedOut::MovedAway => {
                    tracing::info!("drag out: download {id}'s file moved where MDM can't follow");
                    item.moved_away = true;
                    item.note(crate::i18n::t(
                        "Dragged out of Download complete and moved to a place MDM couldn't learn",
                    ));
                }
            }
            for c in &mut self.complete {
                if c.id == id {
                    match &out {
                        dialogs::DraggedOut::Moved(to) => {
                            c.path = to.clone();
                            c.moved_away = false;
                        }
                        dialogs::DraggedOut::MovedAway => c.moved_away = true,
                    }
                }
            }
            self.dirty = true;
        }
    }

    fn open_item(&mut self, id: u64) {
        let Some(i) = self.items.iter().find(|i| i.id == id) else {
            return;
        };
        if i.status == Status::Complete {
            crate::dialogs::open_download(&i.dest, i.moved_away);
        } else if !self.progress.iter().any(|p| p.id == id) {
            self.progress
                .push(Progress::new(id, self.settings.show_complete));
        } else if let Some(p) = self.progress.iter_mut().find(|p| p.id == id && p.hidden) {
            // Hidden in the tray (#63): back, as it was.
            p.hidden = false;
        } else {
            // Open already, behind: to the front.
            dialogs::raise(
                &self.ui_ctx,
                egui::ViewportId::from_hash_of(("progress", id)),
            );
        }
    }

    /// `target`: the right-clicked download, if any. Stop, Resume, Remove and the queue
    /// commands act on every selected download (or the right-clicked one outside the
    /// selection); the others on one download.
    fn run(&mut self, cmd: Cmd, target: Option<u64>, ctx: &egui::Context) {
        // The log a crash report quotes: what was asked for (commands carry no addresses).
        tracing::info!("command {cmd:?}");
        let many = self.targets(target);
        let target = target.or(self.selection.one());
        let dest = target
            .and_then(|id| self.items.iter().find(|i| i.id == id))
            .map(|i| i.dest.clone());
        // Its file dragged out of Download complete to a place MDM couldn't learn (#869).
        let moved_away = target
            .and_then(|id| self.items.iter().find(|i| i.id == id))
            .is_some_and(|i| i.moved_away);
        match cmd {
            Cmd::AddUrl => self.add_url = Some(AddUrl::from_clipboard(&self.settings.url_history)),
            Cmd::Exit => {
                self.exiting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Cmd::Stop => {
                for id in many {
                    if self.item(id).is_some_and(|i| i.live.is_some()) {
                        self.stop_by_hand(id);
                    }
                }
            }
            Cmd::Remove => self.ask_delete(many, false),
            Cmd::RemoveWithFiles => self.ask_delete(many, true),
            Cmd::Resume => {
                for id in many {
                    if self.item(id).is_some_and(|i| i.can_start()) {
                        self.start(id, true);
                    }
                }
            }
            Cmd::StopAll => self.stop_all(),
            Cmd::DeleteCompleted => self.delete_completed(),
            Cmd::StartQueue(q) => self.start_queue(q),
            Cmd::StopQueue(q) => self.stop_queue(q),
            Cmd::Browse(c) => {
                let dir = self.settings.category_dir(c, &self.downloads);
                let _ = std::fs::create_dir_all(&dir);
                crate::launch::open(dir);
            }
            Cmd::DoubleClickOpen(open) => {
                self.settings.double_click_open = open;
                self.dirty = true;
            }
            Cmd::AddToQueue(q) => {
                // "Move to queue" for a download already in one: it leaves that queue.
                let done: Vec<u64> = many
                    .iter()
                    .copied()
                    .filter(|id| {
                        self.items
                            .iter()
                            .any(|i| i.id == *id && i.status == Status::Complete)
                    })
                    .collect();
                for id in many {
                    // Finished files go only to synchronization queues.
                    if done.contains(&id) && !self.queues[q].sync {
                        continue;
                    }
                    for (k, other) in self.queues.iter_mut().enumerate() {
                        if k != q {
                            other.items.retain(|x| *x != id);
                        }
                    }
                    if !self.queues[q].items.contains(&id) {
                        self.queues[q].items.push(id);
                    }
                }
                self.dirty = true;
            }
            Cmd::RemoveFromQueue => {
                for id in many {
                    for q in &mut self.queues {
                        q.items.retain(|x| *x != id);
                    }
                }
                self.dirty = true;
            }
            Cmd::ToggleCategories => self.show_categories = !self.show_categories,
            Cmd::ChooseLanguage => {
                if self.lang_picker.is_none() {
                    self.lang_picker = Some(crate::langpick::Picker::new(
                        false,
                        self.settings.language.as_deref(),
                    ));
                }
            }
            Cmd::Language(code) => {
                self.settings.language = code.map(str::to_owned);
                self.settings.language_asked = true;
                crate::i18n::set_lang(code);
                crate::theme::language_fonts(ctx);
                if let Some(tray) = &self.tray {
                    tray.relabel();
                }
                self.dirty = true;
                ctx.request_repaint();
            }
            Cmd::ToggleDarkMode => {
                self.settings.dark_mode_support = !self.settings.dark_mode_support;
                self.theme_checked = None;
                self.dirty = true;
            }
            Cmd::ToggleAlwaysDark => {
                self.settings.always_dark = !self.settings.always_dark;
                self.theme_checked = None;
                self.dirty = true;
            }
            Cmd::Theme(id) => {
                self.settings.theme = id.to_owned();
                crate::theme::select(id, ctx);
                // Live: egui's own visuals, the glyph rendering and the captions follow at once;
                // every painter reads the new theme from its next frame.
                crate::theme::apply_theme(ctx);
                crate::theme::apply_mode(ctx);
                self.caption_set = false;
                self.theme_checked = None;
                self.dirty = true;
                ctx.request_repaint();
            }
            Cmd::OpenThemesFolder => {
                if let Some(dir) = crate::themefile::dir() {
                    let _ = std::fs::create_dir_all(&dir);
                    crate::launch::open(dir);
                }
            }
            Cmd::AddThemeFile => {
                // i18n: title of the file dialog of View → Theme → Add a theme file
                let title = crate::i18n::t("Add a theme file").to_owned();
                // i18n: the kind of file View → Theme → Add a theme file lists: MDM's themes (TOML, JSON), Base16 schemes (YAML) and pywal's colors.json
                let kinds = crate::i18n::t("Themes and colour schemes").to_owned();
                let all = crate::i18n::t("All files").to_owned();
                // Where pywal writes its colours, when it has: the first place to look.
                let start = crate::themefile::pywal_file()
                    .filter(|p| p.is_file())
                    .and_then(|p| p.parent().map(std::path::Path::to_path_buf));
                winui::start_pick(&self.ui_ctx, "app-theme-file", move || {
                    let dialog = crate::winui::file_dialog(&title);
                    let dialog = match start {
                        Some(dir) => dialog.set_directory(dir),
                        None => dialog,
                    };
                    dialog
                        .add_filter(kinds, &["toml", "json", "yaml", "yml"])
                        .add_filter(all, &["*"])
                        .pick_file()
                });
            }
            // Every window at once (the frames after this one); saved by `follow_zoom`.
            Cmd::InterfaceSize(percent) => crate::scale::set(ctx, percent),
            #[cfg(windows)]
            Cmd::SmoothText(on) => {
                crate::theme::set_smooth_text(ctx, on);
                self.settings.smooth_text = on;
                self.dirty = true;
            }
            #[cfg(gpu)]
            Cmd::DrawWith(api) => {
                // Chosen again after it failed here: its next start is tried.
                if Some(api) == crate::renderer::GPU {
                    crate::renderer::forget_failure();
                } else {
                    // OpenGL chosen again after it couldn't draw MDM (#835, #847): tried and
                    // checked again.
                    crate::renderer::forget_gl_failure();
                }
                tracing::info!("draw with {api:?} from the next start");
                self.settings.renderer = Some(api);
                self.dirty = true;
            }
            Cmd::Open => {
                if let Some(id) = target {
                    self.open_item(id);
                }
            }
            Cmd::Properties => {
                if let Some(id) = target {
                    self.properties(id);
                }
            }
            Cmd::Extract => {
                // Every chosen archive (a split one once, from whichever volume).
                for id in self.targets(target) {
                    if let Some(i) = self.items.iter().find(|i| i.id == id) {
                        self.extracts.offer(i, &self.items, &self.settings, true);
                    }
                }
            }
            Cmd::VerifyChecksum => {
                if let Some(id) = target
                    && !self.checksums.iter().any(|c| c.id == id)
                    && let Some(i) = self.items.iter().find(|i| i.id == id)
                {
                    let file = (i.status == Status::Complete).then(|| i.dest.clone());
                    self.checksums.push(crate::checksum::ChecksumDialog::new(
                        id,
                        i.checksum.as_ref(),
                        file,
                    ));
                }
            }
            Cmd::RefreshAddress => {
                // As IDM: the page the link came from opens, so a fresh link can be taken
                // there; Properties takes the new address and the download resumes from it.
                if let Some(id) = target {
                    let page = self
                        .items
                        .iter()
                        .find(|i| i.id == id)
                        .and_then(|i| i.context.referer.clone());
                    if let Some(page) = page {
                        crate::launch::open(page);
                    }
                    self.properties(id);
                }
            }
            Cmd::OpenWith => {
                if let Some(d) = dest {
                    dialogs::open_download_with(&d, moved_away);
                }
            }
            Cmd::OpenFolder => {
                if let Some(d) = dest {
                    dialogs::show_download_in_folder(&d, moved_away);
                }
            }
            Cmd::ToggleColumn(i) => self.list.visible[i] = !self.list.visible[i],
            Cmd::LimiterOn | Cmd::LimiterOff => {
                self.settings.speed_limit_on = cmd == Cmd::LimiterOn;
                self.apply_speed_limit();
                self.dirty = true;
            }
            Cmd::LimiterSettings => {
                self.speed_settings = Some(dialogs::SpeedSettings::new(&self.settings));
            }
            Cmd::Find => {
                if self.find.is_none() {
                    self.find = Some(crate::find::Find::new(self.last_find.as_ref()));
                } else {
                    dialogs::raise(ctx, dialogs::dialog_viewport("find"));
                }
            }
            Cmd::FindNext => match self.last_find.clone() {
                Some(q) => self.find_next(&q),
                None => self.find = Some(crate::find::Find::new(None)),
            },
            Cmd::VideoPage => {
                if self.video_dlg.is_none() {
                    self.video_dlg = Some(self.video_dialog(None));
                }
            }
            Cmd::AddMagnet => self.open_torrent_dialog(None, ctx),
            Cmd::HelpPage(page) => crate::help::open_page(page),
            Cmd::Shortcuts => self.shortcuts = Some(crate::help::Shortcuts),
            Cmd::ReportProblem => crate::help::report_problem(),
            Cmd::CheckUpdates => {
                if self.updates.is_none() {
                    self.updates = Some(crate::help::Updates::check(&self.rt, ctx));
                }
            }
            Cmd::About => self.about = Some(crate::help::About),
            Cmd::Support => crate::help::support(),
            Cmd::TellFriend => {
                if self.tell_friend.is_none() {
                    self.tell_friend = Some(crate::help::TellFriend::new());
                }
            }
            Cmd::TorrentSettings => {
                if self.torrent_settings.is_none() {
                    self.torrent_settings =
                        Some(crate::torrentdlg::TorrentSettings::new(&self.settings));
                }
            }
            Cmd::RssFeeds(rules) => {
                if self.rss_dlg.is_none() {
                    self.rss_dlg = Some(crate::rssdlg::RssWindow::new(&self.rss, rules));
                }
            }
            Cmd::TorrentRecheck => {
                if let Some(id) = target {
                    self.torrent_act(id, crate::torrents::Act::Recheck);
                }
            }
            Cmd::TorrentReannounce => {
                if let Some(id) = target {
                    self.torrent_act(id, crate::torrents::Act::Reannounce);
                }
            }
            Cmd::TorrentInOrder | Cmd::TorrentEndsFirst => {
                if let Some(id) = target {
                    self.toggle_torrent_order(id, matches!(cmd, Cmd::TorrentInOrder));
                }
            }
            Cmd::TorrentAddTrackers => {
                let torrent = |i: &&Item| i.torrent.is_some();
                if let Some(i) = target
                    .and_then(|id| self.items.iter().find(|i| i.id == id))
                    .filter(torrent)
                {
                    self.add_trackers =
                        Some(crate::torrentdlg::AddTrackers::new(i.id, &i.file_name));
                }
            }
            Cmd::TorrentFiles => {
                if let Some(item) = target.and_then(|id| self.items.iter().find(|i| i.id == id)) {
                    self.torrent_files =
                        crate::torrentfiles::TorrentFiles::new(item, &self.torrent_host);
                }
            }
            Cmd::TorrentCopyMagnet => {
                // With its trackers, from the .torrent kept for it; else the list's address.
                let found = target.and_then(|id| self.items.iter().find(|i| i.id == id));
                if let Some((spec, url)) = found.and_then(|i| i.torrent.as_ref().zip(Some(&i.url)))
                {
                    let link =
                        crate::torrents::magnet_of(&spec.hash).unwrap_or_else(|| url.to_string());
                    crate::copy_text(ctx, link);
                }
            }
            Cmd::OpenTorrentFile => {
                let kind = crate::i18n::t("Torrent files").to_owned();
                let title = crate::i18n::t("Open torrent file").to_owned();
                winui::start_pick(ctx, "app-torrent-file", move || {
                    crate::winui::file_dialog(&title)
                        .add_filter(kind, &["torrent"])
                        .pick_file()
                });
            }
            Cmd::Batch => {
                if self.batch.is_none() {
                    self.batch = Some(crate::batch::Batch::new());
                }
            }
            // Read now, off the window's thread; what it holds opens a window when it comes
            // (`batch_from_clipboard`).
            Cmd::BatchClipboard => self.batch_clip = Some(crate::clipboard::Prefill::ask()),
            Cmd::Options => {
                if self.options.is_none() {
                    self.options = Some(
                        crate::options::OptionsDialog::new(
                            &self.settings,
                            &self.downloads,
                            self.host_limits.learned(),
                        )
                        .with_remote(self.remote.clone()),
                    );
                } else {
                    dialogs::raise(ctx, dialogs::dialog_viewport("options"));
                }
            }
            Cmd::Scheduler => {
                if self.scheduler.is_none() {
                    self.scheduler = Some(crate::scheduler::Scheduler::new(
                        &self.queues,
                        &self.settings,
                    ));
                } else {
                    dialogs::raise(ctx, dialogs::dialog_viewport("scheduler"));
                }
            }
            Cmd::EditQueue(_) | Cmd::ScheduleQueue(_) | Cmd::NewQueue => {
                let s = self.scheduler.get_or_insert_with(|| {
                    crate::scheduler::Scheduler::new(&self.queues, &self.settings)
                });
                match cmd {
                    // Edit queue: its files; Schedule: when it runs.
                    Cmd::EditQueue(q) => s.show_queue(q, true),
                    Cmd::ScheduleQueue(q) => s.show_queue(q, false),
                    _ => s.new_queue(),
                }
            }
            // A dialog already open (half filled in) stays.
            Cmd::AddCategory => {
                if self.category_dlg.is_none() {
                    self.category_dlg = Some(crate::categories::CategoryDialog::new());
                }
            }
            Cmd::CategoryProps(c) => {
                let folder = self.settings.category_dir(c, &self.downloads);
                self.category_dlg = Some(crate::categories::CategoryDialog::edit(c, &folder));
            }
            // The user's categories and the built-in ones by file type (#93); not General nor
            // Torrents.
            Cmd::DeleteCategory(c) if c.editable() => {
                self.confirm_category_delete = Some(dialogs::DeleteCategory::new(c));
            }
            Cmd::DeleteCategory(_) => {}
            Cmd::ToggleDropTarget => {
                self.settings.drop_target = !self.settings.drop_target;
                self.dirty = true;
            }
            Cmd::ShowMain => self.show_window(ctx),
            Cmd::Export { ef2 } => self.export(ef2),
            Cmd::Import { ef2 } => self.import(ef2),
            Cmd::LinkGrabber => {
                self.show_import(Vec::new(), String::new());
                dialogs::raise(ctx, dialogs::dialog_viewport("grabber"));
            }
            Cmd::Redownload => {
                if let Some(id) = target
                    && self.can_redownload(id)
                    && let Some(item) = self.items.iter().find(|i| i.id == id)
                {
                    let text = crate::i18n::fmt(
                        crate::i18n::t(
                            "Download \"{name}\" again from the start? A finished file is replaced when the new download completes.",
                        ),
                        &[("name", &item.file_name)],
                    );
                    // i18n: window title, verb: download the file again from the start
                    let title = crate::i18n::mark("Redownload");
                    self.confirm_redownload = Some((id, dialogs::Confirm::new(title, text)));
                }
            }
            Cmd::MoveRename => {
                if many.len() > 1 {
                    self.move_many(many);
                } else if let Some(id) = target {
                    self.move_rename(id);
                }
            }
            Cmd::DeleteQueue(q) => {
                if q >= 2 && q < self.queues.len() {
                    let name = self.queues[q].name.clone();
                    self.confirm_queue_delete = Some((
                        q,
                        dialogs::Confirm::new(
                            crate::i18n::mark("Delete queue"),
                            crate::i18n::fmt(
                                crate::i18n::t(
                                    "Delete the queue \"{queue}\"? Its downloads stay in the list.",
                                ),
                                &[("queue", &name)],
                            ),
                        ),
                    ));
                }
            }
            Cmd::Arrange(col) => self.list.sort = col.map(|c| (c, true)),
            Cmd::ClearFilters => self.list.filters = vec![None; list::COLS],
            Cmd::ResetColumns => self.list.reset_columns(),
            Cmd::ShowAll => self.view = View::All,
            Cmd::SelectAll => {
                let ids = self.shown_ids();
                self.selection.apply(crate::selection::Pick::All, &ids);
            }
            Cmd::None => {}
        }
    }

    /// Pull engine progress into the rows; open "Download complete" when one finishes.
    fn poll(&mut self, ctx: &egui::Context) {
        self.poll_moves();
        self.poll_dragged_out();
        self.poll_deferred_torrent_renames();
        if !self.moving.is_empty()
            || !self.renaming_torrents.is_empty()
            || !self.deferred_torrent_queue.is_empty()
        {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        let mut finished = Vec::new();
        let mut failed = Vec::new();
        let mut any_live = false;
        for i in &mut self.items {
            if i.poll() {
                self.dirty = true;
                match i.status {
                    Status::Complete => {
                        // Downloaded again: its file is where MDM saved it.
                        i.moved_away = false;
                        finished.push(i.id);
                    }
                    Status::Error => failed.push(i.id),
                    _ => {}
                }
            }
            any_live |= i.live.is_some();
        }
        let now = Instant::now();
        self.check_limits(now);
        if !finished.is_empty() {
            self.sound(0);
        }
        if !failed.is_empty() {
            self.sound(1);
        }
        // Downloads added together (#766): their group is looked at until all of it is done, and
        // its summary speaks for the finished ones (no Download complete each).
        for (id, ok) in finished
            .iter()
            .map(|id| (*id, true))
            .chain(failed.iter().map(|id| (*id, false)))
        {
            if let Some(g) = self.items.iter().find(|i| i.id == id).and_then(|i| i.group) {
                self.groups_open.insert(g);
                if ok {
                    self.group_finished.insert(id);
                }
            }
        }
        for id in failed {
            let e = self.failures.entry(id).or_insert((now, 0));
            *e = (now, e.1 + 1);
            for q in &mut self.queues {
                queue::requeue_failed(q, id);
            }
        }
        for id in &finished {
            self.release_link(*id);
            self.failures.remove(id);
            let torrent_renaming = self.apply_deferred_rename(*id);
            if !torrent_renaming {
                self.flush_pending_mark(*id);
                // Its archive, when it is to be extracted and its last volume is in (#37).
                self.extracts.finished(*id, &self.items, &self.settings);
            }
            // Finished files leave download queues (synchronization queues keep them).
            for q in self.queues.iter_mut().filter(|q| !q.sync) {
                q.items.retain(|x| x != id);
            }
        }
        if self.extracts.poll(&mut self.items, ctx) {
            self.dirty = true;
        }
        self.tick_queues();
        // A download started early behind Download File Info stays quiet until the dialog's
        // answer (Start shows its box then; Cancel drops it).
        let early = self.file_info.as_ref().and_then(|f| f.early);
        let close = finished_windows(
            finished,
            |id| self.progress.iter().any(|p| p.id == id && !p.hidden),
            |id| {
                self.items
                    .iter()
                    .any(|i| i.id == id && i.status == Status::Complete)
            },
            &mut self.finishing,
            now,
        );
        for id in close {
            self.close_finished(id, early);
        }
        self.settle_groups();
        // A snooze over (#741): Download complete shows again.
        if self.snooze.expire(unix_now()) {
            tracing::info!("download complete: the snooze is over");
            self.snoozed_finished = 0;
            self.settings.complete_snoozed_until = None;
            self.dirty = true;
        }
        if let Some(left) = self.snooze.left(unix_now()) {
            ctx.request_repaint_after(Duration::from_secs(left.unsigned_abs().min(60)));
        }
        // The next one's moment ends without a frame of its own otherwise (nothing runs).
        if let Some(&(_, at)) = self.finishing.iter().min_by_key(|(_, at)| *at) {
            ctx.request_repaint_after(progress::SHOWN_WHOLE.saturating_sub(now.duration_since(at)));
        }
        if any_live {
            ctx.request_repaint_after(Duration::from_millis(250));
            // The list's progress figures, every 30 s while something runs: after a crash the
            // rows show about where each download was (torrents keep their own resume data,
            // files their state file; this is only what the list shows).
            if self.last_save.elapsed() > Duration::from_secs(30) {
                self.dirty = true;
            }
        }
        // The keychain took a secret (#44): saved again, with its reference in its place.
        let vault = crate::secrets::vault();
        if vault.pending() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        let confirmed = vault.confirmed();
        if confirmed != self.secrets_confirmed {
            self.secrets_confirmed = confirmed;
            self.dirty = true;
        }
        if self.dirty && self.last_save.elapsed() > Duration::from_secs(1) {
            // Written on the saver's thread (store::Saver); refused at once when the list
            // couldn't be read at start.
            if let Err(e) = self.saver.save(
                self.next_id,
                &self.items,
                &self.list.layout(),
                &self.settings,
                &self.queues,
            ) {
                self.report_save(Err(e.to_string()));
            }
            self.dirty = false;
            self.last_save = Instant::now();
        }
        if let Some(outcome) = self.saver.outcome() {
            self.report_save(outcome);
        }
    }

    /// A finished download's progress window closes, and Download complete opens for it (`early`:
    /// the download Download File Info started early, which stays quiet).
    fn close_finished(&mut self, id: u64, early: Option<u64>) {
        // This file's "Show download complete dialog" (its progress window's Options on
        // completion tab), read before the window closes.
        let window = self.progress.iter().find(|p| p.id == id);
        let this_file = window.is_none_or(|p| p.shows_complete());
        if let Some(w) = window.and_then(Progress::when_done) {
            self.when_done(w);
        }
        self.progress.retain(|p| p.id != id);
        if Some(id) == early {
            return;
        }
        // One of a group: its summary speaks for it (#766).
        if self.group_finished.remove(&id) {
            return;
        }
        if self.settings.show_complete && this_file {
            self.complete_window(id);
        }
    }

    /// Download complete for `id`, unless it is snoozed (#741): then the tray icon's tip counts
    /// it instead.
    fn complete_window(&mut self, id: u64) {
        if self.snooze.active(unix_now()) {
            self.snoozed_finished += 1;
            return;
        }
        if let Some(i) = self.items.iter().find(|i| i.id == id) {
            self.complete.push(Complete::of(i));
        }
    }

    /// Where a group's download stands (#766): running, or waiting in a running queue for its
    /// turn (or for the download limits' window), the group isn't done; stopped, or waiting in
    /// a queue that doesn't run, it is left out of the summary.
    fn group_standing(
        &self,
        i: &Item,
        running: &std::collections::HashSet<u64>,
    ) -> crate::finish::Standing {
        use crate::finish::Standing;
        if i.live.is_some() || i.status == Status::Downloading || self.quota.held.contains(&i.id) {
            return Standing::Pending;
        }
        match i.status {
            Status::Complete => Standing::Finished,
            Status::Error => Standing::Failed,
            Status::Queued | Status::Stopped
                if running.contains(&i.id) && !self.stopped_by_hand.contains(&i.id) =>
            {
                Standing::Pending
            }
            _ => Standing::Idle,
        }
    }

    /// Groups whose downloads are all done (#766): one summary each, or Download complete when
    /// only one of them finished; nothing while Download complete is snoozed or turned off. The
    /// downloads summed up leave their group.
    fn settle_groups(&mut self) {
        if self.groups_open.is_empty() {
            return;
        }
        let running: std::collections::HashSet<u64> = self
            .queues
            .iter()
            .filter(|q| q.running)
            .flat_map(|q| q.items.iter().copied())
            .collect();
        let members: Vec<(u64, u64, crate::finish::Standing)> = self
            .items
            .iter()
            .filter_map(|i| {
                let g = i.group.filter(|g| self.groups_open.contains(g))?;
                Some((i.id, g, self.group_standing(i, &running)))
            })
            .collect();
        for done in crate::finish::settle(&mut self.groups_open, members) {
            for id in done.finished.iter().chain(&done.failed) {
                if let Some(i) = self.item(*id) {
                    i.group = None;
                }
            }
            self.dirty = true;
            tracing::info!(
                "group {} done: {} finished, {} failed",
                done.group,
                done.finished.len(),
                done.failed.len()
            );
            if self.snooze.active(unix_now()) {
                self.snoozed_finished += done.finished.len() as u64;
                continue;
            }
            if !self.settings.show_complete {
                continue;
            }
            match crate::finish::shown(done) {
                crate::finish::Shown::One(id) => self.complete_window(id),
                crate::finish::Shown::Summary(s) => {
                    self.summaries.push(dialogs::GroupDone::new(
                        s.group,
                        &s.finished,
                        &s.failed,
                        &self.items,
                    ));
                }
            }
        }
    }

    /// Download complete snoozed for a while (#741): its setting stays as it is.
    fn start_snooze(&mut self, choice: crate::finish::SnoozeFor) {
        self.snooze.start(choice, unix_now());
        self.settings.complete_snoozed_until = self.snooze.saved();
        self.snoozed_finished = 0;
        self.dirty = true;
        tracing::info!("download complete snoozed: {choice:?}");
    }

    /// Options, Downloads, Resume now: Download complete shows again.
    fn resume_snooze(&mut self) {
        self.snooze.resume();
        self.settings.complete_snoozed_until = None;
        self.snoozed_finished = 0;
        self.dirty = true;
        tracing::info!("download complete: the snooze ended by hand");
    }

    /// A group's summary's Show in list: the main window shown, its downloads selected (in All
    /// Downloads when the view shown hides one), the first scrolled to.
    fn show_in_list(&mut self, ids: &[u64], ctx: &egui::Context) {
        self.show_window(ctx);
        let ids: Vec<u64> = ids
            .iter()
            .copied()
            .filter(|id| self.items.iter().any(|i| i.id == *id))
            .collect();
        let Some(&first) = ids.first() else { return };
        let shown = self.shown_ids();
        if ids.iter().any(|id| !shown.contains(id)) {
            self.view = View::All;
        }
        self.selection.set_many(&ids);
        self.list.scroll_to = self.shown_ids().iter().position(|x| *x == first);
    }

    /// A list that can't be saved says so, once for each reason (not every second); it was
    /// silent and a restart lost the changes.
    fn report_save(&mut self, outcome: Result<(), String>) {
        {
            match outcome {
                Ok(()) => self.save_failed = None,
                Err(e) => {
                    if self.save_failed.as_ref() != Some(&e) {
                        self.notice = Some((
                            crate::i18n::mark("Mister Download Manager"),
                            crate::i18n::fmt(
                                crate::i18n::t(
                                    "The download list could not be saved: {error}. Changes are kept while MDM runs.",
                                ),
                                &[("error", &e)],
                            ),
                        ));
                    }
                    self.save_failed = Some(e);
                }
            }
        }
    }

    // ---- menus -----------------------------------------------------------------------------

    /// IDM 6.42's menus (notes/ui-spec/menus.md); unbuilt features are greyed out.
    fn menu_entries(&self, index: usize) -> Vec<MenuEntry<Cmd>> {
        use MenuEntry::{Item as I, Sep, Sub};
        let sel = self.selected_item();
        let picked = self.chosen(None);
        let running = picked.iter().any(|i| i.live.is_some());
        let resumable = picked.iter().any(|i| i.can_start());
        let deletable = picked.iter().any(|i| i.live.is_none());
        let any_running = self.items.iter().any(|i| i.live.is_some());
        let off = |label| I(label, Cmd::None, false, false);
        #[allow(unused_mut)]
        let mut menu = match index {
            0 => vec![
                I("&Add new download", Cmd::AddUrl, true, false),
                I("Open &torrent file...", Cmd::OpenTorrentFile, true, false),
                I("Add &magnet link...", Cmd::AddMagnet, true, false),
                I("Torrent setti&ngs...", Cmd::TorrentSettings, true, false),
                I("Add &batch download", Cmd::Batch, true, false),
                I(
                    "Add batch download from &clipboard",
                    Cmd::BatchClipboard,
                    true,
                    false,
                ),
                I(
                    "Download &video from a page...",
                    Cmd::VideoPage,
                    true,
                    false,
                ),
                // i18n: Tasks menu: open Link grabber (links in packages, to tick before adding)
                I("&Link grabber...", Cmd::LinkGrabber, true, false),
                // i18n: Tasks menu: the RSS feeds window (feeds of torrents, and rules that add them)
                I("&RSS feeds...", Cmd::RssFeeds(false), true, false),
                Sep,
                I(
                    "Show &drop target",
                    Cmd::ToggleDropTarget,
                    true,
                    self.settings.drop_target,
                ),
                Sep,
                // i18n: verb: save the download list to a file
                Sub(
                    "E&xport",
                    !self.items.is_empty(),
                    vec![
                        I(
                            "To &IDM export file...",
                            Cmd::Export { ef2: true },
                            true,
                            false,
                        ),
                        I("To &text file...", Cmd::Export { ef2: false }, true, false),
                    ],
                ),
                // i18n: verb: load downloads from a file
                Sub(
                    "&Import",
                    true,
                    vec![
                        I(
                            "From &IDM export file...",
                            Cmd::Import { ef2: true },
                            true,
                            false,
                        ),
                        I(
                            "From &text file...",
                            Cmd::Import { ef2: false },
                            true,
                            false,
                        ),
                    ],
                ),
                Sep,
                I("&Exit", Cmd::Exit, true, false),
            ],
            1 => vec![
                I("&Stop Download", Cmd::Stop, running, false),
                // i18n: verb: removes the selected download from the list
                I("&Remove", Cmd::Remove, deletable, false),
                // i18n: verb: removes the selected download from the list and its file from the disk
                I("Remove with &files", Cmd::RemoveWithFiles, deletable, false),
                I("&Download Now", Cmd::Resume, resumable, false),
                // i18n: verb: download the file again from the start
                I(
                    "R&edownload",
                    Cmd::Redownload,
                    sel.is_some_and(|i| self.can_redownload(i.id)),
                    false,
                ),
            ],
            2 => vec![
                I("&Pause All", Cmd::StopAll, any_running, false),
                I("&Stop All", Cmd::StopAll, any_running, false),
                Sep,
                I(
                    "&Delete All Completed",
                    Cmd::DeleteCompleted,
                    self.items.iter().any(|i| i.status == Status::Complete),
                    false,
                ),
                Sep,
                I("Find (Ctrl-F)", Cmd::Find, true, false),
                I("Find Next (F3)", Cmd::FindNext, true, false),
                I(
                    "Select &all",
                    Cmd::SelectAll,
                    !self.shown_ids().is_empty(),
                    false,
                ),
                Sep,
                I("S&cheduler", Cmd::Scheduler, true, false),
                Sub("Start &queue", true, self.queue_drop(true)),
                Sub("S&top queue", true, self.queue_drop(false)),
                Sep,
                Sub(
                    "Speed &Limiter",
                    true,
                    vec![
                        I(
                            "Turn o&n",
                            Cmd::LimiterOn,
                            true,
                            self.settings.speed_limit_on,
                        ),
                        I(
                            "Turn of&f",
                            Cmd::LimiterOff,
                            true,
                            !self.settings.speed_limit_on,
                        ),
                        Sep,
                        I("S&ettings...", Cmd::LimiterSettings, true, false),
                    ],
                ),
                Sep,
                I("&Options", Cmd::Options, true, false),
            ],
            3 => vec![
                I(
                    if self.show_categories {
                        crate::i18n::mark("Hide &categories")
                    } else {
                        crate::i18n::mark("Show &categories")
                    },
                    Cmd::ToggleCategories,
                    true,
                    false,
                ),
                self.arrange_menu(),
                // i18n: noun: the toolbar (submenu of View with its settings)
                Sub("Toolbar", false, vec![]),
                Sub("&Tray icon", false, vec![]),
                off("C&ustomize URL List..."),
                I(
                    "Dark Mode support",
                    Cmd::ToggleDarkMode,
                    true,
                    self.settings.dark_mode_support,
                ),
                // MDM's (#60): dark whatever the system says (a desktop that reports nothing).
                I(
                    // i18n: View menu: a check item; dark mode on, whatever the system's setting
                    "Always dark",
                    Cmd::ToggleAlwaysDark,
                    true,
                    self.settings.always_dark,
                ),
                // i18n: noun: the list's font (submenu)
                Sub("Font", false, vec![]),
                Sep,
                // IDM's place for it; a window rather than IDM's submenu, which 86 languages
                // would make taller than the screen (langpick.rs).
                I("&Language...", Cmd::ChooseLanguage, true, false),
            ],
            // IDM's Help menu, ours (D-020): pages of the user guide (docs/); "Video downloads" stands
            // where IDM has "Grabber Help"; "Report a problem" where "Contact IDM Support";
            // Keyboard shortcuts added; no Tip of the Day.
            _ => vec![
                I("H&elp contents", Cmd::HelpPage("README.md"), true, false),
                I(
                    "&Tutorials",
                    Cmd::HelpPage("README.md#tutorials"),
                    true,
                    false,
                ),
                I(
                    "&Scheduler and queues",
                    Cmd::HelpPage("queues-and-scheduler.md"),
                    true,
                    false,
                ),
                I("&Video downloads", Cmd::HelpPage("video.md"), true, false),
                I("&Keyboard shortcuts", Cmd::Shortcuts, true, false),
                Sep,
                I(
                    "Mister Download Manager &Home Page",
                    Cmd::HelpPage(""),
                    true,
                    false,
                ),
                I("&Report a problem", Cmd::ReportProblem, true, false),
                Sep,
                I("Check for &updates...", Cmd::CheckUpdates, true, false),
                Sep,
                I("&About Mister Download Manager", Cmd::About, true, false),
                I("Tell a &friend", Cmd::TellFriend, true, false),
                // MDM's, not IDM's: the creator's GitHub Sponsors page, asked for only.
                I("Support &MDM", Cmd::Support, true, false),
            ],
        };
        // View → Theme (MDM's, #45), after Always dark; then Draw with (#89, #206), where wgpu is
        // built (Windows, Linux, macOS: renderer.rs).
        if let Some(at) = menu
            .iter()
            .position(|e| matches!(e, I(_, Cmd::ToggleAlwaysDark, ..)))
        {
            menu.insert(at + 1, self.theme_menu());
            menu.insert(at + 2, self.interface_size_menu());
            #[cfg(windows)]
            menu.insert(at + 3, self.text_menu());
            #[cfg(gpu)]
            menu.insert(
                at + if cfg!(windows) { 4 } else { 3 },
                self.draw_with_menu(),
            );
        }
        menu
    }

    /// View → Theme (#45): the built-in themes, the theme files in the themes folder, and a way
    /// to that folder. Picked, a theme applies at once.
    fn theme_menu(&self) -> MenuEntry<Cmd> {
        use MenuEntry::{Item as I, Radio, Sep, Sub};
        let chosen = self.settings.theme.as_str();
        let mut items = Vec::new();
        let mut files = false;
        let on = |id: &str| chosen == id;
        // i18n: View → Theme: MDM's own look, the default (IDM's layout, Windows 11's controls)
        items.push(Radio("Classic", Cmd::Theme("classic"), true, on("classic")));
        // i18n: View → Theme: the system's own colours (its accent colour, its high contrast)
        items.push(Radio("Native", Cmd::Theme("native"), true, on("native")));
        // i18n: View → Theme: black and white with strong edges, for low vision
        items.push(Radio(
            "High contrast",
            Cmd::Theme("high-contrast"),
            true,
            on("high-contrast"),
        ));
        items.push(Sep);
        // i18n: View → Theme: a style theme's name (Windows 8's flat look); keep "Metro" unless your language writes it otherwise
        items.push(Radio("Metro", Cmd::Theme("metro"), true, on("metro")));
        // i18n: View → Theme: a style theme's name (GNOME's look, the Linux desktop), a name: keep "Adwaita" as written
        items.push(Radio("Adwaita", Cmd::Theme("adwaita"), true, on("adwaita")));
        // i18n: View → Theme: a style theme's name (KDE Plasma's look, the Linux desktop), a name: keep "Breeze" as written
        items.push(Radio("Breeze", Cmd::Theme("breeze"), true, on("breeze")));
        // i18n: View → Theme: a style theme's name (Windows XP's look), a name: keep "Luna" as written
        items.push(Radio("Luna", Cmd::Theme("luna"), true, on("luna")));
        // i18n: View → Theme: a style theme's name (Windows 7's glassy look), a name: keep "Aero" as written
        items.push(Radio("Aero", Cmd::Theme("aero"), true, on("aero")));
        // i18n: View → Theme: a style theme's name (Mac OS X's look), a name: keep "Aqua" as written
        items.push(Radio("Aqua", Cmd::Theme("aqua"), true, on("aqua")));
        // i18n: View → Theme: a style theme's name (a white canvas, emerald fills, 6-point corners), a name: keep "Emerald" as written
        items.push(Radio("Emerald", Cmd::Theme("emerald"), true, on("emerald")));
        let wal = crate::themefile::PYWAL;
        for e in self.theme_list.iter().filter(|e| e.path.is_some()) {
            if !files {
                items.push(Sep);
                files = true;
            }
            // pywal's colours, where `wal` writes them, followed as it writes them again.
            if e.id == wal && e.name == wal {
                // i18n: View → Theme: the colours of pywal (a program making colour schemes from the wallpaper), followed as they change; a name: keep "pywal" as written
                items.push(Radio("pywal", Cmd::Theme(wal), true, chosen == wal));
                continue;
            }
            // A file's theme by its own name, untranslated.
            let (id, name) = (crate::chrome::label(&e.id), crate::chrome::label(&e.name));
            items.push(Radio(name, Cmd::Theme(id), true, e.id == chosen));
        }
        items.push(Sep);
        // i18n: View → Theme: picks a theme file or a Base16 or pywal colour scheme, copies it into the themes folder and uses it
        items.push(I("Add a theme file…", Cmd::AddThemeFile, true, false));
        // i18n: View → Theme: opens the folder where theme files go (TOML or JSON)
        items.push(I("Open themes folder", Cmd::OpenThemesFolder, true, false));
        // i18n: View menu: submenu choosing MDM's look (colours, sizes, icons): Classic, Native…
        Sub("Theme", true, items)
    }

    /// View → Interface size (#262): every window larger, as a browser zooms (Ctrl + / Ctrl − /
    /// Ctrl 0 too). The size shown is the zoom drawn now, a test run's MDM_ZOOM too.
    fn interface_size_menu(&self) -> MenuEntry<Cmd> {
        let now = crate::scale::of_zoom(self.zoom);
        let items = crate::scale::STEPS
            .into_iter()
            .map(|p| {
                // As the interface language writes a percentage: 125%, 125 %, %125.
                let label = crate::chrome::label(&crate::i18n::percent(f64::from(p) / 100.0, 0));
                MenuEntry::Radio(label, Cmd::InterfaceSize(p), true, p == now)
            })
            .collect();
        // i18n: View menu: submenu choosing how large MDM draws its windows and text (100 % to 200 %), as a browser's zoom
        MenuEntry::Sub("&Interface size", true, items)
    }

    /// View → Text (Windows): how the letters are drawn. Smooth is DirectWrite's natural look
    /// (any size, each letter where it falls: easy to read on a dense screen); Classic is the
    /// hinted whole-pixel look of Windows' old controls. Applies at once.
    #[cfg(windows)]
    fn text_menu(&self) -> MenuEntry<Cmd> {
        let smooth = self.settings.smooth_text;
        // i18n: View → Text: letters drawn smooth, as Windows' modern apps draw them, anti-aliased at any size
        let a = MenuEntry::Radio("&Smooth", Cmd::SmoothText(true), true, smooth);
        // i18n: View → Text: letters drawn the classic way, hinted to whole pixels (crisp, coarse on a dense screen)
        let b = MenuEntry::Radio("&Classic", Cmd::SmoothText(false), true, !smooth);
        // i18n: View menu: submenu choosing how the text is drawn (smooth or classic)
        MenuEntry::Sub("Te&xt", true, vec![a, b])
    }

    /// View → Draw with (#89, #206): OpenGL or the system's own graphics API (Direct3D on
    /// Windows, Vulkan on Linux, Metal on macOS), used from the next start. Greyed lines say when
    /// what draws now isn't the choice: the system's API failed here (and why), OpenGL can't
    /// draw MDM here (#835), a software renderer draws, or the choice waits for the next start.
    #[cfg(gpu)]
    fn draw_with_menu(&self) -> MenuEntry<Cmd> {
        use crate::renderer::{self, Api, Why};
        use MenuEntry::{Item as I, Radio, Sep, Sub};
        let off = |label| I(label, Cmd::None, false, false);
        let chosen = renderer::chosen(self.settings.renderer);
        let gpu = renderer::GPU.unwrap_or(Api::OpenGl);
        let on = chosen == gpu;
        let draw = Cmd::DrawWith(gpu);
        // Its choice, the note when it failed here, the one when a software renderer draws it,
        // and the one when that is because the card has no driver for it (Windows' WARP, #847).
        let (choice, failed_note, software_note, no_driver_note) = match gpu {
            Api::Direct3D => (
                // i18n: View → Draw with: a graphics system's name (Microsoft's), usually kept as is
                Radio("Direct3D", draw, true, on),
                // i18n: View → Draw with: a greyed note; Direct3D failed on this computer, so OpenGL draws the windows
                off("Direct3D didn't start here: OpenGL draws"),
                // i18n: View → Draw with: a greyed note; Direct3D draws the windows through Windows' own software renderer (WARP) instead of a graphics card
                off("Direct3D draws with Windows' software renderer (WARP)"),
                // i18n: View → Draw with: a greyed note; what draws the windows and why: Direct3D 12 through Windows' own software renderer (WARP, keep the name), because this computer's graphics card has no Direct3D 12 driver
                Some(off(
                    "Direct3D 12 (WARP): this graphics card has no Direct3D 12 driver",
                )),
            ),
            Api::Vulkan => (
                // i18n: View → Draw with: a graphics system's name (Khronos', used on Linux), usually kept as is
                Radio("Vulkan", draw, true, on),
                // i18n: View → Draw with: a greyed note; Vulkan failed on this computer, so OpenGL draws the windows
                off("Vulkan didn't start here: OpenGL draws"),
                // i18n: View → Draw with: a greyed note; Vulkan draws the windows through a software renderer instead of a graphics card
                off("Vulkan draws with a software renderer"),
                None,
            ),
            _ => (
                // i18n: View → Draw with: a graphics system's name (Apple's), usually kept as is
                Radio("Metal", draw, true, on),
                // i18n: View → Draw with: a greyed note; Metal failed on this computer, so OpenGL draws the windows
                off("Metal didn't start here: OpenGL draws"),
                // i18n: View → Draw with: a greyed note; Metal draws the windows through a software renderer instead of a graphics card
                off("Metal draws with a software renderer"),
                None,
            ),
        };
        let mut items = vec![
            // i18n: View → Draw with: a graphics system's name, usually kept as is
            Radio(
                "OpenGL",
                Cmd::DrawWith(Api::OpenGl),
                true,
                chosen == Api::OpenGl,
            ),
            choice,
        ];
        let failed = renderer::gpu_failed().filter(|_| on);
        let active = renderer::active();
        // The system's API on a software renderer (WARP): said, with why below (no card for
        // it, an OpenGL that can't draw MDM).
        let software = active == Some(gpu) && renderer::software();
        // OpenGL can't draw MDM here (#835, #847): said, and why, where it changed what draws.
        let unfit = renderer::gl_failed().filter(|_| active == Some(gpu) && (software || !on));
        // The software renderer draws because no card here has a driver for the system's API:
        // one line says both (Windows, #847).
        let no_driver = no_driver_note.filter(|_| software && failed == Some(Why::NoAdapter));
        if active.is_some_and(|a| a != chosen) || software {
            items.push(Sep);
            let mut notes = Vec::new();
            match (failed, no_driver) {
                (_, Some(note)) => items.push(note),
                (Some(why), None) => {
                    items.push(if software { software_note } else { failed_note });
                    // Why, under it ("it": Direct3D, Vulkan or Metal).
                    notes.push(match why {
                        // i18n: View → Draw with: a greyed note under "… didn't start here"; why: no graphics card here has a driver for that graphics system ("it")
                        Why::NoAdapter => off("No graphics card here has a driver for it"),
                        // i18n: View → Draw with: a greyed note under "… didn't start here"; why: the window stayed empty, so MDM started again with OpenGL
                        Why::NoFirstFrame => off("Its first frame didn't show within 10 seconds"),
                        // i18n: View → Draw with: a greyed note under "… didn't start here"; why: MDM crashed or was closed while starting with it
                        Why::EndedEarly => off("Its last start ended before its first frame"),
                        // i18n: View → Draw with: a greyed note under "… didn't start here"; why: the graphics card stopped working twice (a driver update or reset)
                        Why::LostTwice => off("The graphics card was lost twice in 10 minutes"),
                        // i18n: View → Draw with: a greyed note under "… didn't start here"; why: another error, written in MDM's log
                        _ => off("It couldn't start: the log says why"),
                    });
                }
                // A software renderer that draws for want of a card says only that.
                (None, None) if software => items.push(software_note),
                (None, None) if unfit.is_some() => {}
                // i18n: View → Draw with: a greyed note under the choices; the choice takes effect when MDM is started again
                (None, None) => notes.push(off("Applies the next time MDM starts")),
            }
            if let Some(why) = unfit {
                notes.push(match why {
                    // i18n: View → Draw with: a greyed note; this computer's OpenGL (its graphics driver) is too old, so MDM doesn't draw with it
                    Why::TooOld => off("OpenGL here is too old to draw MDM"),
                    // i18n: View → Draw with: a greyed note; why MDM no longer draws with OpenGL: its window froze inside the graphics card's driver while OpenGL drew
                    Why::FrozeInDriver => off("OpenGL froze inside the graphics driver here"),
                    // i18n: View → Draw with: a greyed note; why MDM no longer draws with OpenGL: its window stayed empty for 10 seconds, so MDM started again another way
                    Why::NoFirstFrame => off("OpenGL's first frame didn't show within 10 seconds"),
                    // i18n: View → Draw with: a greyed note; why MDM no longer draws with OpenGL: MDM crashed or was closed while starting with it
                    Why::EndedEarly => off("OpenGL's last start ended before its first frame"),
                    // i18n: View → Draw with: a greyed note; why MDM no longer draws with OpenGL: it couldn't start on this computer
                    _ => off("OpenGL didn't start here"),
                });
            }
            items.extend(notes);
        }
        // i18n: View menu: submenu choosing what draws MDM's windows: OpenGL, or Direct3D, Vulkan or Metal (graphics systems)
        Sub("Draw with", true, items)
    }

    /// Start Queue / Stop Queue drop-downs (and the Downloads menu's submenus).
    fn queue_drop(&self, start: bool) -> Vec<MenuEntry<Cmd>> {
        let label = |q: usize| {
            let template = if start {
                crate::i18n::t("Start {queue}")
            } else {
                crate::i18n::t("Stop {queue}")
            };
            crate::chrome::label(&crate::i18n::fmt(
                template,
                // A name's "&" is text, not an access key (Tom & Jerry lost it).
                &[(
                    "queue",
                    &crate::i18n::t(&self.queues[q].name).replace('&', "&&"),
                )],
            ))
        };
        (0..self.queues.len())
            .map(|q| {
                let running = self.queues[q].running;
                if start {
                    MenuEntry::Item(label(q), Cmd::StartQueue(q), !running, false)
                } else {
                    MenuEntry::Item(label(q), Cmd::StopQueue(q), running, false)
                }
            })
            .collect()
    }

    /// IDM 6.42's list menu (notes/ui-spec/menus.md "Context menus"): on a row, or on blank
    /// space (`None`: nothing selected, almost everything greyed).
    /// The list's right-click menu. With several downloads selected it acts on all of them
    /// (Resume, Stop, Remove, queues); commands for one download need one.
    /// View → Arrange files, also on the list's blank space.
    fn arrange_menu(&self) -> MenuEntry<Cmd> {
        use MenuEntry::{Item as I, Sub};
        let off = |label| I(label, Cmd::None, false, false);
        Sub("Arrange files", true, {
            let sorted = self.list.sort.map(|s| s.0);
            let by = |label, col: Option<usize>| I(label, Cmd::Arrange(col), true, sorted == col);
            vec![
                by("By &Order Of Addition", None),
                by("By &File Name", Some(0)),
                by("By &Size", Some(2)),
                by("By Stat&us", Some(3)),
                by("By &Time Left", Some(4)),
                by("By Transfer &Rate", Some(5)),
                by("By &Last Try Date", Some(6)),
                by("By Date &Added", Some(list::ADDED)),
                by("By &Description", Some(7)),
                off("By Save &Path"),
                off("By R&eferer"),
                off("By Parent &Web Page"),
            ]
        })
    }

    /// The entries of the popup menu `kind`, and the download it acts on.
    fn popup_entries(&self, kind: Popup) -> (Vec<MenuEntry<Cmd>>, Option<u64>) {
        match kind {
            // Column chooser (IDM: right-click on the header). File Name always stays.
            Popup::Columns => {
                let mut e: Vec<MenuEntry<Cmd>> = list::TITLES
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        // As the header names them (its own words, not the menus'), and
                        // the narrow Q column by its whole name.
                        let name = if *t == "Q" {
                            crate::i18n::tc("column", "Queue")
                        } else {
                            crate::i18n::tc("column", t)
                        };
                        let name = crate::chrome::label(&name.replace('&', "&&"));
                        MenuEntry::Item(name, Cmd::ToggleColumn(i), i != 0, self.list.visible[i])
                    })
                    .collect();
                e.push(MenuEntry::Sep);
                e.push(MenuEntry::Item(
                    "Clear all &filters",
                    Cmd::ClearFilters,
                    self.list.filters.iter().any(Option::is_some),
                    false,
                ));
                // i18n: header menu: put the columns back as they first were (order, widths, all shown)
                e.push(MenuEntry::Item(
                    "&Reset columns",
                    Cmd::ResetColumns,
                    true,
                    false,
                ));
                (e, None)
            }
            Popup::Item(None) => (self.blank_entries(), None),
            Popup::Item(id) => (self.item_entries(id), id),
            Popup::QueueDrop(start) => (self.queue_drop(start), None),
            Popup::TorrentDrop => (
                vec![
                    MenuEntry::Item("Open &torrent file...", Cmd::OpenTorrentFile, true, false),
                    MenuEntry::Item("Add &magnet link...", Cmd::AddMagnet, true, false),
                    MenuEntry::Item("&RSS feeds...", Cmd::RssFeeds(false), true, false),
                    MenuEntry::Sep,
                    MenuEntry::Item("Torrent setti&ngs...", Cmd::TorrentSettings, true, false),
                ],
                None,
            ),
            Popup::Node(node) => (self.node_entries(node), None),
            Popup::DropTarget => (
                vec![
                    MenuEntry::Item("Show Mister Download Manager", Cmd::ShowMain, true, false),
                    MenuEntry::Item("&Add new download", Cmd::AddUrl, true, false),
                    MenuEntry::Sep,
                    MenuEntry::Item("Hide drop target", Cmd::ToggleDropTarget, true, false),
                ],
                None,
            ),
        }
    }

    /// Right-click on the list's blank space: what applies to the whole list (IDM greys the
    /// download menu there).
    fn blank_entries(&self) -> Vec<MenuEntry<Cmd>> {
        use MenuEntry::{Item as I, Sep};
        vec![
            I("Add &new download...", Cmd::AddUrl, true, false),
            Sep,
            I(
                "Select &all",
                Cmd::SelectAll,
                !self.shown_ids().is_empty(),
                false,
            ),
            self.arrange_menu(),
            I(
                "Clear all &filters",
                Cmd::ClearFilters,
                self.list.filters.iter().any(Option::is_some),
                false,
            ),
            Sep,
            I(
                "&Delete All Completed",
                Cmd::DeleteCompleted,
                self.items.iter().any(|i| i.status == Status::Complete),
                false,
            ),
        ]
    }

    fn item_entries(&self, id: Option<u64>) -> Vec<MenuEntry<Cmd>> {
        use MenuEntry::{Item as I, Radio, Sep, Sub};
        let chosen = if id.is_some() {
            self.chosen(id)
        } else {
            Vec::new()
        };
        let it = (chosen.len() == 1).then(|| chosen[0]);
        let sel = it.is_some();
        let done = it.is_some_and(|i| i.status == Status::Complete);
        let live = it.is_some_and(|i| i.live.is_some());
        let any = !chosen.is_empty();
        let any_live = chosen.iter().any(|i| i.live.is_some());
        let any_resumable = chosen.iter().any(|i| i.can_start());
        let any_idle = chosen.iter().any(|i| i.live.is_none());
        let all_done = any && chosen.iter().all(|i| i.status == Status::Complete);
        let queue_of = |q: &Queue| chosen.iter().all(|i| q.items.contains(&i.id)) && any;
        let in_queue = self
            .queues
            .iter()
            .any(|q| chosen.iter().any(|i| q.items.contains(&i.id)));
        // The other queues they can go to; finished files only to synchronization queues.
        let targets: Vec<MenuEntry<Cmd>> = self
            .queues
            .iter()
            .enumerate()
            .filter(|(_, q)| !queue_of(q) && (!all_done || q.sync))
            .map(|(q, queue)| {
                I(
                    crate::chrome::label(&crate::i18n::t(&queue.name).replace('&', "&&")),
                    Cmd::AddToQueue(q),
                    true,
                    false,
                )
            })
            .chain([I("Create new queue", Cmd::NewQueue, true, false)])
            .collect();
        let open = self.settings.double_click_open;
        // Several selected: enabled when one of them can go; the others are said to stay (#333).
        let movable = chosen.iter().any(|i| can_move(i));
        let redo = it.is_some_and(|i| self.can_redownload(i.id));
        let extractable = chosen.iter().any(|i| {
            i.status == Status::Complete
                && i.torrent.is_none()
                && i.video.is_none()
                && crate::extractq::archive_name(&i.file_name)
        });
        let torrent = it.and_then(|i| i.torrent.as_ref().map(|t| (i, t)));
        let mut entries = vec![
            // i18n: verb: opens the downloaded file
            I("Open", Cmd::Open, done, false),
            I("Open with...", Cmd::OpenWith, done, false),
            // A torrent's folder is there while it downloads (#354).
            I(
                "Open folder",
                Cmd::OpenFolder,
                done || it.is_some_and(|i| i.torrent.is_some()),
                false,
            ),
            Sep,
            I("Move/Rename (Ctrl-M)", Cmd::MoveRename, movable, false),
            Sep,
            // i18n: verb: download the file again from the start
            I("Redownload", Cmd::Redownload, redo, false),
            Sep,
            I("Resume Download", Cmd::Resume, any_resumable, false),
            I("Stop Download", Cmd::Stop, any_live, false),
            Sep,
            I(
                "Refresh download address",
                Cmd::RefreshAddress,
                sel && !live && !done,
                false,
            ),
            I("Verify checksum...", Cmd::VerifyChecksum, sel, false),
            // i18n: verb: unpack the downloaded archive (zip, 7z, rar) into its folder
            I("Extract", Cmd::Extract, extractable, false),
            Sep,
            // i18n: verb: removes the download from the list
            I("Remove", Cmd::Remove, any_idle, false),
            // i18n: verb: removes the download from the list and its file from the disk
            I("Remove with files", Cmd::RemoveWithFiles, any_idle, false),
            Sep,
            Sub(
                if in_queue {
                    crate::i18n::mark("Move to queue")
                } else {
                    crate::i18n::mark("Add to queue")
                },
                any,
                targets,
            ),
            I("Delete from queue", Cmd::RemoveFromQueue, in_queue, false),
            Sep,
            Sub(
                "On Double click",
                true,
                vec![
                    // i18n: what double-clicking a download does — verb: open the file
                    Radio("Open", Cmd::DoubleClickOpen(true), true, open),
                    // i18n: what double-clicking a download does — noun: show its properties
                    Radio("Properties", Cmd::DoubleClickOpen(false), true, !open),
                ],
            ),
            Sep,
            // i18n: noun: opens the download's Properties window
            I("Properties", Cmd::Properties, sel, false),
        ];
        // A torrent's own actions (#39), after Verify checksum.
        if let Some((i, spec)) = torrent {
            let busy = self.torrent_host.busy(&spec.hash);
            // Running or shared now (the session has it going): Reannounce has someone to ask.
            let going = self
                .torrent_host
                .get()
                .and_then(|t| t.seen(&spec.hash))
                .is_some_and(|s| {
                    matches!(
                        s.phase,
                        mdm_torrent::Phase::Downloading | mdm_torrent::Phase::Finished
                    )
                });
            let at = entries
                .iter()
                .position(|e| matches!(e, I(_, Cmd::VerifyChecksum, _, _)))
                .map_or(entries.len(), |p| p + 1);
            entries.insert(
                at,
                Sub(
                    // i18n: right-click menu: the submenu of a torrent's own commands
                    "Torrent",
                    true,
                    vec![
                        // i18n: right-click → Torrent submenu: open the selected torrent's file list
                        I("Files...", Cmd::TorrentFiles, true, false),
                        Sep,
                        // i18n: right-click → Torrent: read every piece of the torrent's files again and check it
                        I(
                            "Recheck files",
                            Cmd::TorrentRecheck,
                            !busy && i.status != Status::Queued,
                            false,
                        ),
                        // i18n: right-click → Torrent: ask the torrent's trackers for peers again now
                        I("Reannounce", Cmd::TorrentReannounce, !busy && going, false),
                        I("Add trackers...", Cmd::TorrentAddTrackers, !busy, false),
                        Sep,
                        // i18n: right-click → Torrent, a choice with a check mark: download the torrent's files from their start, to watch a video while it downloads
                        I(
                            "Download in order",
                            Cmd::TorrentInOrder,
                            true,
                            spec.in_order,
                        ),
                        // i18n: right-click → Torrent, a choice with a check mark (with Download in order): each file's first and last pieces first, which a video player needs to start
                        I(
                            "First and last pieces first",
                            Cmd::TorrentEndsFirst,
                            spec.in_order,
                            spec.in_order && spec.ends_first,
                        ),
                        Sep,
                        I("Copy magnet link", Cmd::TorrentCopyMagnet, true, false),
                    ],
                ),
            );
        }
        entries
    }

    /// Categories right-click: categories (Browse, Properties, Add category, Delete; greyed
    /// on the built-in nodes) and queues (Start now, Stop, Edit queue, Schedule, Delete,
    /// Create new queue).
    fn node_entries(&self, node: Option<View>) -> Vec<MenuEntry<Cmd>> {
        use MenuEntry::{Item as I, Sep};
        let off = |label| I(label, Cmd::None, false, false);
        let queue = match node {
            Some(View::Queue(q)) => Some(q),
            None => None,
            Some(v) => {
                let cat = match v {
                    View::Cat(c) | View::Unfinished(Some(c)) | View::Finished(Some(c)) => Some(c),
                    _ => None,
                };
                return vec![
                    match cat {
                        // i18n: verb: opens this category's download folder
                        Some(c) => I("Browse", Cmd::Browse(c), true, false),
                        // i18n: verb: opens this category's download folder
                        None => off("Browse"),
                    },
                    match cat {
                        // i18n: noun: opens the category's properties
                        Some(c) => I("Properties", Cmd::CategoryProps(c), true, false),
                        // i18n: noun: opens the category's properties
                        None => off("Properties"),
                    },
                    Sep,
                    I("Add category", Cmd::AddCategory, true, false),
                    match cat {
                        // i18n: verb: deletes the category
                        Some(c) if c.editable() => I("Delete", Cmd::DeleteCategory(c), true, false),
                        // i18n: verb: deletes the category
                        _ => off("Delete"),
                    },
                ];
            }
        };
        let running = queue.is_some_and(|q| self.queues[q].running);
        vec![
            match queue {
                Some(q) => I("Start now", Cmd::StartQueue(q), !running, false),
                None => off("Start now"),
            },
            match queue {
                // i18n: verb: stops the queue
                Some(q) => I("Stop", Cmd::StopQueue(q), running, false),
                // i18n: verb: stops the queue
                None => off("Stop"),
            },
            Sep,
            match queue {
                Some(q) => I("Edit queue", Cmd::EditQueue(q), true, false),
                None => off("Edit queue"),
            },
            match queue {
                // i18n: command: opens this queue's schedule (when it starts and stops)
                Some(q) => I("Schedule", Cmd::ScheduleQueue(q), true, false),
                // i18n: command: opens this queue's schedule (when it starts and stops)
                None => off("Schedule"),
            },
            Sep,
            // The two built-in queues cannot be deleted.
            match queue {
                // i18n: verb: deletes the queue
                Some(q) if q >= 2 => I("Delete", Cmd::DeleteQueue(q), !running, false),
                // i18n: verb: deletes the queue
                _ => off("Delete"),
            },
            Sep,
            I("Create new queue", Cmd::NewQueue, true, false),
        ]
    }

    // ---- painting --------------------------------------------------------------------------

    /// The Categories tree: each row, the view it selects (None: just toggles) and its branch.
    fn tree_rows(&self) -> Vec<(TreeRow<'static>, Option<View>, Branch)> {
        let cur = self.view;
        let row = |level, label: &'static str, expander, icon, badge, view: Option<View>| TreeRow {
            level,
            label: label.into(),
            expander,
            selected: view == Some(cur),
            icon,
            badge,
            count: None,
        };
        let mut rows = Vec::new();
        // Silk icons in IDM's roles: folder, folder + green arrow, folder + tick, page stacks.
        let mut branch =
            |label, open: bool, icon, badge, view: View, b: Branch, child: fn(Category) -> View| {
                rows.push((
                    row(0, label, Some(open), icon, badge, Some(view)),
                    Some(view),
                    b,
                ));
                if open {
                    for c in Category::named() {
                        let v = child(c);
                        rows.push((
                            TreeRow {
                                level: 1,
                                label: c.label(),
                                expander: None,
                                selected: v == cur,
                                icon: category_icon(c),
                                badge: None,
                                count: None,
                            },
                            Some(v),
                            Branch::Leaf,
                        ));
                    }
                }
            };
        branch(
            "All Downloads",
            self.all_open,
            IconKey::Themed("cat-all", "folder"),
            None,
            View::All,
            Branch::All,
            View::Cat,
        );
        branch(
            // i18n: adjective: the downloads that are not complete yet
            "Unfinished",
            self.unfinished_open,
            IconKey::Themed("cat-unfinished", "folder_go"),
            None,
            View::Unfinished(None),
            Branch::Unfinished,
            |c| View::Unfinished(Some(c)),
        );
        branch(
            // i18n: adjective: the downloads that are complete
            "Finished",
            self.finished_open,
            IconKey::Themed("cat-finished", "folder"),
            Some("tick"),
            View::Finished(None),
            Branch::Finished,
            |c| View::Finished(Some(c)),
        );
        let stack = IconKey::Silk("page_white_stack");
        rows.push((
            row(
                0,
                // i18n: plural noun: the download queues
                "Queues",
                Some(self.queues_open),
                IconKey::Themed("cat-queues", "folder"),
                Some("page_white_stack"),
                None,
            ),
            None,
            Branch::Queues,
        ));
        if self.queues_open {
            // Every queue, the user's too (IDM lists them all here).
            for (q, queue) in self.queues.iter().enumerate() {
                let view = Some(View::Queue(q));
                rows.push((
                    TreeRow {
                        level: 1,
                        label: queue.name.clone().into(),
                        expander: None,
                        selected: view == Some(cur),
                        icon: stack.clone(),
                        badge: queue.sync.then_some("tick"),
                        count: None,
                    },
                    view,
                    Branch::Leaf,
                ));
            }
        }
        // Each top node's downloads, as its list would show them (before column filters):
        // the pane is narrow, its children's names fill it already.
        for (row, view, _) in &mut rows {
            if row.level == 0
                && let Some(v) = view
            {
                let n = self.items.iter().filter(|i| self.in_view(*v, i)).count();
                row.count = (n > 0).then_some(n);
            }
        }
        rows
    }

    fn open_popup(&mut self, kind: Popup, at: Pos2) {
        self.popup_serial += 1;
        self.popup = Some((kind, at, self.popup_serial));
    }

    /// The main window's title bar where the theme draws its own (Windows only: elsewhere the
    /// window manager owns the title bar): the frame is switched to ours or back as the theme
    /// changes, the bar drawn at the top of `full`; returns what is left for the layout.
    fn drawn_caption(&mut self, ui: &mut Ui, full: egui::Rect) -> egui::Rect {
        #[cfg(windows)]
        {
            use crate::winui::caption::{self, CAPTION_H, CaptionButton};
            let want =
                crate::theme::current().controls.caption != crate::palette::CaptionLook::System;
            if self.drawn_frame != Some(want)
                && let Some(h) = crate::win32::drawn_frame(crate::MAIN_TITLE, want)
            {
                self.drawn_frame = Some(want);
                self.main_hwnd = Some(h);
                ui.ctx().request_repaint();
            }
            if !want || self.drawn_frame != Some(true) {
                return full;
            }
            let rect = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), CAPTION_H));
            if let Some(h) = self.main_hwnd {
                let m = crate::win32::max_button_state(h);
                caption::frame_maximize(m.hot, m.pressed);
            }
            let buttons = [
                (CaptionButton::Close, true),
                (CaptionButton::Maximize, true),
                (CaptionButton::Minimize, true),
            ];
            let bar = caption::title_bar(ui, rect, crate::MAIN_TITLE, &buttons);
            if let Some(h) = self.main_hwnd {
                // The sizing frame above the bar (Windows 11, #208) in the bar's colour.
                crate::win32::caption_colour(h, [bar.top.r(), bar.top.g(), bar.top.b()]);
                let ppp = ui.ctx().pixels_per_point();
                let px = |r: egui::Rect| {
                    let (a, b) = ((r.min - full.min) * ppp, (r.max - full.min) * ppp);
                    [
                        a.x.round() as i32,
                        a.y.round() as i32,
                        b.x.round() as i32,
                        b.y.round() as i32,
                    ]
                };
                crate::win32::set_caption_hits(
                    h,
                    crate::win32::CaptionHits {
                        strip_h: (CAPTION_H * ppp).round() as i32,
                        icon: px(bar.icon),
                        buttons: bar.buttons.iter().map(|r| px(*r)).collect(),
                        max_button: Some(1),
                    },
                );
            }
            let ctx = ui.ctx();
            match bar.pressed {
                Some(CaptionButton::Close) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Some(CaptionButton::Minimize) => crate::dialogs::minimize(ctx, self.main_hwnd),
                Some(CaptionButton::Maximize) => match self.main_hwnd {
                    // The system's command, as the frame's own Maximize sends it (#874).
                    Some(h) => crate::win32::toggle_maximized(h),
                    None => {
                        let max = ctx.input(|i| i.viewport().maximized) == Some(true);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                    }
                },
                _ => {}
            }
            egui::Rect::from_min_max(egui::pos2(full.min.x, full.min.y + CAPTION_H), full.max)
        }
        #[cfg(not(windows))]
        {
            let _ = ui;
            full
        }
    }

    fn toggle(&mut self, b: Branch) {
        match b {
            Branch::All => self.all_open = !self.all_open,
            Branch::Unfinished => self.unfinished_open = !self.unfinished_open,
            Branch::Finished => self.finished_open = !self.finished_open,
            Branch::Queues => self.queues_open = !self.queues_open,
            Branch::Leaf => {}
        }
    }

    fn paint(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let full = ui.max_rect();
        // The client area, laid out from the right in a right-to-left interface (winui).
        let _surface = winui::Surface::enter(full.min.x, full.max.x);
        // A style theme's title bar, drawn above the layout (Windows; #45), mirrored right to
        // left as the progress window's.
        let full = self.drawn_caption(ui, full);
        let o = full.min;
        let (w, h) = (full.width(), full.height());
        let mut chosen: Option<(Cmd, Option<u64>)> = None;

        // The menu bar from the keyboard: F10 or Alt pressed alone opens its first menu, Alt and a
        // menu's underlined letter that menu (Windows and Linux); on macOS, whose apps have no
        // Alt keys, Ctrl+F2 (the system's key for the menu bar).
        // A focused button or list (Tab, a screen reader, #328) leaves these keys to the menu bar;
        // only a text box keeps them.
        let typing = ctx
            .memory(|m| m.focused())
            .is_some_and(|id| egui::TextEdit::load_state(&ctx, id).is_some());
        if self.open_menu.is_none() && self.popup.is_none() && !typing {
            let labels: Vec<String> = chrome::menus(ui.painter())
                .iter()
                .map(|(l, ..)| chrome::menu_letter(l))
                .collect();
            let (alt, other, f10, ctrl_f2, letter) = ctx.input(|i| {
                let mut letter = None;
                let mut other = false;
                for e in &i.events {
                    match e {
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            other = true;
                            if modifiers.alt && !modifiers.ctrl && !modifiers.command {
                                letter = Some(key.name().to_lowercase());
                            }
                        }
                        egui::Event::PointerButton { pressed: true, .. } => other = true,
                        // A letter of another script (Cyrillic, Arabic...) typed with Alt.
                        egui::Event::Text(t) if i.modifiers.alt && !i.modifiers.ctrl => {
                            letter = Some(t.to_lowercase());
                        }
                        _ => {}
                    }
                }
                (
                    i.modifiers.alt,
                    other,
                    i.key_pressed(egui::Key::F10) && i.modifiers.is_none(),
                    i.key_pressed(egui::Key::F2) && i.modifiers.ctrl,
                    letter,
                )
            });
            let mut open = None;
            if cfg!(target_os = "macos") {
                if ctrl_f2 {
                    open = Some(0);
                }
            } else {
                if alt && !self.alt_down {
                    self.alt_alone = true;
                    // The keyboard is in use: access letters show (light mode too).
                    crate::chrome::show_cues(&ctx);
                }
                if alt && other {
                    self.alt_alone = false;
                }
                if !alt && self.alt_down && self.alt_alone {
                    open = Some(0);
                }
                if f10 {
                    open = Some(0);
                }
                if let Some(l) = letter {
                    open = labels.iter().position(|m| *m == l).or(open);
                }
            }
            self.alt_down = alt;
            if let Some(i) = open {
                self.alt_alone = false;
                self.open_menu = chrome::menubar_next(self.open_menu, MenuBarEvent::Open(i));
                chrome::start_keyboard(&ctx, &format!("menu-{i}"));
            }
        } else {
            self.alt_down = false;
            self.alt_alone = false;
        }

        // Menu bar and its popups: which menu is open is chrome::menubar_next's to say.
        if let Some(event) = chrome::menu_bar(ui, o, w, self.open_menu) {
            self.open_menu = chrome::menubar_next(self.open_menu, event);
        }
        // A menu opened by Left or Right is drawn in the same pass as the one it replaces (a
        // second turn): on X11 the bar's menus share one window (chrome::popup), and a pass
        // without a menu closed it, a new window opening for the next (#277).
        for _ in 0..2 {
            let Some(i) = self.open_menu else { break };
            let x = chrome::menus(ui.painter())[i].1;
            let entries = self.menu_entries(i);
            // Under the item's leading edge (the menu's top-right in RTL).
            let at = pos2(winui::mx(o.x + x), o.y + chrome::MENU_H - 1.0);
            let event = match chrome::popup(
                &ctx,
                MenuKind::Bar,
                &format!("menu-{i}"),
                at,
                &entries,
                &|c: &Cmd| c.hint(),
            ) {
                PopupResult::Open => None,
                PopupResult::Neighbor(d) => {
                    let n = chrome::menus(ui.painter()).len() as i32;
                    let j = (i as i32 + d).rem_euclid(n) as usize;
                    chrome::start_keyboard(&ctx, &format!("menu-{j}"));
                    Some(MenuBarEvent::Open(j))
                }
                PopupResult::Chosen(c) => {
                    chosen = Some((c, None));
                    Some(MenuBarEvent::Chosen)
                }
                PopupResult::Dismissed => {
                    // Shut by a press on a title: its click (on release) is `menu_bar`'s. Not
                    // by Escape with the pointer merely resting on the bar.
                    let press =
                        ui.input(|x| x.pointer.press_origin().filter(|_| x.pointer.any_pressed()));
                    let press_on_title = press
                        .is_some_and(|at| chrome::menubar_title_at(ui.painter(), o, at).is_some());
                    Some(MenuBarEvent::Dismissed { press_on_title })
                }
            };
            let neighbor = matches!(event, Some(MenuBarEvent::Open(_)));
            if let Some(event) = event {
                self.open_menu = chrome::menubar_next(self.open_menu, event);
            }
            if !neighbor {
                break;
            }
        }

        // Toolbar — full labels (IDM truncates "Delete Co...", "Start Qu...": user decision).
        let picked = self.chosen(None);
        let running = picked.iter().any(|i| i.live.is_some());
        let resumable = picked.iter().any(|i| i.can_start());
        let deletable = picked.iter().any(|i| i.live.is_none());
        let any_running = self.items.iter().any(|i| i.live.is_some());
        let any_complete = self.items.iter().any(|i| i.status == Status::Complete);
        let buttons = [
            (Icon::AddUrl, "Add URL", true, Cmd::AddUrl),
            (Icon::AddTorrent, "Add Torrent", true, Cmd::OpenTorrentFile),
            // i18n: verb: continue the selected downloads
            (Icon::Resume, "Resume", resumable, Cmd::Resume),
            // i18n: verb: stop the selected downloads
            (Icon::Stop, "Stop", running, Cmd::Stop),
            (Icon::StopAll, "Stop All", any_running, Cmd::StopAll),
            // i18n: verb: delete the selected downloads
            (Icon::Delete, "Delete", deletable, Cmd::Remove),
            (
                Icon::DeleteCompleted,
                "Delete Completed",
                any_complete,
                Cmd::DeleteCompleted,
            ),
            // i18n: plural noun: opens the Options window (settings)
            (Icon::Options, "Options", true, Cmd::Options),
            // i18n: noun: the tool that starts and stops queues on time
            (Icon::Scheduler, "Scheduler", true, Cmd::Scheduler),
            (
                Icon::StartQueue,
                "Start Queue",
                // The button acts on the main queue (the others: its arrow, the menus).
                self.queues.first().is_some_and(|q| !q.running),
                Cmd::StartQueue(0),
            ),
            (
                Icon::StopQueue,
                "Stop Queue",
                self.queues.first().is_some_and(|q| q.running),
                Cmd::StopQueue(0),
            ),
        ];
        let tb: Vec<(Icon, &str, bool)> = buttons.iter().map(|(i, l, e, _)| (*i, *l, *e)).collect();
        match chrome::toolbar(ui, o, w, &tb) {
            Some(ToolbarEvent::Click(k)) => chosen = Some((buttons[k].3, None)),
            Some(ToolbarEvent::Drop(k, at)) => match buttons[k].0 {
                Icon::AddTorrent => self.open_popup(Popup::TorrentDrop, at),
                icon => self.open_popup(Popup::QueueDrop(matches!(icon, Icon::StartQueue)), at),
            },
            None => {}
        }

        // Categories pane.
        let list_left = if self.show_categories {
            let rows = self.tree_rows();
            let tree: Vec<TreeRow<'_>> = rows.iter().map(|(r, ..)| r.clone()).collect();
            // The splitter: the 4 px gap between the pane and the list, dragged sideways.
            let max_w = (w * 0.6).max(100.0);
            let cat_w = (self.settings.categories_width as f32).clamp(100.0, max_w);
            let right = o.x + 6.0 + cat_w;
            let split = winui::rect(egui::Rect::from_min_max(
                pos2(right, o.y + chrome::MENU_H + chrome::pane_top()),
                pos2(right + 4.0, o.y + h - STRIP_H - 5.0),
            ));
            let sr = ui.interact(
                split,
                egui::Id::unique("cat-splitter"),
                winui::pointer_drag(),
            );
            if sr.hovered() || sr.dragged() {
                ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if sr.dragged()
                && let Some(p) = sr.interact_pointer_pos()
            {
                // The pointer stays on the splitter's middle (the pane's width from its leading
                // edge: the right in RTL).
                let nw = (winui::mx(p.x) - o.x - 6.0 - 2.0)
                    .round()
                    .clamp(100.0, max_w);
                self.settings.categories_width = nw as u32;
            }
            if sr.drag_stopped() {
                self.dirty = true;
            }
            // The tree's keys while it has them (Tab moves between it and the list): Up / Down
            // through the nodes, Left / Right close and open a branch, Home / End.
            let free = self.popup.is_none()
                && self.open_menu.is_none()
                && ctx.memory(|m| m.focused().is_none());
            if free
                && ctx.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                        || i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab)
                })
            {
                self.tree_keys = !self.tree_keys;
                self.list.set_keys(!self.tree_keys);
                crate::chrome::show_cues(&ctx);
            }
            let mut tree_event = None;
            if free && self.tree_keys {
                let at = rows
                    .iter()
                    .position(|(_, v, _)| *v == Some(self.view))
                    .unwrap_or(0);
                let with_view: Vec<usize> =
                    (0..rows.len()).filter(|k| rows[*k].1.is_some()).collect();
                let pos = with_view.iter().position(|k| *k == at).unwrap_or(0);
                let key = |k| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
                let to = if key(egui::Key::ArrowDown) {
                    with_view.get(pos + 1).copied()
                } else if key(egui::Key::ArrowUp) {
                    pos.checked_sub(1).and_then(|p| with_view.get(p).copied())
                } else if key(egui::Key::Home) {
                    with_view.first().copied()
                } else if key(egui::Key::End) {
                    with_view.last().copied()
                } else {
                    None
                };
                if let Some(k) = to {
                    tree_event = Some(TreeEvent::Select(k));
                }
                let open = rows[at].0.expander;
                let (left, right) = if winui::rtl() {
                    (egui::Key::ArrowRight, egui::Key::ArrowLeft)
                } else {
                    (egui::Key::ArrowLeft, egui::Key::ArrowRight)
                };
                if (open == Some(true) && key(left)) || (open == Some(false) && key(right)) {
                    tree_event = Some(TreeEvent::Toggle(at));
                }
            }
            if let Some(ev) = chrome::categories(
                ui,
                &mut self.icons,
                o,
                o.y + h - STRIP_H,
                right,
                &tree,
                self.tree_keys && free,
            )
            .or(tree_event)
            {
                // A click in the tree gives it the keys.
                self.tree_keys = true;
                self.list.set_keys(false);
                match ev {
                    TreeEvent::Close => self.show_categories = false,
                    TreeEvent::Select(k) => match rows[k].1 {
                        Some(v) => self.view = v,
                        None => self.toggle(rows[k].2),
                    },
                    TreeEvent::Toggle(k) => self.toggle(rows[k].2),
                    TreeEvent::Context(k, at) => {
                        // Leaves and nodes with a view; "Queues" (a branch without one) has
                        // the queue menu, all greyed.
                        let node = rows[k].1;
                        if node.is_some() || rows[k].2 == Branch::Queues {
                            self.open_popup(Popup::Node(node), at);
                        }
                    }
                }
            }
            6.0 + cat_w + 4.0
        } else {
            6.0
        };

        // Download list.
        let (ids, rows) = self.list_rows();
        self.selection.retain(&ids);
        let (selected, focus) = self.selection.rows(&ids);
        let outer = egui::Rect::from_min_max(
            pos2(o.x + list_left, o.y + chrome::MENU_H + chrome::pane_top()),
            pos2(o.x + w - 7.0, o.y + h - STRIP_H - 4.0),
        );
        self.list.menu_open = self.popup.is_some() || self.open_menu.is_some();
        // Another node shows from its top (its rows aren't the last one's).
        if self.list_view != self.view {
            self.list.scroll = 0.0;
            self.list_view = self.view;
        }
        // The Status cell's tip, the download's recent events: made for the row under the
        // pointer only (twelve dated lines for every row, every frame, before).
        let items = &self.items;
        let tip = |i: usize| {
            ids.get(i)
                .and_then(|id| items.iter().find(|it| it.id == *id))
                .map_or(String::new(), |it| it.log_lines(12))
        };
        let r = list::show(
            ui,
            outer,
            &mut self.list,
            &rows,
            &selected,
            focus,
            &mut self.icons,
            &tip,
        );
        if self.list.has_keys() {
            self.tree_keys = false;
        }
        if let Some(pick) = r.pick {
            self.selection.apply(pick, &ids);
        }
        if let Some(i) = r.properties {
            self.properties(ids[i]);
        }
        // A finished download: Open or Properties (IDM's default), as the context menu sets.
        if let Some(i) = r.double_clicked {
            let id = ids[i];
            let complete = self
                .items
                .iter()
                .any(|it| it.id == id && it.status == Status::Complete);
            if complete && !self.settings.double_click_open {
                self.properties(id);
            } else {
                self.open_item(id);
            }
        }
        if let Some(i) = r.enter {
            self.open_item(ids[i]);
        }
        // Something droppable over the window: the list is where it goes.
        let dragging = self.drops.over() || ctx.input(|i| !i.raw.hovered_files.is_empty());
        if dragging {
            ui.painter().rect_stroke(
                winui::rect(outer),
                0.0,
                egui::Stroke::new(2.0, winui::accent()),
                egui::StrokeKind::Inside,
            );
        }
        if ids.is_empty()
            && let Some(cmd) = self.empty_state(ui, r.body.unwrap_or(outer))
        {
            chosen = Some((cmd, None));
        }
        self.status_strip(ui, o, w, h, ids.len());
        if let Some((i, pos)) = r.context {
            self.open_popup(Popup::Item(i.map(|i| ids[i])), pos);
        }
        let layout = self.list.layout();
        if self.last_layout.as_ref() != Some(&layout) {
            self.dirty |= self.last_layout.is_some();
            self.last_layout = Some(layout);
        }
        if let Some(pos) = r.header_context {
            self.open_popup(Popup::Columns, pos);
        }
        if let Some((kind, pos, serial)) = self.popup {
            let (entries, target) = self.popup_entries(kind);
            match chrome::popup(
                &ctx,
                MenuKind::Context,
                &format!("popup-{serial}"),
                pos,
                &entries,
                &|c: &Cmd| c.hint(),
            ) {
                PopupResult::Open | PopupResult::Neighbor(_) => {}
                PopupResult::Chosen(c) => {
                    chosen = Some((c, target));
                    self.popup = None;
                }
                PopupResult::Dismissed => self.popup = None,
            }
        }
        // Ctrl+F finds, F3 finds the next match (Downloads menu); Ctrl+N adds a download and
        // Ctrl+B the links in the clipboard (#709), neither in IDM. Not Ctrl+Shift+V for that: the
        // windowing library answers the paste keys by reading the clipboard on the window's
        // thread (#418). The full list: Help → Keyboard shortcuts (`help.rs`).
        if chosen.is_none() && self.popup.is_none() && self.open_menu.is_none() {
            let (find, next, add, clip) = ctx.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::F),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::F3),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::N),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::B),
                )
            });
            // Ctrl+M as IDM; F2 as renaming in Explorer.
            if ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::M)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::F2)
            }) {
                chosen = Some((Cmd::MoveRename, None));
            }
            if find {
                chosen = Some((Cmd::Find, None));
            } else if next {
                chosen = Some((Cmd::FindNext, None));
            } else if add {
                chosen = Some((Cmd::AddUrl, None));
            } else if clip {
                chosen = Some((Cmd::BatchClipboard, None));
            }
        }
        // Delete removes the selected downloads, Shift+Delete with their files (both ask first);
        // not while a text box (Find, a filter) has the keys.
        if chosen.is_none()
            && self.popup.is_none()
            && self.open_menu.is_none()
            && ctx.memory(|m| m.focused().is_none())
        {
            // On Windows egui reports Shift+Delete as Cut (Ctrl+X is Cut too, without Shift).
            let (del, shift_del) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Delete) && !i.modifiers.shift,
                    // Windows reports it as Cut; macOS and Linux as the key.
                    i.modifiers.shift
                        && !i.modifiers.command
                        && (i.events.iter().any(|e| matches!(e, egui::Event::Cut))
                            || i.key_pressed(egui::Key::Delete)),
                )
            });
            if shift_del {
                chosen = Some((Cmd::RemoveWithFiles, None));
            } else if del {
                chosen = Some((Cmd::Remove, None));
            }
        }
        if let Some((c, target)) = chosen {
            self.run(c, target, &ctx);
        }
    }

    /// Download File Info for an address from Add URL, the browser (`context`) or the command
    /// line. One at a time: another that arrives meanwhile waits its turn (the browser has
    /// already given its own copy up, so none may be dropped).
    fn open_file_info(&mut self, new: dialogs::NewUrl, context: ipc::Command, ctx: &egui::Context) {
        if self.file_info.is_some() {
            self.file_info_waiting.push_back((new, context));
            return;
        }
        let direct = crate::debrid::browser_started(&context);
        let request = self.request_parts(&new.url, new.login.as_ref(), &context, direct);
        let mut fi = FileInfo::new(
            new,
            context,
            request,
            self.debrid_route(),
            self.downloads.clone(),
            self.settings.category_dirs.clone(),
            &self.rt,
            ctx,
        );
        fi.remember = self.settings.change_folder_on_last;
        fi.folders = self.settings.folder_history.clone();
        self.file_info = Some(fi);
    }

    /// Options → Downloads → "If a duplicate download link is added", for a new download that
    /// the first row `same` finds already is: what becomes of it ([`duplicate_step`]). The one
    /// already there is replaced (its files and row go), opened or resumed here; asking is the
    /// caller's ([`Self::ask_duplicate`]).
    fn duplicate_of(&mut self, same: impl Fn(&Item) -> bool) -> DupStep {
        if std::mem::take(&mut self.duplicate_cleared) {
            return DupStep::Add;
        }
        let found = self
            .items
            .iter()
            .find(|i| same(i))
            .map(|i| (i.id, model::finished_file_gone(i.status, &i.dest)));
        let step = duplicate_step(self.settings.duplicate_action, found);
        match step {
            // #342: a finished file moved or deleted leaves nothing to duplicate: the old row
            // goes and this is an ordinary new download.
            DupStep::Replace(dup, outputs) => self.replace(dup, outputs),
            DupStep::Existing(dup) => self.open_or_resume(dup),
            DupStep::Add | DupStep::Ask(_) => {}
        }
        step
    }

    /// "Duplicate download link" about `dup`, for `pending`. One question at a time; the others
    /// wait their turn (a second link while the first was asked was dropped).
    fn ask_duplicate(&mut self, dup: u64, pending: Pending) {
        let name = self
            .items
            .iter()
            .find(|i| i.id == dup)
            .map(|i| i.file_name.clone())
            .unwrap_or_default();
        let ask = (dup, pending, dialogs::Duplicate::new(name));
        if self.duplicate.is_none() {
            self.duplicate = Some(ask);
        } else {
            self.duplicates_waiting.push_back(ask);
        }
    }

    /// Options → Downloads → "If a duplicate download link is added": true when adding `url`
    /// goes on now. 0 asks (`pending` waits), 1 adds it with a numbered name, 2 replaces the
    /// download already there, 3 opens that one (finished) or resumes it.
    fn duplicate_gate(&mut self, url: &url::Url, pending: impl FnOnce() -> Pending) -> bool {
        match self.duplicate_of(|i| i.url == *url) {
            DupStep::Add | DupStep::Replace(..) => true,
            DupStep::Existing(_) => false,
            DupStep::Ask(dup) => {
                self.ask_duplicate(dup, pending());
                false
            }
        }
    }

    /// Download video's Download Now / Later, or the browser's quality menu's choice
    /// (`from_menu`), through the same check as any other download: the same page in the same
    /// quality is a duplicate, another quality is not. A question opens its own window (MDM's
    /// stays where it is). What became of it.
    fn add_video_checked(
        &mut self,
        mut add: crate::videodlg::VideoAdd,
        from_menu: bool,
    ) -> DupStep {
        // The Subtitles choice is offered again for the next video (#68), whatever comes of
        // this one.
        if let Some(subs) = add.remember.take() {
            self.settings.video_subs = subs;
            self.dirty = true;
        }
        let (page, selector) = (add.page.clone(), add.selector.clone());
        let step = self.duplicate_of(|i| same_video(i, &page, &selector));
        match video_route(step, add, from_menu) {
            VideoRoute::Add(add) => self.add_video(*add, from_menu),
            VideoRoute::Ask(dup, pending) => self.ask_duplicate(dup, pending),
            VideoRoute::Dropped => {}
        }
        step
    }

    /// The user's answer to "Duplicate download link".
    fn resolve_duplicate(
        &mut self,
        dup: u64,
        pending: Pending,
        choice: usize,
        ctx: &egui::Context,
    ) {
        let there = self.items.iter().any(|i| i.id == dup);
        let (overwrite, pending) = match answered(dup, there, choice, pending) {
            Answer::Existing(dup) => {
                self.open_or_resume(dup);
                return;
            }
            Answer::Again(pending) => {
                self.duplicate_again(pending, ctx);
                return;
            }
            Answer::Add(overwrite, pending) => (overwrite, pending),
        };
        if let Some(dup) = overwrite {
            self.overwrite(dup);
        }
        match pending {
            Pending::Command(cmd) => {
                self.duplicate_cleared = true;
                self.handle_command(*cmd, ctx);
                self.duplicate_cleared = false;
            }
            Pending::Url(new) => self.open_file_info(new, Default::default(), ctx),
            Pending::Video(add, from_menu) => self.add_video(*add, from_menu),
        }
    }

    /// What was asked about a download that has left the list meanwhile: checked afresh, as
    /// when it came (it may be a duplicate of another, or of nothing now).
    fn duplicate_again(&mut self, pending: Pending, ctx: &egui::Context) {
        match pending {
            Pending::Command(cmd) => self.handle_command(*cmd, ctx),
            Pending::Url(new) => {
                if self.duplicate_gate(&new.url, || {
                    Pending::Url(dialogs::NewUrl {
                        url: new.url.clone(),
                        login: new.login.clone(),
                    })
                }) {
                    self.open_file_info(new, Default::default(), ctx);
                }
            }
            Pending::Video(add, from_menu) => {
                self.add_video_checked(*add, from_menu);
            }
        }
    }

    /// The new copy replaces the download already there: its file and its row go (a running
    /// one is left alone; the new copy then gets a numbered name).
    fn overwrite(&mut self, id: u64) {
        self.replace(id, true);
    }

    /// [`Self::overwrite`]; `yt_dlp`: a video's files left by yt-dlp go too, so that the new
    /// copy takes the same name ([`replaced_files`]).
    fn replace(&mut self, id: u64, yt_dlp: bool) {
        let Some(files) = replaced_files(&self.items, id, yt_dlp) else {
            return;
        };
        for f in files {
            let _ = std::fs::remove_file(&f);
        }
        self.delete(id);
    }

    /// The download already there: its "Download complete" box when finished, else resumed.
    fn open_or_resume(&mut self, id: u64) {
        let Some(i) = self.items.iter().find(|i| i.id == id) else {
            return;
        };
        self.selection.set(id);
        if i.status == Status::Complete {
            // Asked for: shown even while Download complete is snoozed.
            self.complete.push(Complete::of(i));
        } else if i.live.is_none() {
            self.start(id, true);
        } else if !self.progress.iter().any(|p| p.id == id) {
            self.progress
                .push(Progress::new(id, self.settings.show_complete));
        }
    }

    /// Tasks → Show drop target: the box while it's on; what is dropped on it is added as if
    /// given on the command line (an address, a magnet link, a .torrent file).
    fn drop_target(&mut self, ctx: &egui::Context) {
        if !self.settings.drop_target {
            self.drop_target = None;
            return;
        }
        // First shown: beside the main window's top-right corner, on the same screen.
        let pos = self
            .settings
            .drop_target_pos
            .map(|(x, y)| pos2(x as f32, y as f32))
            .or_else(|| {
                ctx.input(|i| i.viewport().outer_rect)
                    .map(|r| pos2(r.right() - 90.0, r.top() + 90.0))
            });
        let dt = self
            .drop_target
            .get_or_insert_with(|| crate::droptarget::DropTarget::new(pos));
        let icon = egui::include_image!("../assets/icons/app/mdm-48.png");
        for ev in dt.show(ctx, &icon) {
            use crate::droptarget::Event;
            match ev {
                Event::Drop(s) => self.dropped(vec![s], ctx),
                Event::ShowMain => self.show_window(ctx),
                Event::Context(screen) => self.drop_target_menu(screen, ctx),
                Event::Moved(p) => {
                    self.settings.drop_target_pos = Some((p.x.round() as i32, p.y.round() as i32));
                    self.dirty = true;
                }
            }
        }
    }

    /// The drop target's menu at `screen` (a point on the screen, logical points).
    fn drop_target_menu(&mut self, screen: Pos2, ctx: &egui::Context) {
        // Popups take a point in the main window.
        let origin = ctx
            .input(|i| i.viewport().inner_rect)
            .map_or(Pos2::ZERO, |r| r.min);
        self.open_popup(Popup::DropTarget, screen - origin.to_vec2());
    }

    /// Tasks → Export: every download's address, as an IDM export file or plain text.
    fn export(&mut self, ef2: bool) {
        let (ext, name) = if ef2 {
            ("ef2", "downloads.ef2")
        } else {
            ("txt", "downloads.txt")
        };
        let title = crate::i18n::t("Export").to_owned();
        let key = if ef2 {
            "app-export-ef2"
        } else {
            "app-export-txt"
        };
        winui::start_pick(&self.ui_ctx, key, move || {
            crate::winui::file_dialog(&title)
                .add_filter(ext.to_ascii_uppercase(), &[ext])
                .set_file_name(name)
                .save_file()
        });
    }

    /// Tasks → Export, the file chosen: the addresses written to it.
    fn export_to(&mut self, path: &std::path::Path, ef2: bool) {
        let text = if ef2 {
            crate::transfer::export_ef2(&self.items)
        } else {
            crate::transfer::export_text(&self.items)
        };
        if let Err(e) = std::fs::write(path, text) {
            self.notice = Some((
                crate::i18n::mark("Export"),
                crate::i18n::fmt(
                    crate::i18n::t("The list could not be saved: {error}"),
                    &[("error", &e.to_string())],
                ),
            ));
        }
    }

    /// The window's place and size, the Categories pane and the node shown, kept in the
    /// settings as they change (so they come back at the next start).
    fn remember_layout(&mut self, ctx: &egui::Context) {
        // Screen readers speak in the interface's language (its voice for Arabic, Japanese...).
        let lang = crate::i18n::code();
        if self.a11y_lang != lang {
            ctx.set_language(Some(lang.to_owned()));
            self.a11y_lang = lang;
        }
        let s = &mut self.settings;
        let pane = (
            self.show_categories,
            [
                self.all_open,
                self.unfinished_open,
                self.finished_open,
                self.queues_open,
            ],
            Some(self.view),
        );
        if (s.show_categories, s.branches, s.view) != pane {
            (s.show_categories, s.branches, s.view) = pane;
            self.dirty = true;
        }
        // Not a test instance's off-screen window, nor one zoomed for screenshots (MDM_ZOOM).
        if crate::offscreen() || crate::scale::test_zoom().is_some() {
            return;
        }
        // Kept at 100 % interface size (the system's logical pixels), as the window is made
        // before the zoom applies (main.rs).
        let zoom = ctx.zoom_factor();
        let (outer, inner, maximized, minimized) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.inner_rect, v.maximized, v.minimized)
        });
        if minimized == Some(true) {
            return;
        }
        if let Some(m) = maximized {
            self.maximized = Some(m);
            if s.window_maximized != m {
                s.window_maximized = m;
                self.dirty = true;
            }
        }
        if self.maximized == Some(false)
            && let (Some(outer), Some(inner)) = (outer, inner)
        {
            let rect = [
                (outer.min.x * zoom).round() as i32,
                (outer.min.y * zoom).round() as i32,
                (inner.width() * zoom).round() as i32,
                (inner.height() * zoom).round() as i32,
            ];
            if s.window != Some(rect) {
                s.window = Some(rect);
                self.dirty = true;
            }
        }
    }

    /// View → Interface size (#262): its keys in the main window (the dialogs take them too), and
    /// a new size, from the menu or the keys in any window, saved; the windows that don't follow
    /// it by themselves are made to: the main window's minimum size, the drop target (kept where
    /// it is on the screen). The main window is brought inside the screen once after the start
    /// and after each change (a large size on a small screen), in the frame after it: egui
    /// measures the window at the old size in the frame a new one applies. False in that frame.
    fn follow_zoom(&mut self, ctx: &egui::Context) -> bool {
        const MIN: egui::Vec2 = egui::vec2(600.0, 300.0);
        crate::scale::keys(ctx);
        let zoom = ctx.zoom_factor();
        let settled = zoom == self.zoom;
        if settled && !self.fitted && ctx.input(|i| i.viewport().outer_rect).is_some() {
            self.fitted = true;
            crate::scale::fit_window(ctx, MIN, None);
        }
        // Saved whatever changed it, and kept when Options, opened before, hands back its copy
        // of the settings.
        let percent = crate::scale::of_zoom(zoom);
        if crate::scale::test_zoom().is_none() && self.settings.interface_size != percent {
            self.settings.interface_size = percent;
            self.dirty = true;
        }
        if settled {
            return true;
        }
        let ratio = self.zoom / zoom;
        self.zoom = zoom;
        self.fitted = false;
        tracing::info!("interface size {percent}%");
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(MIN));
        if let Some((x, y)) = self.settings.drop_target_pos {
            let at = |v: i32| (v as f32 * ratio).round() as i32;
            self.settings.drop_target_pos = Some((at(x), at(y)));
        }
        if let Some(d) = &mut self.drop_target {
            d.rescale(ctx, ratio);
        }
        false
    }

    /// What was dropped on the main window (its OLE target on Windows; winit's dropped files on
    /// macOS and Linux, which hand over files only).
    fn main_drops(&mut self, ctx: &egui::Context) {
        #[cfg_attr(windows, allow(unused_mut))]
        let mut got = self.drops.poll(ctx, crate::MAIN_TITLE);
        #[cfg(not(windows))]
        got.extend(ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().display().to_string())
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
        }));
        if !got.is_empty() {
            self.dropped(got, ctx);
        }
    }

    /// Links or files dropped (on the main window, on the drop target): one address, a magnet
    /// link or a .torrent file goes the usual way (Download File Info, Add torrent); several
    /// addresses, or a list file (.txt, .ef2, .htm, .csv), show in Import to tick first.
    fn dropped(&mut self, items: Vec<String>, ctx: &egui::Context) {
        let list_file = |s: &str| {
            let p = std::path::Path::new(s);
            p.is_file()
                && p.extension().is_some_and(|e| {
                    ["txt", "ef2", "htm", "html", "csv"]
                        .contains(&e.to_string_lossy().to_ascii_lowercase().as_str())
                })
        };
        let (lists, items): (Vec<String>, Vec<String>) =
            items.into_iter().partition(|s| list_file(s));
        for file in lists {
            self.import_file(std::path::Path::new(&file));
        }
        let (links, other): (Vec<String>, Vec<String>) = items
            .into_iter()
            .partition(|s| url::Url::parse(s.trim()).is_ok_and(|u| mdm_engine::supported(&u)));
        if links.len() > 1 {
            self.show_window(ctx);
            let found = links
                .into_iter()
                .map(|url| crate::transfer::Imported {
                    url,
                    referer: None,
                    cookies: None,
                    user_agent: None,
                    private: false,
                    ..Default::default()
                })
                .collect();
            // i18n: in Import, where the addresses came from: dragged onto the window
            let source = crate::i18n::t("Dropped links").to_owned();
            self.show_import(found, source);
        } else {
            for url in links {
                self.handle_command(
                    ipc::Command {
                        url: Some(url),
                        ..Default::default()
                    },
                    ctx,
                );
            }
        }
        for s in other {
            let torrent = mdm_torrent::Source::magnet(&s).is_some()
                || (s.to_ascii_lowercase().ends_with(".torrent")
                    && std::path::Path::new(&s).is_file());
            if torrent {
                self.handle_command(
                    ipc::Command {
                        url: Some(s),
                        ..Default::default()
                    },
                    ctx,
                );
            } else {
                // A file MDM can't download or import (a picture, a program).
                self.notice = Some((
                    crate::i18n::mark("Mister Download Manager"),
                    crate::i18n::fmt(
                        crate::i18n::t(
                            "Nothing to download in {item}: drop links, a .torrent file, or a list of addresses (.txt, .ef2).",
                        ),
                        &[("item", &crate::i18n::isolate(&s))],
                    ),
                ));
            }
        }
    }

    /// Tasks → Import: read a file and show the addresses found, to check before adding.
    fn import(&mut self, ef2: bool) {
        let title = crate::i18n::t("Import").to_owned();
        // i18n: the kind of file the Import dialog lists (text files with addresses)
        let lists = crate::i18n::t("Lists of addresses").to_owned();
        let all = crate::i18n::t("All files").to_owned();
        winui::start_pick(&self.ui_ctx, "app-import", move || {
            let dialog = crate::winui::file_dialog(&title);
            let dialog = if ef2 {
                dialog.add_filter("EF2", &["ef2"])
            } else {
                dialog.add_filter(lists, &["txt", "htm", "html", "csv"])
            };
            dialog.add_filter(all, &["*"]).pick_file()
        });
    }

    /// The system dialogs opened from the main window, answered (winui::start_pick): their
    /// work goes on.
    fn poll_picks(&mut self, ctx: &egui::Context) {
        if let Some(f) = winui::picked(ctx, "app-torrent-file") {
            self.open_torrent_dialog(Some(f.display().to_string()), ctx);
        }
        if let Some(path) = winui::picked(ctx, "app-export-ef2") {
            self.export_to(&path, true);
        }
        if let Some(path) = winui::picked(ctx, "app-export-txt") {
            self.export_to(&path, false);
        }
        if let Some(path) = winui::picked(ctx, "app-import") {
            self.import_file(&path);
        }
        if let Some(path) = winui::picked(ctx, "app-theme-file") {
            self.add_theme_file(&path, ctx);
        }
        if let Some(target) = winui::picked(ctx, "app-move-rename")
            && let Some(id) = self.move_rename_for.take()
        {
            self.start_move(id, target);
        }
        if let Some(dir) = winui::picked(ctx, "app-move-many") {
            let ids = std::mem::take(&mut self.move_many_for);
            self.move_many_to(ids, dir);
        }
    }

    /// View → Theme → Add a theme file, answered: the file copied into the themes folder and
    /// picked, as clicking it in View → Theme does; a file that isn't a theme says why.
    fn add_theme_file(&mut self, path: &std::path::Path, ctx: &egui::Context) {
        match crate::themefile::add(path) {
            Ok(id) => {
                // Listed at once (the watcher would at its next look).
                self.theme_list = crate::themefile::list();
                self.run(Cmd::Theme(crate::chrome::label(&id)), None, ctx);
            }
            Err(e) => {
                tracing::warn!("add a theme file: {e}");
                self.notice = Some((
                    // i18n: title of the message when View → Theme → Add a theme file fails
                    crate::i18n::mark("Add a theme file"),
                    crate::i18n::fmt(
                        // i18n: {error} is why, in English (a file that isn't a theme or a colour scheme, a colour missing)
                        crate::i18n::t("This file can't be used as a theme: {error}"),
                        &[("error", &e)],
                    ),
                ));
            }
        }
    }

    /// The addresses in a list file, shown in Import to tick before adding. The file is read and
    /// searched beside the window (#805): a large one, or one that isn't a list (a video, an
    /// archive picked by mistake), froze it. The status strip says "Reading …" meanwhile.
    fn import_file(&mut self, path: &std::path::Path) {
        self.importing.start(path, Some(&self.ui_ctx));
    }

    /// Each frame, never waiting: the list files read since, into Import (`import_file`).
    fn poll_imports(&mut self) {
        for (path, read) in self.importing.finished() {
            self.imported(&path, read);
        }
    }

    /// A list file read: its addresses into Import, or why there are none.
    fn imported(&mut self, path: &std::path::Path, read: crate::transfer::ListRead) {
        use crate::transfer::ListRead;
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let found = match read {
            ListRead::Found(found) => found,
            ListRead::TooBig(size) => {
                self.notice = Some((
                    crate::i18n::mark("Import"),
                    crate::i18n::fmt(
                        // i18n: Import refused a file; {size} = its size and {limit} = the largest Import reads, like "4.50 GB" and "64.00 MB"
                        crate::i18n::t(
                            "{file} is too large to be a list of addresses ({size}). Import reads text files, web pages and .ef2 exports of up to {limit}.",
                        ),
                        &[
                            ("file", &crate::i18n::isolate(&file)),
                            ("size", &model::size(size)),
                            ("limit", &model::size(crate::transfer::IMPORT_LIMIT)),
                        ],
                    ),
                ));
                return;
            }
            ListRead::Failed(e) => {
                self.notice = Some((
                    crate::i18n::mark("Import"),
                    crate::i18n::fmt(
                        crate::i18n::t("The file could not be read: {error}"),
                        &[("error", &e)],
                    ),
                ));
                return;
            }
        };
        if found.is_empty() {
            self.notice = Some((
                crate::i18n::mark("Import"),
                crate::i18n::fmt(
                    crate::i18n::t("No web addresses were found in {file}."),
                    &[("file", &file)],
                ),
            ));
            return;
        }
        self.show_import(found, file);
    }

    /// What Click'n'Load pages posted: an allowed site's links into Link grabber; the others
    /// wait for their site's answer (`cnl.ask`). The window comes up for either, once a minute
    /// for one site.
    fn poll_cnl(&mut self, ctx: &egui::Context) {
        let (links, raise) = self.cnl.take(&self.settings.cnl_sites);
        if raise {
            self.show_window(ctx);
        }
        if !links.is_empty() {
            self.show_import(links, "Click'n'Load".into());
        }
    }

    /// Links to tick before adding (the browser's page links, Import, dropped links): into Link
    /// grabber open already (several dropped lists, a list and links), else a new one; those in
    /// the list already go unticked under "Already in the list" (imported twice, they were
    /// doubled).
    fn show_import(&mut self, found: Vec<crate::transfer::Imported>, source: String) {
        let known: std::collections::HashSet<String> =
            self.items.iter().map(|i| i.url.to_string()).collect();
        let (downloads, s) = (&self.downloads, &self.settings);
        let grabber = self.grabber.get_or_insert_with(|| {
            crate::grabber::Grabber::new(
                downloads.clone(),
                s.category_dirs.clone(),
                s.grabber_hide_pages,
                s.extract_delete,
            )
        });
        grabber.add(found, source, |u| known.contains(u.as_str()));
    }

    /// Link grabber's ticked links: each package's to its folder, tagged with the package (its
    /// archives are extracted when all are in), into the main queue; started too on Start (else
    /// only queued, as IDM's import). A magnet link's torrent is added once its files are known
    /// (#709), with every file, in the package's folder when it has one. All of them are one
    /// group (#766): one summary when the last is done.
    fn add_grabbed(&mut self, start: bool, packages: Vec<crate::grabber::Picked>) {
        // Package numbers after those in the list.
        let last = self
            .items
            .iter()
            .filter_map(|i| i.package.as_ref().map(|p| p.id))
            .max()
            .unwrap_or(0);
        let mut added = Vec::new();
        let mut magnets = Vec::new();
        for (id, pk) in (last + 1..).zip(packages) {
            let tag = crate::model::PackageTag {
                id,
                name: pk.name.clone(),
                extract: pk.extract,
                delete_after: pk.delete,
                password: pk.password.clone(),
            };
            for (d, name, size) in pk.links {
                if let Some(magnet) = mdm_torrent::Source::magnet(&d.url) {
                    let dir = (!pk.folder.as_os_str().is_empty()).then(|| pk.folder.clone());
                    magnets.push((magnet, dir));
                    continue;
                }
                let Ok(url) = url::Url::parse(&d.url) else {
                    continue;
                };
                let category = Category::of(&name);
                let dir = if pk.folder.as_os_str().is_empty() {
                    self.settings.category_dir(category, &self.downloads)
                } else {
                    pk.folder.clone()
                };
                let dest = mdm_engine::naming::destination(&dir, &name);
                let context = ipc::Command {
                    referer: d.referer,
                    cookies: d.cookies,
                    user_agent: d.user_agent,
                    private: d.private,
                    ..Default::default()
                };
                let id = self.push_item(
                    url,
                    dest,
                    category,
                    String::new(),
                    size,
                    None,
                    context,
                    crate::i18n::mark("by Link grabber"),
                );
                if let Some(i) = self.items.iter_mut().find(|i| i.id == id) {
                    i.package = Some(tag.clone());
                }
                // A name the server didn't give (none looked up) may still change.
                if size.is_none() {
                    self.name_guessed(id, true);
                }
                self.queues[0].items.push(id);
                added.push(id);
            }
        }
        // The torrents join the others' group when their files come (a group of one
        // download's summary is its own Download complete).
        let group = (added.len() + magnets.len() > 1)
            .then(|| added.first().copied())
            .flatten();
        if let Some(g) = group {
            for i in self.items.iter_mut().filter(|i| added.contains(&i.id)) {
                i.group = Some(g);
            }
        }
        for (source, dir) in magnets {
            let slot = self.look_up_torrent(source);
            self.remote_torrents.push(crate::remote::Lookup {
                slot,
                category: None,
                queue: (!start).then_some(0),
                from_magnet: true,
                dir,
                group,
                tell: true,
            });
        }
        self.dirty = true;
        // Start runs the main queue (its files at once, as Options → Queue sets).
        if start && !added.is_empty() {
            self.start_queue(0);
        }
    }

    /// Link grabber's look-ups (Options → Archives → "Check links"): the next links, each
    /// with its own request (the page's referer and cookies), on the runtime. Then the web pages
    /// given on their own that the look-ups found, each read for its links (#705), which join.
    fn grabber_checks(&mut self, ctx: &egui::Context) {
        let read = self
            .grabber
            .as_mut()
            .map(|g| g.pages_read())
            .unwrap_or_default();
        for found in read {
            // The same rule as every link added: one in the download list comes unticked.
            self.show_import(found, String::new());
        }
        if !self.settings.grabber_check {
            return;
        }
        self.grabber_reads(ctx);
        let Some(g) = &self.grabber else { return };
        for (i, url, d) in g.to_check() {
            let context = ipc::Command {
                referer: d.referer.clone(),
                cookies: d.cookies.clone(),
                user_agent: d.user_agent.clone(),
                ..Default::default()
            };
            let parts = self.request_parts(&url, None, &context, false);
            let slot: std::sync::Arc<std::sync::Mutex<Option<_>>> = Default::default();
            let (out, ctx) = (slot.clone(), ctx.clone());
            self.rt.spawn(async move {
                let res = mdm_engine::probe(url, parts.headers, &parts.opts)
                    .await
                    .map_err(|e| crate::grabber::state_of(&e));
                *out.lock().unwrap() = Some(res);
                ctx.request_repaint();
            });
            if let Some(g) = &mut self.grabber {
                g.checking(i, slot);
            }
        }
    }

    /// Link grabber's web pages to read for their links (#705), each with its own request, on
    /// the runtime: the page's links come back with the page as their referer (the package is
    /// named after it), the agent and private mark the page had. Not the page's cookies, which
    /// are its own site's.
    fn grabber_reads(&mut self, ctx: &egui::Context) {
        let Some(g) = &self.grabber else { return };
        for (i, url, d) in g.to_read() {
            let context = ipc::Command {
                referer: d.referer.clone(),
                cookies: d.cookies.clone(),
                user_agent: d.user_agent.clone(),
                ..Default::default()
            };
            let parts = self.request_parts(&url, None, &context, false);
            let slot: std::sync::Arc<std::sync::Mutex<Option<_>>> = Default::default();
            let (out, ctx) = (slot.clone(), ctx.clone());
            self.rt.spawn(async move {
                let found = match mdm_engine::page_links(url, parts.headers, &parts.opts).await {
                    Ok((at, links)) => {
                        tracing::info!("link grabber: {} links on a page", links.len());
                        links
                            .into_iter()
                            .map(|u| crate::transfer::Imported {
                                url: u.into(),
                                referer: Some(at.to_string()),
                                user_agent: d.user_agent.clone(),
                                private: d.private,
                                ..Default::default()
                            })
                            .collect()
                    }
                    Err(e) => {
                        tracing::info!("link grabber: a page couldn't be read: {e}");
                        Vec::new()
                    }
                };
                *out.lock().unwrap() = Some(found);
                ctx.request_repaint();
            });
            if let Some(g) = &mut self.grabber {
                g.reading(i, slot);
            }
        }
    }

    /// Can this download be fetched again from the start? A file download that isn't running
    /// (torrents re-check their data instead; a video's file is yt-dlp's to replace).
    fn can_redownload(&self, id: u64) -> bool {
        self.items
            .iter()
            .find(|i| i.id == id)
            .is_some_and(|i| i.live.is_none() && i.torrent.is_none() && i.video.is_none())
    }

    /// Download again from the start: the partial state goes; a finished file is replaced when
    /// the new download completes.
    fn redownload(&mut self, id: u64) {
        if !self.can_redownload(id) {
            return;
        }
        let Some(item) = self.item(id) else { return };
        let _ = std::fs::remove_file(state_path(&item.dest));
        let _ = std::fs::remove_file(mdm_engine::naming::part_path(&item.dest));
        item.downloaded = 0;
        item.status = Status::Stopped;
        item.error = None;
        item.note(crate::i18n::t("Downloading again from the start"));
        self.dirty = true;
        self.start(id, true);
    }

    /// Move/Rename: a Save As box at the file's place; a finished file is moved there, one
    /// that hasn't started gets the new name and folder.
    fn move_rename(&mut self, id: u64) {
        let Some(item) = self.items.iter().find(|i| i.id == id) else {
            return;
        };
        if !can_move(item) {
            // Said, not silently ignored (F2, Ctrl+M on a download that can't move).
            self.notice = Some(cant_move_notice());
            return;
        }
        let title = crate::i18n::t("Move/Rename").to_owned();
        let from = item
            .dest
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let name = item.file_name.clone();
        self.move_rename_for = Some(id);
        winui::start_pick(&self.ui_ctx, "app-move-rename", move || {
            crate::winui::file_dialog(&title)
                .set_directory(from)
                .set_file_name(name)
                .save_file()
        });
    }

    /// Move/Rename with several downloads selected (#333): their folder is asked for, not a name
    /// each.
    fn move_many(&mut self, ids: Vec<u64>) {
        let can = ids
            .iter()
            .filter_map(|id| self.items.iter().find(|i| i.id == *id))
            .find(|i| can_move(i));
        let Some(first) = can else {
            self.notice = Some(cant_move_notice());
            return;
        };
        let title = crate::i18n::t("Move/Rename").to_owned();
        let from = first
            .dest
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        self.move_many_for = ids;
        winui::start_pick(&self.ui_ctx, "app-move-many", move || {
            // Off-screen test runs have no folder dialog to answer.
            if let Some(dir) = std::env::var_os("MDM_TEST_MOVE_TO") {
                return Some(PathBuf::from(dir));
            }
            crate::winui::file_dialog(&title)
                .set_directory(from)
                .pick_folder()
        });
    }

    /// The several downloads' new folder: each that can go goes there, under its own name.
    fn move_many_to(&mut self, ids: Vec<u64>, dir: PathBuf) {
        let mut taken = std::collections::HashSet::new();
        let mut stay = false;
        for id in ids {
            let Some(item) = self.items.iter().find(|i| i.id == id) else {
                continue;
            };
            if !can_move(item) {
                stay = true;
                continue;
            }
            if item.dest.parent() == Some(dir.as_path()) {
                continue;
            }
            let target = place_in(&dir, &item.file_name, &mut taken);
            self.start_move(id, target);
        }
        self.dirty = true;
        if stay {
            self.notice = Some(cant_move_notice());
        }
    }

    /// Delete a queue of the user's (not the two built-in ones); its downloads stay in the list.
    fn delete_queue(&mut self, q: usize) {
        if q < 2 || q >= self.queues.len() {
            return;
        }
        if self.queues[q].running {
            self.stop_queue(q);
        }
        self.queues.remove(q);
        // The tree's selection follows the queues after it, and leaves the deleted one.
        self.view = match self.view {
            View::Queue(v) if v == q => View::All,
            View::Queue(v) if v > q => View::Queue(v - 1),
            v => v,
        };
        self.dirty = true;
    }

    /// IDM looks for a new version once a week; so do we (off: the switch in Check for updates). Found: the
    /// update window says so; otherwise nothing shows.
    /// The automatic update check: a few seconds after every start, then once a day while MDM
    /// runs (it was at most once a week). A new version's window comes to the front, as apps
    /// announce theirs. Not in off-screen test runs (their windows stay as asked).
    fn auto_update_check(&mut self, ctx: &egui::Context) {
        let now = chrono::Utc::now().timestamp();
        let first = self.update_checked_this_run.is_none()
            && self.started.elapsed() >= crate::help::CHECK_AFTER_START;
        let again = self
            .update_checked_this_run
            .is_some_and(|t| now - t >= crate::help::CHECK_EVERY);
        if self.auto_update.is_none()
            && self.settings.update_check
            // Off-screen test runs check only when told where (MDM_UPDATE_FEED: their own feed).
            && (!crate::offscreen() || std::env::var_os("MDM_UPDATE_FEED").is_some())
            && (first || again)
        {
            self.update_checked_this_run = Some(now);
            self.settings.update_checked = now;
            self.dirty = true;
            self.auto_update = Some(crate::help::check(&self.rt, ctx));
        }
        if self.updates.is_some() {
            if self.raise_updates && crate::dialogs::focus_when_shown(ctx, "updates") {
                self.raise_updates = false;
            }
        } else {
            self.raise_updates = false;
        }
        if self.options.is_some() {
            if self.raise_options && crate::dialogs::focus_when_shown(ctx, "options") {
                self.raise_options = false;
            }
        } else {
            self.raise_options = false;
        }
        // Until the check at start is due.
        if self.update_checked_this_run.is_none() && self.settings.update_check {
            ctx.request_repaint_after(crate::help::CHECK_AFTER_START);
        }
        let done = self
            .auto_update
            .as_ref()
            .and_then(|s| s.lock().unwrap().take());
        if let Some(res) = done {
            self.auto_update = None;
            if let (Ok(crate::help::Found::Newer(rel)), None) = (res, &self.updates) {
                tracing::info!("update available: {}", rel.version);
                self.updates = Some(crate::help::Updates::found(rel));
                self.raise_updates = true;
            }
        }
    }

    fn help_dialogs(&mut self, ctx: &egui::Context) {
        if let Some(d) = &mut self.lang_picker {
            let first = !self.settings.language_asked;
            match d.show(ctx) {
                Outcome::Open => {}
                Outcome::Done(code) => {
                    // On the first start, keeping the preselected Windows language follows
                    // Windows from then on.
                    let code = code.filter(|c| !(first && *c == crate::i18n::system_code()));
                    self.lang_picker = None;
                    self.run(Cmd::Language(code), None, ctx);
                    crate::theme::interface_fonts(ctx);
                }
                Outcome::Closed => {
                    self.lang_picker = None;
                    crate::theme::interface_fonts(ctx);
                    // Cancelled on the first start: Windows' language, and don't ask again.
                    if first {
                        self.settings.language_asked = true;
                        self.dirty = true;
                    }
                }
            }
        }
        // Once the language is settled: the video helpers, offered once (helpers.rs).
        if self.lang_picker.is_none()
            && self.settings.language_asked
            && !self.settings.helpers_asked
        {
            self.settings.helpers_asked = true;
            self.dirty = true;
            if crate::helpers::worth_asking() {
                self.helpers = Some(crate::helpers::Helpers::new());
            }
        }
        if let Some(d) = &mut self.helpers
            && !matches!(d.show(ctx, &self.rt), Outcome::Open)
        {
            self.helpers = None;
        }
        if let Some(q) = &mut self.shown_existing
            && !matches!(q.show(ctx), Outcome::Open)
        {
            self.shown_existing = None;
        }
        // Then the browser extension, once per start until a browser has it (extension.rs).
        if self.lang_picker.is_none()
            && self.helpers.is_none()
            && self.settings.language_asked
            && !self.extension_weighed
        {
            self.extension_weighed = true;
            if crate::extension::worth_asking(&self.settings) {
                self.extension = Some(crate::extension::Extension::new());
            }
        }
        if let Some(d) = &mut self.extension {
            match d.show(ctx) {
                Outcome::Open => {}
                Outcome::Done(()) => {
                    self.extension = None;
                    self.settings.extension_never_ask = true;
                    self.dirty = true;
                }
                Outcome::Closed => self.extension = None,
            }
        }
        if let Some(d) = &mut self.about
            && !matches!(d.show(ctx), Outcome::Open)
        {
            self.about = None;
        }
        let mut weekly = self.settings.update_check;
        let mut close_for_update = false;
        if let Some(d) = &mut self.updates {
            match d.show(ctx, &self.rt, &mut weekly) {
                Outcome::Open => {}
                // Update now: the new copy is going in place; MDM saves and closes for it.
                Outcome::Done(true) => {
                    self.updates = None;
                    close_for_update = true;
                }
                _ => self.updates = None,
            }
        }
        if close_for_update {
            self.run(Cmd::Exit, None, ctx);
        }
        if weekly != self.settings.update_check {
            self.settings.update_check = weekly;
            self.dirty = true;
        }
        if let Some(d) = &mut self.shortcuts
            && !matches!(d.show(ctx), Outcome::Open)
        {
            self.shortcuts = None;
        }
        if let Some(d) = &mut self.tell_friend
            && !matches!(d.show(ctx), Outcome::Open)
        {
            self.tell_friend = None;
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        self.help_dialogs(ctx);
        // The next question in turn; one about a download that has left the list meanwhile is
        // checked afresh instead (it may ask again, about another).
        while self.duplicate.is_none()
            && let Some((dup, pending, d)) = self.duplicates_waiting.pop_front()
        {
            if self.items.iter().any(|i| i.id == dup) {
                self.duplicate = Some((dup, pending, d));
            } else {
                self.duplicate_again(pending, ctx);
            }
        }
        if let Some((_, _, d)) = &mut self.duplicate {
            match d.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.duplicate = None,
                Outcome::Done(choice) => {
                    if let Some((dup, pending, _)) = self.duplicate.take() {
                        self.resolve_duplicate(dup, pending, choice, ctx);
                    }
                }
            }
        }
        if let Some((_, d)) = &mut self.queue_pick {
            match d.show(ctx) {
                Outcome::Open => {}
                // Cancel: the downloads stay in the list, in no queue.
                Outcome::Closed => self.queue_pick = None,
                Outcome::Done((q, start)) => {
                    if let Some((ids, _)) = self.queue_pick.take()
                        && q < self.queues.len()
                    {
                        for id in ids {
                            if !self.queues[q].items.contains(&id) {
                                self.queues[q].items.push(id);
                            }
                        }
                        if start {
                            self.start_queue(q);
                        }
                        self.dirty = true;
                    }
                }
            }
        }
        self.grabber_checks(ctx);
        if let Some(d) = &mut self.grabber {
            match d.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.grabber = None,
                Outcome::Done((start, packages)) => {
                    self.grabber = None;
                    self.add_grabbed(start, packages);
                }
            }
        }
        // Click'n'Load's question about a site; closing it is "Not now".
        let answer = self.cnl.ask.as_mut().and_then(|d| match d.show(ctx) {
            Outcome::Open => None,
            Outcome::Closed => Some(crate::cnl::Answer::NotNow),
            Outcome::Done(a) => Some(a),
        });
        if let Some(answer) = answer {
            let (links, keep) = self.cnl.answer(answer);
            if let Some((site, allowed)) = keep {
                self.settings.cnl_sites.retain(|(s, _)| *s != site);
                self.settings.cnl_sites.push((site, allowed));
                self.dirty = true;
            }
            if !links.is_empty() {
                self.show_import(links, "Click'n'Load".into());
            }
        }
        if let Some(d) = &mut self.extracts.needs_7zip
            && !matches!(d.show(ctx), Outcome::Open)
        {
            self.extracts.needs_7zip = None;
        }
        if let Some(d) = &mut self.extracts.ask {
            match d.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.extracts.answer(None),
                Outcome::Done(pw) => self.extracts.answer(pw),
            }
        }
        if let Some(d) = &mut self.category_dlg {
            match d.show(ctx, &crate::categories::names()) {
                Outcome::Open => {}
                Outcome::Closed => self.category_dlg = None,
                Outcome::Done(e) => {
                    self.category_dlg = None;
                    let made = crate::categories::apply(&mut self.settings, e, &self.downloads);
                    // Made with Download File Info's "+": that download goes in it.
                    if std::mem::take(&mut self.category_for_file_info)
                        && let Some(fi) = &mut self.file_info
                    {
                        fi.set_category(made);
                    }
                    self.dirty = true;
                }
            }
        }
        if let Some(d) = &mut self.confirm_category_delete {
            match d.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.confirm_category_delete = None,
                Outcome::Done(remove) => {
                    let cat = d.category;
                    self.confirm_category_delete = None;
                    // Off the list first (not their files); the running ones can't go, so they
                    // move to General with the rest.
                    if remove {
                        let ids: Vec<u64> = self
                            .items
                            .iter()
                            .filter(|i| i.category == cat && i.live.is_none())
                            .map(|i| i.id)
                            .collect();
                        self.remove_downloads(&ids, false);
                    }
                    crate::categories::delete(&mut self.settings, cat, &mut self.items);
                    let gone = Some(cat);
                    if matches!(self.view, View::Cat(c) if Some(c) == gone)
                        || matches!(self.view, View::Unfinished(c) | View::Finished(c) if c == gone)
                    {
                        self.view = View::All;
                    }
                    self.dirty = true;
                }
            }
        }
        if let Some((id, d)) = &mut self.confirm_redownload {
            match d.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.confirm_redownload = None,
                Outcome::Done(()) => {
                    let id = *id;
                    self.confirm_redownload = None;
                    self.redownload(id);
                }
            }
        }
        if let Some((q, d)) = &mut self.confirm_queue_delete {
            match d.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.confirm_queue_delete = None,
                Outcome::Done(()) => {
                    let q = *q;
                    self.confirm_queue_delete = None;
                    self.delete_queue(q);
                }
            }
        }
        if let Some(d) = &mut self.add_url {
            let out = d.show(ctx, |url| self.settings.site_login(url));
            // Every address that goes on is remembered for the Address drop list.
            if let Outcome::Done(new) = &out {
                crate::settings::remember(&mut self.settings.url_history, new.url.as_str());
                self.dirty = true;
            }
            match out {
                Outcome::Open => {}
                Outcome::Closed => self.add_url = None,
                Outcome::Done(new) if new.url.scheme() == "magnet" => {
                    self.add_url = None;
                    self.open_torrent_dialog(Some(new.url.to_string()), ctx);
                }
                // A .torrent's address (#39): Add torrent fetches it and shows what it holds.
                Outcome::Done(new) if crate::torrents::is_torrent_url(&new.url, None, None) => {
                    self.add_url = None;
                    let parts = self.request_parts(
                        &new.url,
                        new.login.as_ref(),
                        &Default::default(),
                        false,
                    );
                    let remote = crate::torrents::Remote {
                        url: new.url.clone(),
                        parts,
                    };
                    self.open_torrent_dialog_with(Some(new.url.to_string()), Some(remote), ctx);
                }
                // A video's page, not a file (YouTube, Vimeo…): the video window looks it up.
                Outcome::Done(new) if mdm_tools::pages::is_video_page(&new.url) => {
                    self.add_url = None;
                    self.video_dlg =
                        Some(self.video_dialog(Some((new.url.as_str(), Default::default()))));
                }
                // An image gallery's page (Wikimedia Commons, Flickr…): gallery-dl (#58).
                Outcome::Done(new) if mdm_tools::pages::is_gallery_page(&new.url) => {
                    self.add_url = None;
                    let dir = self
                        .settings
                        .category_dir(Category::General, &self.downloads);
                    self.gallery_dlg =
                        Some(crate::gallerydlg::GalleryDialog::new(dir, new.url.as_str()));
                }
                Outcome::Done(new) => {
                    self.add_url = None;
                    let url = new.url.clone();
                    if self.duplicate_gate(&url, || {
                        Pending::Url(dialogs::NewUrl {
                            url: new.url.clone(),
                            login: new.login.clone(),
                        })
                    }) {
                        self.open_file_info(new, Default::default(), ctx);
                    }
                }
            }
        }
        if let Some(d) = &mut self.confirm_delete {
            match d.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.confirm_delete = None,
                Outcome::Done((files, dont_show)) => {
                    let ids = std::mem::take(&mut d.ids);
                    let remember = d.remember;
                    self.confirm_delete = None;
                    self.remove_downloads(&ids, files);
                    if remember {
                        self.settings.delete_files_too = files;
                    }
                    if dont_show {
                        self.settings.confirm_delete = false;
                    }
                    self.dirty = true;
                }
            }
        }
        if let Some(n) = &mut self.limit_notice
            && !matches!(n.show(ctx), Outcome::Open)
        {
            self.limit_notice = None;
        }
        if std::mem::take(&mut self.exit_requested) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Some(t) = &mut self.turn_off {
            match t.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.turn_off = None,
                Outcome::Done(()) => {
                    t.execute();
                    self.turn_off = None;
                }
            }
        }
        let mut changes = Vec::new();
        let mut files_requested = Vec::new();
        let items = &self.items;
        let icons = &mut self.icons;
        self.properties.retain_mut(|pr| {
            let Some(item) = items.iter().find(|i| i.id == pr.id) else {
                return false;
            };
            match pr.show(ctx, icons, item) {
                Outcome::Open => {
                    if pr.take_files_requested() {
                        files_requested.push(pr.id);
                    }
                    true
                }
                Outcome::Closed => false,
                Outcome::Done(c) => {
                    changes.push(c);
                    false
                }
            }
        });
        for c in changes {
            self.apply_properties(c);
        }
        for id in files_requested {
            if let Some(item) = self.items.iter().find(|i| i.id == id) {
                self.torrent_files =
                    crate::torrentfiles::TorrentFiles::new(item, &self.torrent_host);
            }
        }
        let mut hashes = Vec::new();
        self.checksums.retain_mut(|c| match c.show(ctx) {
            Outcome::Open => true,
            Outcome::Closed => false,
            Outcome::Done(h) => {
                hashes.push((c.id, h));
                false
            }
        });
        for (id, h) in hashes {
            if let Some(item) = self.item(id) {
                item.note(crate::i18n::fmt(
                    // i18n: event log entry: the user gave the checksum the file must have
                    crate::i18n::t("Expected checksum set ({algorithm})"),
                    &[("algorithm", h.algo.name())],
                ));
                item.checksum = Some(h);
                self.dirty = true;
            }
        }
        if let Some(f) = &mut self.find {
            match f.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.find = None,
                Outcome::Done(()) => {
                    let q = f.query.clone();
                    self.find = None;
                    self.find_next(&q);
                    self.last_find = Some(q);
                }
            }
        }
        if let Some((title, text)) = &self.notice
            && !matches!(dialogs::message(ctx, "notice", title, text), Outcome::Open)
        {
            self.notice = None;
        }
        if !self.torrents_started {
            self.torrents_started = true;
            // The handlers follow the exe (a portable copy may have moved).
            if self.settings.torrent_associations {
                #[cfg(windows)]
                crate::win32::set_torrent_handlers(true);
                #[cfg(unix)]
                crate::unix::set_torrent_handlers(true);
            }
            if self.settings.torrent_keep_sharing {
                self.torrent_host.set_sharing(
                    &self.rt,
                    self.session_hashes(),
                    self.complete_torrents(),
                    true,
                );
            }
        }
        if let Some(t) = &mut self.torrent_settings {
            match t.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.torrent_settings = None,
                Outcome::Done(crate::torrentdlg::TorrentChoice {
                    share,
                    assoc,
                    net,
                    upload,
                    seeding,
                }) => {
                    self.torrent_settings = None;
                    // The network: the next torrent session takes it (torrent_tick).
                    net.apply(&mut self.settings);
                    // The upload limit: at once (torrent_tick sets it on the session).
                    (
                        self.settings.torrent_upload_on,
                        self.settings.torrent_upload_kb,
                    ) = upload;
                    // Seeding limits: the tick stops those over them; raised, the finished
                    // torrents now under them are shared again (when sharing stays on).
                    let s = &mut self.settings;
                    let before = s.sharing_limits();
                    s.torrent_stop_ratio_on = seeding.ratio_on;
                    s.torrent_stop_ratio = seeding.ratio;
                    s.torrent_stop_minutes_on = seeding.minutes_on;
                    s.torrent_stop_minutes = seeding.minutes;
                    if share
                        && share == self.settings.torrent_keep_sharing
                        && before != self.settings.sharing_limits()
                    {
                        self.torrent_host.set_sharing(
                            &self.rt,
                            self.session_hashes(),
                            self.complete_torrents(),
                            true,
                        );
                    }
                    if share != self.settings.torrent_keep_sharing {
                        self.settings.torrent_keep_sharing = share;
                        self.torrent_host.set_sharing(
                            &self.rt,
                            self.session_hashes(),
                            self.complete_torrents(),
                            share,
                        );
                    }
                    if assoc != self.settings.torrent_associations {
                        self.settings.torrent_associations = assoc;
                        #[cfg(windows)]
                        crate::win32::set_torrent_handlers(assoc);
                        #[cfg(unix)]
                        crate::unix::set_torrent_handlers(assoc);
                    }
                    self.dirty = true;
                }
            }
        }
        if let Some(w) = &mut self.rss_dlg {
            match w.show(ctx, &mut self.rss, &self.rss_busy) {
                Outcome::Open => {}
                Outcome::Closed => {
                    self.rss_dlg = None;
                    self.save_rss();
                }
                Outcome::Done(crate::rssdlg::Act::Saved) => self.save_rss(),
                Outcome::Done(crate::rssdlg::Act::Refresh(ids)) => {
                    self.save_rss();
                    let ids = if ids.is_empty() {
                        self.rss.feeds.iter().map(|f| f.id).collect()
                    } else {
                        ids
                    };
                    for id in ids {
                        self.rss_fetch(id);
                    }
                }
                // By hand: Add torrent, as for any address (a .torrent's is fetched).
                Outcome::Done(crate::rssdlg::Act::Download(link)) => {
                    let remote = url::Url::parse(&link)
                        .ok()
                        .filter(|u| matches!(u.scheme(), "http" | "https"))
                        .map(|url| crate::torrents::Remote {
                            parts: crate::app::RequestParts {
                                headers: Default::default(),
                                opts: crate::settings::own_net(),
                            },
                            url,
                        });
                    self.open_torrent_dialog_with(Some(link), remote, ctx);
                }
            }
        }
        if let Some(d) = &mut self.add_trackers {
            match d.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.add_trackers = None,
                Outcome::Done(list) => {
                    let id = d.id;
                    self.add_trackers = None;
                    self.torrent_act(id, crate::torrents::Act::AddTrackers(list));
                }
            }
        }
        let mut torrent_files_selection = None;
        if let Some(d) = &mut self.torrent_files {
            match d.show(ctx) {
                Outcome::Open => {
                    torrent_files_selection = d.take_selection().map(|selection| (d.id, selection));
                }
                Outcome::Closed => self.torrent_files = None,
                Outcome::Done(_) => self.torrent_files = None,
            }
        }
        if let Some((id, (files, all, size))) = torrent_files_selection {
            self.choose_files(id, files, all, size);
        }
        // The next torrent handed over while Add torrent was open (#507), a frame after it
        // closed: that frame without it closes its window, and the next one opens as a window
        // of its own (opened in the same frame, the closed window would only change its
        // contents). One that goes to the debrid account opens nothing, and the one after it
        // is next.
        if self.torrent_dlg.is_none() && !self.torrent_waiting.is_empty() {
            if std::mem::take(&mut self.torrent_closed) {
                ctx.request_repaint();
            } else {
                while self.torrent_dlg.is_none()
                    && let Some((source, remote)) = self.torrent_waiting.pop_front()
                {
                    self.open_torrent_dialog_with(Some(source), remote, ctx);
                }
            }
        }
        if let Some(t) = &mut self.torrent_dlg {
            let closed = match t.show(ctx, &self.rt) {
                Outcome::Open => false,
                Outcome::Closed => true,
                Outcome::Done(add) => {
                    if self.settings.change_folder_on_last
                        && self
                            .settings
                            .keep_category_dir(add.category, &add.dir, &self.downloads)
                    {
                        self.dirty = true;
                    }
                    self.torrent_dlg = None;
                    self.add_torrent(add);
                    true
                }
            };
            if closed {
                self.torrent_dlg = None;
                self.torrent_closed = true;
                ctx.request_repaint();
            }
        }
        if let Some(g) = &mut self.gallery_dlg {
            match g.show(ctx, &self.rt) {
                Outcome::Open => {}
                Outcome::Closed => self.gallery_dlg = None,
                Outcome::Done(add) => {
                    self.gallery_dlg = None;
                    self.add_gallery(add);
                }
            }
        }
        if let Some(v) = &mut self.video_dlg {
            match v.show(ctx, &self.rt) {
                Outcome::Open => {}
                Outcome::Closed => self.video_dlg = None,
                Outcome::Done(out) => {
                    // Options' "Change folder for this category on last selected", which
                    // Download File Info follows through its box (#475): Music's for a
                    // sound-only choice (#611).
                    if self.settings.change_folder_on_last
                        && self.settings.keep_category_dir(
                            v.category(),
                            &v.save_folder(),
                            &self.downloads,
                        )
                    {
                        self.dirty = true;
                    }
                    // Embed chapters is kept for the next video (#827), and the videos added
                    // now start with it.
                    if self.settings.video_chapters != v.chapters() {
                        self.settings.video_chapters = v.chapters();
                        crate::settings::share_video_settings(&self.settings);
                        self.dirty = true;
                    }
                    self.video_dlg = None;
                    match out {
                        crate::videodlg::VideoOut::One(add) => {
                            self.add_video_checked(add, false);
                        }
                        crate::videodlg::VideoOut::Many(add) => self.add_playlist(add),
                    }
                }
            }
        }
        if let Some(got) = self.batch_clip.as_mut().and_then(|p| p.answer()) {
            self.batch_clip = None;
            self.batch_from_clipboard(got.as_deref(), ctx);
        }
        if let Some(b) = &mut self.batch {
            match b.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.batch = None,
                Outcome::Done(added) => {
                    self.batch = None;
                    self.add_batch(added);
                }
            }
        }
        // Options, Downloads, Resume now (#741), clicked in the frame before.
        if self
            .options
            .as_mut()
            .is_some_and(|d| std::mem::take(&mut d.resume_snooze))
        {
            self.resume_snooze();
        }
        if let Some(d) = &mut self.options {
            d.snoozed_until = self.snooze.until(unix_now());
            match d.show(ctx, &self.downloads) {
                Outcome::Open => {}
                Outcome::Closed => {
                    // Cancel: categories the dialog added, changed or restored are undone, and
                    // the remote control listens as before Pair a device... moved it.
                    self.settings.publish_categories();
                    if let Some(r) = &self.remote {
                        r.apply(crate::remote::config(&self.settings));
                    }
                    self.options = None;
                }
                Outcome::Done(new) => {
                    let new = Settings::merge_changes(&self.settings, &d.from, &new);
                    self.options = None;
                    self.apply_options(new);
                }
            }
        }
        if let Some(mut s) = self.scheduler.take() {
            let mut actions = Vec::new();
            let open = s.show(
                ctx,
                &mut self.icons,
                &self.items,
                &self.queues,
                self.wake.error.as_deref(),
                &mut actions,
            );
            for a in actions {
                self.scheduler_action(a);
            }
            if open {
                self.scheduler = Some(s);
            }
        }
        if let Some(d) = &mut self.speed_settings {
            match d.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.speed_settings = None,
                Outcome::Done(c) => {
                    self.settings.speed_limit_kb = c.kb;
                    self.settings.limiter_on_startup = c.on_startup;
                    self.settings.global_limit_on = c.global_on;
                    self.settings.global_limit_kb = c.global_kb;
                    self.settings.global_only_active = c.only_active;
                    self.speed_settings = None;
                    self.apply_speed_limit();
                    self.dirty = true;
                }
            }
        }
        if let Some(mut fi) = self.file_info.take() {
            self.start_early(&mut fi);
            // The downloads waiting their turn behind this one (a browser selection's arrive
            // one by one): Cancel asks whether they are cancelled too (#394).
            fi.waiting = self.file_info_waiting.len();
            let out = fi.show(ctx, &mut self.icons);
            if std::mem::take(&mut fi.new_category) {
                self.category_for_file_info = true;
                self.run(Cmd::AddCategory, None, ctx);
            }
            // The folder a download goes to is remembered for the Save As drop list.
            if matches!(out, Outcome::Done(_))
                && let Some(dir) = fi.dest().parent()
            {
                crate::settings::remember(
                    &mut self.settings.folder_history,
                    &dir.display().to_string(),
                );
                self.dirty = true;
            }
            // The address was edited after the early start, or "Download through" changed (#44):
            // that download is the old one's. It goes; the new one starts early in its turn
            // (start_early).
            if let Some(id) = fi.early
                && self
                    .item(id)
                    .is_some_and(|i| i.url != fi.url || i.direct != fi.direct)
            {
                self.stop(id);
                self.after_stop.push((id, None));
                fi.early = None;
            }
            match out {
                Outcome::Open => self.file_info = Some(fi),
                Outcome::Closed => {
                    if fi.cancel_all {
                        // #394: "Yes" to "Cancel them too?": the browser's links waiting
                        // behind this one are cancelled with it (the browser gave its own copy
                        // up, and Download File Info was their step).
                        self.file_info_waiting.clear();
                    }
                    if let Some(id) = fi.early {
                        self.stop(id);
                        self.after_stop.push((id, None));
                    }
                }
                Outcome::Done(start) => match fi.early {
                    Some(id) => self.adopt_early(&fi, id, start),
                    None => self.add(&mut fi, start),
                },
            }
        }
        // The next download that waited for Download File Info.
        if self.file_info.is_none()
            && let Some((new, context)) = self.file_info_waiting.pop_front()
        {
            self.open_file_info(new, context, ctx);
        }
        self.finish_stopped();
        let mut actions = Vec::new();
        // A progress window can be hidden in the tray only when there is one to bring it back.
        // Off-screen test copies have no tray icon: their windows are drawn as if they had one
        // (the captures and the accessibility checks see the hide button).
        let tray = (self.tray.is_some() || crate::offscreen()) && crate::tray::available();
        let rename_stamp = if !self.progress.is_empty() {
            self.progress_rename_guard_stamp()
        } else {
            0
        };
        for p in &mut self.progress {
            p.keep_sharing = self.settings.torrent_keep_sharing;
            if p.host.is_none() {
                p.host = Some(self.torrent_host.clone());
            }
            p.tray = tray;
            p.minimize_to_tray = self.settings.minimize_to_tray;
            p.main_shown = !self.hidden;
            // Hidden: not shown (its window is gone) until the tray or the list brings it back.
            if p.hidden && tray {
                continue;
            }
            p.hidden = false;
            if let Some(item) = self.items.iter().find(|i| i.id == p.id) {
                if item.status == Status::Complete {
                    p.rename_disabled_reason = None;
                    p.rename_guard_stamp = Some(rename_stamp);
                } else if p.rename_guard_stamp != Some(rename_stamp) {
                    // Looked at off the window's thread (#743), with copies of the rows' places.
                    if p.rename_guard
                        .as_ref()
                        .is_none_or(|(s, _)| *s != rename_stamp)
                    {
                        let asked = crate::launch::Asked::default();
                        let (item, items) = (
                            item.place_copy(),
                            self.items.iter().map(Item::place_copy).collect::<Vec<_>>(),
                        );
                        let (downloads, settings, rss) = (
                            self.downloads.clone(),
                            self.settings.clone(),
                            self.rss.clone(),
                        );
                        let _ = asked.get(
                            &rename_stamp,
                            Duration::MAX,
                            Duration::ZERO,
                            Some(ctx),
                            move |_| {
                                deferred_rename_disabled_reason_with(
                                    &item, &downloads, &settings, &rss, &items,
                                )
                            },
                        );
                        p.rename_guard = Some((rename_stamp, asked));
                    }
                    if let Some(reason) = p.rename_guard.as_ref().and_then(|(s, a)| a.answer(s)) {
                        p.rename_disabled_reason = reason;
                        p.rename_guard_stamp = Some(rename_stamp);
                        p.rename_guard = None;
                    }
                }
                actions.push((p.id, p.show(ctx, item, item.snap.as_ref())));
            }
        }
        for (id, a) in actions {
            match a {
                progress::Action::None => {}
                progress::Action::Pause => self.stop_by_hand(id),
                progress::Action::Resume => self.start(id, true),
                progress::Action::Cancel => {
                    self.stop(id);
                    self.progress.retain(|p| p.id != id);
                }
                progress::Action::Rename(name) => {
                    if let Some(item) = self.item(id) {
                        item.rename_on_complete = name;
                        self.dirty = true;
                    }
                }
                progress::Action::Close => {
                    let confirmed = self
                        .progress
                        .iter_mut()
                        .find(|p| p.id == id)
                        .and_then(Progress::take_confirmed_rename);
                    if let Some(name) = confirmed
                        && let Some(item) = self.item(id)
                    {
                        item.rename_on_complete = name;
                        self.dirty = true;
                    }
                    self.progress.retain(|p| p.id != id);
                }
                progress::Action::Hide => {
                    let mut confirmed = None;
                    if let Some(p) = self.progress.iter_mut().find(|p| p.id == id) {
                        confirmed = p.take_confirmed_rename();
                        p.hidden = true;
                    }
                    if let Some(name) = confirmed
                        && let Some(item) = self.item(id)
                    {
                        item.rename_on_complete = name;
                        self.dirty = true;
                    }
                }
                progress::Action::Limit(limit, remember) => {
                    let mut helper = false;
                    if let Some(item) = self.item(id) {
                        if let Some(l) = item.live.as_ref() {
                            l.dl.set_speed_limit(limit);
                            helper = l.dl.is_helper();
                        }
                        // Remembered, it applies again after a stop or a restart.
                        item.own_limit = if remember { limit } else { None };
                        self.dirty = true;
                    }
                    // A video or a gallery starts again with it (#708).
                    if helper {
                        self.helper_own.insert(id, limit);
                        self.reapply_helper_limits();
                    }
                }
                progress::Action::UploadLimit(limit) => {
                    if let Some(item) = self.item(id)
                        && let Some(spec) = item.torrent.as_mut()
                    {
                        // Kept with the torrent; a Run sets it now, a torrent sharing without
                        // one gets it at the next torrent_tick (torrent_accounts).
                        spec.upload_limit = limit;
                        if let Some(model::Job::Torrent(run)) = item.live.as_ref().map(|l| &l.dl) {
                            run.set_upload_limit(limit);
                        }
                        self.dirty = true;
                    }
                }
                progress::Action::Choose { files, all, size } => {
                    self.choose_files(id, files, all, size)
                }
            }
        }
        let mut keep = Vec::new();
        for mut c in std::mem::take(&mut self.complete) {
            match c.show(ctx) {
                Outcome::Open => keep.push(c),
                Outcome::Done(()) | Outcome::Closed => {
                    if c.dont_show {
                        self.settings.show_complete = false;
                        self.dirty = true;
                    }
                    if let Some(choice) = c.snooze {
                        self.start_snooze(choice);
                    }
                }
            }
        }
        self.complete = keep;
        // Groups' summaries (#766).
        let mut keep = Vec::new();
        for mut g in std::mem::take(&mut self.summaries) {
            let outcome = g.show(ctx);
            if matches!(outcome, Outcome::Open) {
                keep.push(g);
                continue;
            }
            if g.dont_show {
                self.settings.show_complete = false;
                self.dirty = true;
            }
            if let Some(choice) = g.snooze {
                self.start_snooze(choice);
            }
            if let Outcome::Done(dialogs::GroupAction::ShowInList) = outcome {
                self.show_in_list(&g.ids, ctx);
            }
        }
        self.summaries = keep;
    }
}

/// The wall clock, in unix seconds (a snooze's end, #741).
fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The finished downloads whose progress windows close at `now` (then Download complete opens):
/// of the ones just `finished`, those without a window on screen (`shown`), at once. A window on
/// screen first shows its download whole for a moment ([`progress::SHOWN_WHOLE`]; its bar glides
/// a little behind the figures and would close short of the end): kept in `finishing` with when
/// it finished, it closes once that moment is over, unless the download was started again
/// meanwhile (no longer `complete`): its window stays for the new run.
fn finished_windows(
    finished: Vec<u64>,
    shown: impl Fn(u64) -> bool,
    complete: impl Fn(u64) -> bool,
    finishing: &mut Vec<(u64, Instant)>,
    now: Instant,
) -> Vec<u64> {
    let mut close = Vec::new();
    for id in finished {
        if shown(id) {
            finishing.push((id, now));
        } else {
            close.push(id);
        }
    }
    finishing.retain(|&(id, at)| {
        let wait = now.duration_since(at) < progress::SHOWN_WHOLE;
        if !wait && complete(id) {
            close.push(id);
        }
        wait
    });
    close
}

impl eframe::App for App {
    /// What comes in from outside (a second launch, the browser's native host, the automation
    /// API) is handled here, which runs at every frame: `ui` doesn't run while the main window
    /// is minimized, and a magnet or a download handed over then waited until it was restored
    /// (#162). A command that opens a window gets the UI pass that creates it, minimized or not
    /// (no window is raised or focused for it beyond what the command itself does).
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        crate::renderer::present_unpaced(frame);
        // MDM's own work: not counted as the graphics API's time to its first frame (#280).
        crate::renderer::own_work_begin();
        self.tick(ctx);
        crate::renderer::own_work_end();
    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        self.paint_frame(ui, frame);
    }
}

impl App {
    /// What runs at every frame, window or not (`logic`, and a headless run's loop).
    pub fn tick(&mut self, ctx: &egui::Context) {
        // The freeze watchdog times this as it times `ui` (crash.rs): a command or the clock
        // that hangs is reported too, minimized or not (#202).
        crate::crash::logic_begin();
        crate::crash::step(crate::crash::Step::Commands);
        let mut handled = false;
        while let Ok(inc) = self.commands.try_recv() {
            handled = true;
            match inc.reply {
                Some(reply) => {
                    let _ = reply.send(self.answer(&inc.cmd));
                }
                None => self.handle_command(inc.cmd, ctx),
            }
        }
        if handled {
            ctx.request_ui_pass(egui::ViewportId::ROOT);
        }
        self.lookups.prune(Instant::now());
        crate::crash::step(crate::crash::Step::NetworkSettings);
        // The clock: what must go on whatever the main window's state (#174). Minimized, egui
        // runs no `ui`, and the queues, the Scheduler's starts and stops, the download limits,
        // the kill switch, saving the list, the clipboard monitor, the update check, the tray and
        // the taskbar's progress all stood still until the window was restored.
        crate::settings::share_proxy(self.settings.proxy());
        crate::settings::share_dns(self.settings.dns());
        crate::settings::share_video_settings(&self.settings);
        if !self.headless {
            crate::crash::step(crate::crash::Step::Accessibility);
            self.follow_assistive_technology(ctx);
        }
        crate::crash::step(crate::crash::Step::NetworkAdapter);
        self.network_adapter_tick(ctx);
        crate::crash::step(crate::crash::Step::Limits);
        self.limit_restart_tick();
        self.debrid_tick();
        crate::crash::step(crate::crash::Step::Downloads);
        self.poll(ctx);
        crate::crash::step(crate::crash::Step::RemoteTorrents);
        self.poll_remote_torrents();
        // What failed in any window, told here (dialogs::tell).
        if let Some((title, text)) = dialogs::told().pop() {
            self.notice = Some((title, text));
        }
        crate::crash::step(crate::crash::Step::Limits);
        self.apply_global_limit();
        // A server (#157) has no screen, clipboard or tray, and is updated by its image.
        if !self.headless {
            crate::crash::step(crate::crash::Step::UpdateCheck);
            self.auto_update_check(ctx);
            crate::crash::step(crate::crash::Step::Clipboard);
            self.watch_clipboard(ctx);
            crate::crash::step(crate::crash::Step::Tray);
            self.tray(ctx);
            // The taskbar button's green bar: everything running, as one.
            #[cfg(windows)]
            {
                crate::crash::step(crate::crash::Step::Taskbar);
                crate::win32::taskbar_progress(crate::MAIN_TITLE, self.overall_progress());
            }
        }
        if self
            .ticked
            .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
        {
            self.ticked = Some(Instant::now());
            // For a crash report: what MDM has open and running (sent when it changes).
            crate::crash::step(crate::crash::Step::ReportState);
            crate::crash::state(
                self.items.len(),
                self.items.iter().filter(|i| i.live.is_some()).count(),
                self.queues.len(),
                ctx.pixels_per_point(),
            );
            crate::crash::step(crate::crash::Step::Torrents);
            self.torrent_tick();
            // Minimized, the windows these open (Download complete, a limit's notice, Download
            // File Info for a copied link, an update) are made by a pass of the main window's
            // UI, which egui runs then only when asked: every 2 s, as the clock repaints.
            if ctx.input(|i| i.viewport().minimized == Some(true)) {
                ctx.request_ui_pass(egui::ViewportId::ROOT);
            }
        }
        ctx.request_repaint_after(Duration::from_secs(2));
        crate::crash::logic_end();
    }

    fn paint_frame(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        // The freeze watchdog times every frame's drawing, as it times its logic (crash.rs).
        crate::crash::frame_begin();
        // A lost graphics device is noticed before this frame paints (renderer.rs).
        crate::renderer::frame_begin();
        // Off-screen test runs: no window of ours may become the foreground one (win32.rs). Linux
        // and macOS need no such pass: their off-screen windows open inactive (main.rs `quiet`)
        // and nothing raises them then (dialogs::raise).
        #[cfg(windows)]
        if crate::offscreen() {
            crate::win32::keep_from_foreground();
        }
        crate::crash::window("main");
        let ctx = ui.ctx().clone();
        // Not in the frame the interface size changed in: the window's rect is measured at the
        // old one then.
        if self.follow_zoom(&ctx) {
            dialogs::publish_main_rect(&ctx);
        }
        // Incoming commands and the clock (downloads, queues, Scheduler, saving…): `logic`, just
        // before, which runs minimized too.
        self.poll_picks(&ctx);
        self.poll_cnl(&ctx);
        self.poll_imports();
        self.drop_target(&ctx);
        self.main_drops(&ctx);
        self.remember_layout(&ctx);
        // Light or dark: IDM follows Windows' app mode (checked every 2 s) unless View → Dark
        // Mode support is off. Switches live (IDM asks to restart). On Linux the desktop's own
        // change wakes a frame, and the check runs in it rather than at its next 2 s (#156).
        crate::crash::step(crate::crash::Step::Theme);
        let desktop_changed = crate::theme::system_changed();
        if desktop_changed
            || self
                .theme_checked
                .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
        {
            self.theme_checked = Some(Instant::now());
            // A theme file added, removed or renamed, pywal's colours written or taken away:
            // View → Theme lists it. Nothing here reads the registry or the disk: `lookwatch`'s
            // threads do, and this takes what they found last (#719).
            let files = crate::lookwatch::theme_files(&ctx);
            if let Some((seen, found)) = &files
                && *seen != self.theme_files_seen
            {
                self.theme_files_seen = *seen;
                self.theme_list = found.list.clone();
            }
            let changed = crate::theme::update(
                self.settings.dark_mode_support,
                self.settings.always_dark,
                &ctx,
            );
            // Native follows the system's accent and contrast as they change, a theme made from
            // pywal's colours the colours `wal` writes for each new wallpaper.
            let changed = if crate::theme::follow_system(&ctx)
                || crate::theme::follow_pywal(files.as_ref().map(|(_, f)| f))
            {
                crate::theme::apply_theme(&ctx);
                true
            } else {
                changed
            };
            // The caption follows (first check: the window may not exist yet; retried). Only
            // then: asking DWM every 2 s caught the window's thread in a stalled system (#633).
            if changed || !self.caption_set {
                crate::crash::step(crate::crash::Step::Caption);
                crate::theme::apply_mode(&ctx);
                #[cfg(windows)]
                {
                    self.caption_set =
                        crate::win32::caption_dark(crate::MAIN_TITLE, crate::theme::dark_surface());
                    // The menus Windows draws (the tray icon's, the window menu) too (#682).
                    crate::win32::menus_dark(crate::theme::dark_surface());
                }
                // Linux: the tray's menu is drawn by the desktop's StatusNotifierItem host (ksni
                // sends only its items), in the desktop's theme; no app can colour it. macOS: the
                // menu bar item's menu follows the menu bar's appearance, the system's.
                #[cfg(not(windows))]
                {
                    self.caption_set = true;
                }
            }
            if changed {
                ctx.request_repaint();
            }
        }
        crate::crash::step(crate::crash::Step::MainWindow);
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::new().fill(chrome::WINDOW_BG()))
            .show(ui, |ui| self.paint(ui));
        crate::crash::step(crate::crash::Step::Dialogs);
        self.dialogs(&ctx);
        crate::crash::frame_end();
        crate::renderer::frame_done(&ctx);
    }
}

/// A download no Download File Info answered: its address, login and browser context.
type Unseen = (url::Url, Option<(String, String)>, ipc::Command);

impl App {
    /// Quitting: downloads the browser handed over that never got their Download File Info
    /// (waiting their turn, or arriving at the last moment) go to the main queue as Download
    /// Later, not lost (the browser gave its own copy up). Commands after this get "gone".
    fn keep_unseen(&mut self) {
        let mut unseen: Vec<Unseen> = self
            .file_info_waiting
            .drain(..)
            .map(|(new, context)| (new.url, new.login, context))
            .collect();
        // The one Download File Info shows, unanswered (an early download is in the list
        // already).
        if let Some(fi) = self.file_info.take()
            && fi.early.is_none()
        {
            unseen.insert(0, (fi.url.clone(), fi.login.clone(), fi.context.clone()));
        }
        while let Ok(inc) = self.commands.try_recv() {
            if inc.reply.is_none()
                && let Some(url) = inc
                    .cmd
                    .url
                    .as_deref()
                    .and_then(|u| url::Url::parse(u.trim()).ok())
                    .filter(mdm_engine::supported)
            {
                unseen.push((url, None, inc.cmd));
            }
        }
        for (url, login, context) in unseen {
            let guessed = context.name.is_none();
            let name = context
                .name
                .clone()
                .unwrap_or_else(|| crate::model::browser_guess(&url, &context));
            let name = mdm_engine::naming::sanitize(&name);
            let category = Category::of(&name);
            let dir = self.settings.category_dir(category, &self.downloads);
            let dest = mdm_engine::naming::destination(&dir, &name);
            let size = context.size;
            let id = self.push_item(
                url,
                dest,
                category,
                String::new(),
                size,
                login,
                context,
                crate::i18n::mark("from the browser"),
            );
            self.name_guessed(id, guessed);
            self.queues[0].items.push(id);
        }
        // The channel's receiver goes with it: later commands are answered "gone".
        self.commands = std::sync::mpsc::channel().1;
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Let any in-flight torrent folder rename finish and update `item.dest` before saving.
        for (id, from, rx) in std::mem::take(&mut self.renaming_torrents) {
            if let Ok((r, sharing_error)) = rx.recv_timeout(Duration::from_secs(2)) {
                self.apply_finished_torrent_rename(id, &from, r, sharing_error, false);
            }
        }
        // An archive being extracted stops; its staging folder goes with it.
        self.extracts.stop();
        // No wake for a queue MDM won't be there to start (Linux's alarm would outlive it).
        self.wake.clear();
        self.keep_unseen();
        // The feeds as last handed to their saver.
        self.rss_saver.flush();
        // What the saver has in hand first, then the last state, written here and now.
        self.saver.flush();
        let _ = store::save(
            self.next_id,
            &self.items,
            &self.list.layout(),
            &self.settings,
            &self.queues,
        );
    }
}

/// The event log's line when a seeding limit stops a torrent's sharing.
fn sharing_stopped_note(why: crate::torrents::Reached) -> String {
    match why {
        crate::torrents::Reached::Ratio(r) => crate::i18n::fmt(
            // i18n: event log entry, past tense: a finished torrent stopped being shared to others because its share ratio (sent ÷ received) reached the limit set in Torrent settings; {ratio} = e.g. 2.00
            crate::i18n::t("Sharing stopped: the share ratio reached {ratio}"),
            &[("ratio", &crate::i18n::decimal(r, 2))],
        ),
        crate::torrents::Reached::Time(s) => crate::i18n::fmt(
            // i18n: event log entry, past tense: a finished torrent stopped being shared to others after the time set in Torrent settings; {time} = e.g. 1 hour(s) 15 min
            crate::i18n::t("Sharing stopped after sharing for {time}"),
            &[("time", &model::time(s as f64))],
        ),
    }
}

/// What one side (torrents, or file downloads) may take of the global cap (bytes/s) while the
/// other uses `other`: half of it at least, and what the other leaves when that is more; never
/// under 1 KB/s (the limiter takes 0 as no limit).
///
/// A video's or a gallery's speed limit as it starts (#708): the lower of `one` (its own limit,
/// else the one for one file) and an even share of `global` (the limit for all downloads as file
/// downloads have it now) with the `others` running; never under 1 KB/s.
fn helper_rate(one: Option<u64>, global: Option<u64>, others: u64) -> Option<u64> {
    let share = global.map(|g| (g / (others + 1)).max(1024));
    match (one, share) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// An other side within 15% of its half counts as using its half: measured speeds wobble, and
/// taking every dip as room to grow kept both sides over their halves (measured: 463 + 398
/// KB/s under an 800 KB/s cap).
fn share(cap: u64, other: f64) -> u64 {
    let half = cap / 2;
    let other = other.max(0.0);
    let rest = if other >= half as f64 * 0.85 {
        half
    } else {
        cap.saturating_sub(other as u64)
    };
    rest.max(half).max(1024)
}

#[cfg(test)]
mod finished_window_tests {
    use super::finished_windows;
    use crate::progress::SHOWN_WHOLE;
    use std::time::{Duration, Instant};

    /// A window on screen shows its finished download whole for SHOWN_WHOLE, then closes; one
    /// not on screen (hidden in the tray, or none) closes at once; one started again meanwhile
    /// stays for the new run.
    #[test]
    fn a_finished_window_shows_it_whole_for_a_moment() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut finishing = Vec::new();
        let close = finished_windows(vec![1, 2], |id| id == 1, |_| true, &mut finishing, t0);
        assert_eq!(close, [2], "not on screen: at once");
        assert_eq!(finishing, [(1, t0)]);
        let soon = t0 + SHOWN_WHOLE - ms(1);
        let close = finished_windows(vec![], |_| true, |_| true, &mut finishing, soon);
        assert!(close.is_empty(), "held for the moment");
        let close = finished_windows(vec![3], |_| true, |_| true, &mut finishing, t0 + ms(100));
        assert!(close.is_empty());
        let close = finished_windows(vec![], |_| true, |_| true, &mut finishing, t0 + SHOWN_WHOLE);
        assert_eq!(close, [1], "then it closes");
        // 3, started again during its moment: not closed, and no longer waited for.
        let later = t0 + ms(100) + SHOWN_WHOLE;
        let close = finished_windows(vec![], |_| true, |id| id != 3, &mut finishing, later);
        assert!(close.is_empty());
        assert!(finishing.is_empty());
    }
}

#[cfg(test)]
mod move_many_tests {
    use super::place_in;

    /// #333: a selection moved to one folder keeps each its own name, or the next free one when
    /// several would land on the same name (not started: no file yet) or one is on the disk.
    #[test]
    fn a_selection_gets_a_free_name_each() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("a.zip"), b"x").unwrap();
        let mut taken = std::collections::HashSet::new();
        let names: Vec<String> = ["a.zip", "a.zip", "b.bin", "b.bin"]
            .iter()
            .map(|n| {
                place_in(d, n, &mut taken)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["a (1).zip", "a (2).zip", "b.bin", "b (1).bin"]);
    }
}

#[cfg(test)]
mod same_name_tests {
    use super::{Item, place_to_take};
    use crate::model::{Category, Status};
    use std::path::Path;

    fn row(id: u64, dest: &Path, status: Status, downloaded: u64) -> Item {
        let url = format!("http://files.test/{id}/master.mp4")
            .parse()
            .unwrap();
        let mut i = Item::new(id, url, dest.to_owned(), Category::Video);
        i.status = status;
        i.downloaded = downloaded;
        i
    }

    fn name(p: Option<std::path::PathBuf>) -> Option<String> {
        p.map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
    }

    /// #442: of two rows waiting under one name the first to start keeps it; the other goes to
    /// the next free one, not to the same file. A row with its own data never moves.
    #[test]
    fn a_row_leaves_a_name_another_row_holds() {
        let dir = tempfile::tempdir().unwrap();
        let at = dir.path().join("master.mp4");
        let waiting = |id| row(id, &at, Status::Queued, 0);
        let free = Some("master (1).mp4");

        // Both only waiting: nobody has it yet, the one starting stays.
        let items = [waiting(1), waiting(2)];
        assert_eq!(place_to_take(&items[0], &items), None);

        // The other is part-way (its pieces and resume state are at that name).
        let items = [row(1, &at, Status::Stopped, 500), waiting(2)];
        assert_eq!(name(place_to_take(&items[1], &items)).as_deref(), free);
        // And the part-way one resumes where it is, whoever else waits for the name.
        assert_eq!(place_to_take(&items[0], &items), None);

        // The other finished, its file there.
        std::fs::write(&at, b"x").unwrap();
        let items = [row(1, &at, Status::Complete, 1), waiting(2)];
        assert_eq!(name(place_to_take(&items[1], &items)).as_deref(), free);
        // Finished, but its file is gone: the name is free again.
        std::fs::remove_file(&at).unwrap();
        assert_eq!(place_to_take(&items[1], &items), None);

        // A crash left the list's count at nothing, but the resume state says whose the pieces
        // are: that row stays and resumes, the other leaves.
        let state = mdm_engine::state::state_path(&at);
        let mut items = [waiting(1), waiting(2)];
        let url = items[0].url.clone();
        mdm_engine::state::ResumeState::new(url.clone(), url, at.clone(), 9, Default::default())
            .write(&state)
            .unwrap();
        assert_eq!(place_to_take(&items[0], &items), None);
        assert_eq!(name(place_to_take(&items[1], &items)).as_deref(), free);
        items[0].status = Status::Stopped;
        assert_eq!(name(place_to_take(&items[1], &items)).as_deref(), free);
        std::fs::remove_file(&state).unwrap();

        // Another folder's row of that name is no clash.
        let other = tempfile::tempdir().unwrap();
        let there = other.path().join("master.mp4");
        let items = [row(1, &there, Status::Stopped, 5), waiting(2)];
        assert_eq!(place_to_take(&items[1], &items), None);
    }

    /// A video's stem has to be free whatever the extension: yt-dlp would take another file of
    /// that stem for its own and call the video downloaded.
    #[test]
    fn a_video_leaves_a_stem_another_row_holds() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let mut video = row(2, &d.join("master.mp4"), Status::Queued, 0);
        video.video = Some(crate::model::VideoSpec {
            selector: "b".into(),
            subs: Default::default(),
            parts: Vec::new(),
        });
        let items = [row(1, &d.join("Master.webm"), Status::Stopped, 9), video];
        assert_eq!(
            name(place_to_take(&items[1], &items)).as_deref(),
            Some("master (1).mp4")
        );
    }
}

#[cfg(test)]
mod at_login_tests {
    use super::{AtLogin, TRAY_GRACE, at_login};
    use std::time::Duration;

    /// Where a start with the session leaves the window (#226), in each order its tray icon and
    /// the desktop's tray can come in.
    #[test]
    fn the_window_waits_for_the_tray_then_stays_there_or_shows_minimized() {
        let s = Duration::from_secs;
        let icon = |d| Some(s(d));
        // The tray first (Windows, KDE): MDM's icon registered, the window stays hidden.
        assert_eq!(at_login(s(0), icon(0), true), AtLogin::Tray);
        // The decision first (XFCE: MDM looked before its own icon had registered; GNOME: the
        // AppIndicator extension's tray came 11.5 s after MDM's icon): waited for, hidden, then
        // the tray.
        assert_eq!(at_login(s(0), None, false), AtLogin::Wait);
        assert_eq!(at_login(s(0), icon(0), false), AtLogin::Wait);
        assert_eq!(at_login(s(13), icon(12), true), AtLogin::Tray);
        assert_eq!(
            at_login(s(60), TRAY_GRACE.checked_sub(s(1)), true),
            AtLogin::Tray
        );
        // The grace counts from MDM's icon: a slow start of its own leaves the desktop its time.
        assert_eq!(at_login(TRAY_GRACE + s(5), icon(5), false), AtLogin::Wait);
        // No tray within the grace period (a desktop without one, #59; Windows' taskbar not
        // there to take the icon): shown, minimized.
        assert_eq!(
            at_login(TRAY_GRACE, Some(TRAY_GRACE), false),
            AtLogin::Minimize
        );
        // No icon at all (the tray couldn't be made): the grace from the start.
        assert_eq!(at_login(TRAY_GRACE, None, false), AtLogin::Minimize);
        assert_eq!(at_login(s(60), None, true), AtLogin::Minimize);
    }
}

#[cfg(test)]
mod tray_toggle_tests {
    use super::{TRAY_TOGGLE_GRACE, tray_toggle_hide};
    use std::time::Duration;

    /// Left click toggles: hidden or minimized shows; frontmost hides; behind
    /// shows; a click that just took the focus (Windows) still hides (#520).
    #[test]
    fn only_a_frontmost_window_hides() {
        let ms = Duration::from_millis;
        // Hidden or minimized: always shows.
        assert!(!tray_toggle_hide(true, false, Some(false), None));
        assert!(!tray_toggle_hide(false, true, Some(true), None));
        // Frontmost: hides.
        assert!(tray_toggle_hide(false, false, Some(true), None));
        // Behind other windows: shows.
        assert!(!tray_toggle_hide(
            false,
            false,
            Some(false),
            Some(TRAY_TOGGLE_GRACE + ms(1))
        ));
        assert!(!tray_toggle_hide(false, false, Some(false), None));
        // The click stole the focus just now (Windows): still hides.
        assert!(tray_toggle_hide(
            false,
            false,
            Some(false),
            Some(TRAY_TOGGLE_GRACE - ms(1))
        ));
        // Unknown focus: unsure means show, never hide.
        assert!(!tray_toggle_hide(false, false, None, None));
        assert!(!tray_toggle_hide(false, false, None, Some(Duration::ZERO)));
    }
}

#[cfg(test)]
mod global_cap_tests {
    use super::{helper_rate, share};

    /// #708: a video's or a gallery's limit as it starts: its own (or the one for one file), and
    /// an even share of the limit for all downloads with the other helpers running.
    #[test]
    fn a_helper_starts_under_both_limits() {
        const K: u64 = 1024;
        assert_eq!(helper_rate(None, None, 0), None);
        assert_eq!(helper_rate(Some(500 * K), None, 3), Some(500 * K));
        assert_eq!(helper_rate(None, Some(500 * K), 0), Some(500 * K));
        assert_eq!(
            helper_rate(None, Some(500 * K), 1),
            Some(250 * K),
            "shared by two"
        );
        assert_eq!(helper_rate(Some(200 * K), Some(500 * K), 1), Some(200 * K));
        assert_eq!(helper_rate(Some(400 * K), Some(500 * K), 1), Some(250 * K));
        assert_eq!(
            helper_rate(None, Some(2 * K), 9),
            Some(K),
            "never under 1 KB/s"
        );
    }

    #[test]
    fn each_side_gets_half_and_what_the_other_leaves() {
        let cap = 800 * 1024;
        assert_eq!(share(cap, 0.0), cap, "the other idle: all of it");
        assert_eq!(share(cap, 100.0 * 1024.0), 700 * 1024, "a slow other side");
        assert_eq!(share(cap, 300.0 * 1024.0), 500 * 1024);
        assert_eq!(share(cap, 400.0 * 1024.0), 400 * 1024);
        assert_eq!(
            share(cap, 350.0 * 1024.0),
            400 * 1024,
            "a wobble under its half"
        );
        assert_eq!(share(cap, 800.0 * 1024.0), 400 * 1024, "half at least");
        assert_eq!(
            share(cap, 5e9),
            400 * 1024,
            "the other measured over the cap"
        );
        assert_eq!(share(1024, 0.0), 1024);
        assert_eq!(share(1500, 1500.0), 1024, "at least 1 KB/s");
    }
}

#[cfg(test)]
mod referrer_tests {
    use super::sent_referrer;

    #[test]
    fn referrers_as_browsers_send_them() {
        let url = |u: &str| url::Url::parse(u).unwrap();
        let page = "https://shop.test/orders/123?tab=2#top";
        assert_eq!(
            sent_referrer(&url("https://shop.test/invoice.pdf"), page).as_deref(),
            Some("https://shop.test/orders/123?tab=2"),
            "same site: the whole address, no fragment"
        );
        assert_eq!(
            sent_referrer(&url("https://cdn.test/a.zip"), page).as_deref(),
            Some("https://shop.test/"),
            "another site: the origin only"
        );
        assert_eq!(
            sent_referrer(&url("http://plain.test/a.zip"), page),
            None,
            "https to http: none"
        );
        assert_eq!(
            sent_referrer(&url("https://x.test/a"), "javascript:void(0)"),
            None
        );
    }

    #[test]
    fn a_drive_players_button_downloads_the_file() {
        use super::{drive_file_behind_player, ipc};
        let page = "https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/view";
        let cmd = |video: &str, referer: Option<&str>| ipc::Command {
            video_page: Some(video.to_owned()),
            referer: referer.map(str::to_owned),
            ..Default::default()
        };
        assert_eq!(
            drive_file_behind_player(&cmd(page, None)).as_deref(),
            Some(page)
        );
        let stream = "https://abc.c.drive.google.com/videoplayback?id=1";
        assert_eq!(
            drive_file_behind_player(&cmd(stream, Some(page))).as_deref(),
            Some(page)
        );
        // Another site's video, even when reached from a Drive page, stays a video.
        assert_eq!(
            drive_file_behind_player(&cmd("https://www.youtube.com/watch?v=x", Some(page))),
            None
        );
        let folder = "https://drive.google.com/drive/folders/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu";
        assert_eq!(drive_file_behind_player(&cmd(folder, None)), None);
    }

    /// The quality menu leaves a Drive file's player to the window's click (the file itself),
    /// and everything to it in a run without a window.
    #[test]
    fn the_quality_menu_leaves_drive_and_headless_to_the_window() {
        use super::{ipc, menu_needs_window};
        let page = "https://drive.google.com/file/d/1s52ek_4YTDRt_EOkx1FS53u-vJa0c4nu/view";
        let look = |address: &str, referer: Option<&str>| ipc::Command {
            video_lookup: Some(ipc::VideoLookup {
                address: address.to_owned(),
                ..Default::default()
            }),
            referer: referer.map(str::to_owned),
            startup: true,
            ..Default::default()
        };
        assert!(menu_needs_window(&look(page, None), false));
        let stream = "https://abc.c.drive.google.com/videoplayback?id=1";
        assert!(menu_needs_window(&look(stream, Some(page)), false));
        let video = look("https://www.youtube.com/watch?v=x", Some(page));
        assert!(!menu_needs_window(&video, false));
        assert!(menu_needs_window(&video, true), "no window to show it");
        assert!(menu_needs_window(&look("file:///C:/v.mp4", None), false));
        assert!(menu_needs_window(&look("", None), false));
        let poll = ipc::Command {
            video_poll: Some("abc".into()),
            ..Default::default()
        };
        assert!(!menu_needs_window(&poll, false));
        assert!(menu_needs_window(&poll, true));
    }
}

#[cfg(test)]
mod video_goes_tests {
    use super::{VideoGoes, video_goes};

    /// The browser's quality menu queues a choice (Options: only add to the queue) in the main
    /// queue, never through the queue panel, which would open in MDM's window and, cancelled,
    /// leave it in no queue; the Download video window's Later still asks when Options say so.
    #[test]
    fn a_choice_from_the_browser_is_queued_without_asking() {
        // The quality menu.
        assert_eq!(video_goes(false, true, true), VideoGoes::MainQueue);
        assert_eq!(video_goes(false, true, false), VideoGoes::MainQueue);
        assert_eq!(video_goes(true, true, true), VideoGoes::Start);
        // The Download video window, as before.
        assert_eq!(video_goes(false, false, true), VideoGoes::AskQueue);
        assert_eq!(video_goes(false, false, false), VideoGoes::MainQueue);
        assert_eq!(video_goes(true, false, true), VideoGoes::Start);
    }
}

#[cfg(test)]
mod duplicate_tests {
    use super::*;
    use mdm_tools::video::{InfoSlot, Preloaded};

    const SELECTOR: &str = "bv*[height=720]+ba/b[height=720]";

    fn page() -> url::Url {
        url::Url::parse("https://videos.test/watch?v=1").unwrap()
    }

    /// A choice from the quality menu, holding (a clone of) `slot`, the menu's look-up.
    fn choice(slot: &InfoSlot) -> crate::videodlg::VideoAdd {
        crate::videodlg::VideoAdd {
            page: page(),
            selector: SELECTOR.into(),
            dest: PathBuf::from("Clip.mp4"),
            category: Category::Video,
            size: None,
            start_now: true,
            context: Default::default(),
            subs: Default::default(),
            remember: None,
            preloaded: Some(Preloaded {
                slot: slot.clone(),
                selector: SELECTOR.into(),
                label: "720p".into(),
                merge: true,
            }),
        }
    }

    /// Options' four answers to a duplicate, for any download; a finished file that is gone
    /// (#342) leaves nothing to duplicate, whatever the answer.
    #[test]
    fn options_choice_decides_what_a_duplicate_does() {
        for action in 0..=4 {
            assert_eq!(duplicate_step(action, None), DupStep::Add);
            assert_eq!(
                duplicate_step(action, Some((7, true))),
                DupStep::Replace(7, false)
            );
        }
        let dup = Some((7, false));
        assert_eq!(duplicate_step(0, dup), DupStep::Ask(7));
        assert_eq!(duplicate_step(1, dup), DupStep::Add);
        assert_eq!(duplicate_step(2, dup), DupStep::Replace(7, true));
        assert_eq!(duplicate_step(3, dup), DupStep::Existing(7));
        assert_eq!(duplicate_step(9, dup), DupStep::Ask(7), "unknown: asked");
    }

    /// A video is the same download only on the same page in the same quality; a plain download
    /// of that address is not a video's duplicate.
    #[test]
    fn a_video_is_a_duplicate_in_the_same_quality_only() {
        let mut item = Item::new(1, page(), PathBuf::from("Clip.mp4"), Category::Video);
        assert!(!same_video(&item, &page(), SELECTOR), "not a video");
        item.video = Some(model::VideoSpec {
            selector: SELECTOR.into(),
            subs: Default::default(),
            parts: Vec::new(),
        });
        assert!(same_video(&item, &page(), SELECTOR));
        assert!(!same_video(
            &item,
            &page(),
            "bv*[height=360]+ba/b[height=360]"
        ));
        let other = url::Url::parse("https://videos.test/watch?v=2").unwrap();
        assert!(!same_video(&item, &other, SELECTOR));
    }

    /// Each Options answer on both video paths (the quality menu, the Download video window):
    /// where the choice goes, what the browser is told, and the menu's look-up, which goes with
    /// the new download only: dropped when the one already there is opened or resumed.
    #[test]
    fn each_answer_on_both_video_paths() {
        use ipc::api::lookup;
        let slot = InfoSlot::new(Instant::now());
        for from_menu in [true, false] {
            for action in 0..=3u8 {
                let step = duplicate_step(action, Some((7, false)));
                let route = video_route(step, choice(&slot), from_menu);
                match (action, &route) {
                    (1 | 2, VideoRoute::Add(add)) => {
                        assert!(add.preloaded.is_some());
                        assert_eq!(slot.holders(), 2, "kept for the new download");
                    }
                    (0, VideoRoute::Ask(7, Pending::Video(add, menu))) => {
                        assert_eq!(*menu, from_menu);
                        assert!(add.preloaded.is_some());
                        assert_eq!(slot.holders(), 2, "kept for the answer");
                    }
                    (3, VideoRoute::Dropped) => {
                        assert_eq!(slot.holders(), 1, "the look-up went with the choice");
                    }
                    _ => panic!("action {action}: unexpected route"),
                }
                drop(route);
                assert_eq!(slot.holders(), 1);
                // The browser hears of a new download, or `sent` (asked, or the one there).
                let told = menu_answer(step, true);
                assert_eq!(
                    told,
                    match action {
                        1 | 2 => lookup::STARTED,
                        _ => lookup::SENT,
                    }
                );
            }
        }
        // Options' "only add to the queue": a new one is queued.
        assert_eq!(menu_answer(DupStep::Add, false), lookup::QUEUED);
        assert_eq!(
            menu_answer(DupStep::Replace(7, true), false),
            lookup::QUEUED
        );
        assert_eq!(menu_answer(DupStep::Existing(7), false), lookup::SENT);
        assert_eq!(menu_answer(DupStep::Ask(7), false), lookup::SENT);
    }

    /// The question's answer: opening or resuming the one already there (3) drops the new
    /// choice and its look-up; a numbered name (1) or overwrite (2, of that one) adds it,
    /// look-up and all; whatever the answer, a question about a download that has left the list
    /// meanwhile is checked afresh (never acted on a row that isn't there).
    #[test]
    fn the_answer_open_or_resume_drops_the_new_choices_look_up() {
        let slot = InfoSlot::new(Instant::now());
        for from_menu in [true, false] {
            for answer in 1..=3 {
                let pending = Pending::Video(Box::new(choice(&slot)), from_menu);
                match (answer, answered(7, true, answer, pending)) {
                    (3, Answer::Existing(7)) => assert_eq!(slot.holders(), 1),
                    (1, Answer::Add(None, Pending::Video(add, menu)))
                    | (2, Answer::Add(Some(7), Pending::Video(add, menu))) => {
                        assert_eq!(menu, from_menu);
                        assert!(add.preloaded.is_some());
                        assert_eq!(slot.holders(), 2);
                    }
                    _ => panic!("answer {answer}: unexpected"),
                }
                assert_eq!(slot.holders(), 1);
                let pending = Pending::Video(Box::new(choice(&slot)), from_menu);
                match answered(7, false, answer, pending) {
                    Answer::Again(Pending::Video(add, menu)) => {
                        assert_eq!(menu, from_menu);
                        assert!(add.preloaded.is_some(), "checked afresh, look-up and all");
                    }
                    _ => panic!("answer {answer} about a row gone: not checked afresh"),
                }
            }
        }
    }

    /// The menu's look-up goes with a download started now, while fresh: not with one queued,
    /// nor one added on a duplicate question answered after the look-ups are forgotten (its
    /// links will have expired; it looks the page up itself, as at its start).
    #[test]
    fn a_late_answer_drops_the_menus_look_up() {
        let born = Instant::now();
        let slot = InfoSlot::new(born);
        let ttl = crate::videolookup::TTL;
        let pre = || choice(&slot).preloaded;
        assert!(kept_lookup(pre(), VideoGoes::Start, born).is_some());
        let just = born + ttl - Duration::from_secs(1);
        assert!(kept_lookup(pre(), VideoGoes::Start, just).is_some());
        assert!(kept_lookup(pre(), VideoGoes::Start, born + ttl).is_none());
        let hours = born + Duration::from_secs(3 * 3600);
        assert!(kept_lookup(pre(), VideoGoes::Start, hours).is_none());
        assert!(kept_lookup(pre(), VideoGoes::MainQueue, born).is_none());
        assert!(kept_lookup(pre(), VideoGoes::AskQueue, born).is_none());
        assert_eq!(slot.holders(), 1, "every one dropped");
    }

    /// A video row in `dir` named `name`, finished or not.
    fn video_row(id: u64, dir: &std::path::Path, name: &str, status: Status) -> Item {
        let mut i = Item::new(id, page(), dir.join(name), Category::Video);
        i.status = status;
        i.video = Some(model::VideoSpec {
            selector: SELECTOR.into(),
            subs: Default::default(),
            parts: Vec::new(),
        });
        i
    }

    fn names(files: &[PathBuf]) -> Vec<String> {
        let mut n: Vec<String> = files
            .iter()
            .filter(|f| f.exists())
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        n.sort();
        n
    }

    /// Overwrite takes the old video's own files (its file, yt-dlp's leftovers, its subtitles),
    /// never those of another download in that folder whose name or stem starts them
    /// (`Clip.f137.mp4`'s parts, `Clip.v2.zip`'s), running or paused; one sharing its very stem
    /// (`Clip.zip`) leaves every leftover where it is.
    #[test]
    fn overwrite_spares_every_other_downloads_files() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for f in [
            "Clip.mp4",
            "Clip.en.srt",
            "Clip.fr.srt",
            "Clip.mp4.part-Frag3",
            "Clip.f140.m4a",
            "Clip.f137.mp4.part",
            "Clip.f137.mp4.ytdl",
            "CLIP.F137.mp4.part-Frag2",
            "Clip.v2.zip.mdm.part",
        ] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let old = || video_row(1, d, "Clip.mp4", Status::Complete);
        // Another video named after a stream of the first, running: its parts are its own.
        let other_again = || video_row(2, d, "Clip.f137.mp4", Status::Downloading);
        let other = other_again();
        // A plain download, paused, named alike.
        let plain_again = || {
            let mut plain = Item::new(3, page(), d.join("Clip.v2.zip"), Category::General);
            plain.status = Status::Stopped;
            plain
        };
        let plain = plain_again();
        let files = replaced_files(&[old(), other, plain], 1, true).unwrap();
        assert_eq!(
            names(&files),
            [
                "Clip.en.srt",
                "Clip.f140.m4a",
                "Clip.fr.srt",
                "Clip.mp4",
                "Clip.mp4.part-Frag3"
            ]
        );
        // Deleted with its files (a merge failed, it was never resumed): what yt-dlp kept for
        // Resume goes, never its subtitles or another download's parts.
        assert_eq!(
            names(&video_leftovers(
                &[old(), other_again(), plain_again()],
                1,
                false
            )),
            ["Clip.f140.m4a", "Clip.mp4.part-Frag3"]
        );
        // Another download of the same stem: only the old one's own file.
        let zip = Item::new(4, page(), d.join("clip.zip"), Category::General);
        let files = replaced_files(&[old(), zip], 1, true).unwrap();
        assert_eq!(names(&files), ["Clip.mp4"]);
        // One of that name in another folder changes nothing here.
        let elsewhere = tempfile::tempdir().unwrap();
        let far = video_row(5, elsewhere.path(), "Clip.f140.mp4", Status::Stopped);
        let files = replaced_files(&[old(), far], 1, true).unwrap();
        assert!(names(&files).contains(&"Clip.f140.m4a".to_owned()));
    }

    /// A failed merge's parts go with Delete by the names the run kept, also those yt-dlp
    /// names as no other leftover is named (an HLS sound's `Clip.faud-English.mp4`); never one
    /// in another folder or of another name.
    #[test]
    fn a_failed_merges_parts_go_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for f in ["Clip.f1280.mp4", "Clip.faud-English.mp4", "Other.mp4"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let mut items = [video_row(1, d, "Clip.mp4", Status::Error)];
        assert_eq!(
            names(&video_leftovers(&items, 1, false)),
            ["Clip.f1280.mp4"]
        );
        items[0].video.as_mut().unwrap().parts = vec![
            d.join("Clip.f1280.mp4"),
            d.join("Clip.faud-English.mp4"),
            d.join("Other.mp4"),
        ];
        assert_eq!(
            names(&video_leftovers(&items, 1, false)),
            ["Clip.f1280.mp4", "Clip.faud-English.mp4"]
        );
    }

    /// A running download is left alone; #342's automatic replacement (its finished file gone)
    /// takes its row, state and part file only, never what yt-dlp left beside it.
    #[test]
    fn a_running_one_is_left_alone_and_a_gone_ones_leftovers_stay() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for f in ["Clip.en.srt", "Clip.mp4.part", "Clip.f140.m4a"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let mut running = video_row(1, d, "Clip.mp4", Status::Downloading);
        // A run of a yt-dlp that isn't there: it fails at once, on its own thread (no network).
        let job = mdm_tools::video::VideoJob::start(
            &d.join("no-yt-dlp.exe"),
            page().as_str(),
            SELECTOR,
            d,
            "Clip",
            None,
            None,
            None,
            &Default::default(),
            &Default::default(),
            None,
        )
        .unwrap();
        running.live = Some(Live::new(model::Job::Video { job, url: page() }, 0));
        assert!(replaced_files(&[running], 1, true).is_none());
        // #342: finished, its file gone.
        let gone = video_row(1, d, "Clip.mp4", Status::Complete);
        let files = replaced_files(&[gone], 1, false).unwrap();
        assert!(names(&files).is_empty(), "{:?}", names(&files));
        assert_eq!(
            files,
            [
                d.join("Clip.mp4"),
                state_path(&d.join("Clip.mp4")),
                mdm_engine::naming::part_path(&d.join("Clip.mp4")),
            ]
        );
        assert!(replaced_files(&[], 1, true).is_none());
    }
}

#[cfg(test)]
mod trim_tests {
    use super::next_trim;
    use std::time::Duration;

    /// The tray gives memory back 2 s after hiding, 2 minutes after hiding, then every
    /// 10 minutes (#72).
    #[test]
    fn hidden_memory_is_given_back_at_2_s_2_min_then_every_10_min() {
        let mut at = Duration::ZERO;
        let times: Vec<u64> = (0..4)
            .map(|done| {
                at += next_trim(done);
                at.as_secs()
            })
            .collect();
        assert_eq!(times, [2, 120, 720, 1320]);
    }
}

#[cfg(test)]
mod page_data_tests {
    use super::page_data_for;

    const PAGE: Option<&str> = Some("https://forum.example/thread/1");
    const DATA: Option<&str> = Some("a=1");

    #[test]
    fn links_take_what_belongs_to_them() {
        let got = |link| page_data_for(link, PAGE, DATA);
        assert_eq!(got("https://forum.example/files/a.zip").as_deref(), DATA);
        assert_eq!(got("https://FORUM.example/b.zip").as_deref(), DATA);
        assert_eq!(got("https://other.test/a.zip"), None);
        assert_eq!(got("https://forum.example.other.test/a.zip"), None);
        assert_eq!(got("https://cdn.forum.example/a.zip"), None);
        assert_eq!(got("http://forum.example/a.zip"), None);
        assert_eq!(got("not an address"), None);
    }

    #[test]
    fn no_page_or_nothing_to_take() {
        assert_eq!(page_data_for("https://a.test/x", None, DATA), None);
        assert_eq!(
            page_data_for("https://a.test/x", Some("https://a.test/"), None),
            None
        );
        assert_eq!(page_data_for("https://a.test/x", Some("nope"), DATA), None);
    }
}

#[cfg(test)]
mod window_thread_tests {
    use super::*;

    /// #423: the files of a stopped download go as a group, off the window's thread.
    #[test]
    fn a_stopped_downloads_files_are_removed_together() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        let part = mdm_engine::naming::part_path(&dest);
        let state = mdm_engine::state::state_path(&dest);
        for f in [&dest, &part, &state] {
            std::fs::write(f, b"x").unwrap();
        }
        remove_files_of(&dest, false);
        assert!(!part.exists() && !state.exists());
        assert!(dest.exists(), "a file that wasn't complete stays");
        remove_files_of(&dest, true);
        assert!(!dest.exists());
    }
}

#[cfg(test)]
mod mark_finished_tests {
    /// #510: finished files are marked away from the window's thread; a file gone from its path
    /// meanwhile (moved, deleted) isn't created again by the mark.
    #[test]
    fn a_finished_file_is_marked_and_a_gone_one_is_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let here = dir.path().join("video.mp4");
        std::fs::write(&here, b"x").unwrap();
        let gone = dir.path().join("moved.mp4");
        super::mark_finished(
            vec![here.clone(), gone.clone()],
            "https://videos.test/v".into(),
            None,
        )
        .unwrap()
        .join()
        .unwrap();
        assert!(!gone.exists(), "a moved file's old path stays empty");
        #[cfg(windows)]
        {
            let mut stream = here.as_os_str().to_owned();
            stream.push(":Zone.Identifier");
            let zone = std::fs::read_to_string(stream).unwrap();
            assert!(zone.contains("ZoneId=3"), "{zone}");
            assert!(zone.contains("HostUrl=https://videos.test/v"), "{zone}");
        }
    }
}

#[cfg(test)]
mod deferred_rename_guard_tests {
    use super::*;

    fn test_item(id: u64, dest: PathBuf) -> Item {
        Item::new(
            id,
            "https://example.com/item".parse().unwrap(),
            dest,
            model::Category::General,
        )
    }

    #[test]
    fn refuses_shared_category_and_save_folders_but_allows_exclusive_folder() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("Downloads");
        let video_cat = downloads.join("Video");
        let saved_dir = dir.path().join("SavedAlbums");
        let shared_gallery = downloads.join("pixiv");
        let exclusive_gallery = downloads.join("gallery-42");
        for d in [
            &downloads,
            &video_cat,
            &saved_dir,
            &shared_gallery,
            &exclusive_gallery,
        ] {
            std::fs::create_dir_all(d).unwrap();
        }

        let mut settings = Settings::default();
        settings
            .folder_history
            .push(saved_dir.display().to_string());
        let rss = crate::rss::Store::default();

        let mut g_root = test_item(1, downloads.clone());
        g_root.gallery = Some(Default::default());

        let mut g_cat = test_item(2, video_cat.clone());
        g_cat.gallery = Some(Default::default());

        let mut g_saved = test_item(3, saved_dir.clone());
        g_saved.gallery = Some(Default::default());

        let mut g_shared_a = test_item(4, shared_gallery.clone());
        g_shared_a.gallery = Some(Default::default());
        let g_shared_b = test_item(5, shared_gallery.join("other.jpg"));

        let mut g_exclusive = test_item(6, exclusive_gallery.clone());
        g_exclusive.gallery = Some(Default::default());

        let items = vec![g_root, g_cat, g_saved, g_shared_a, g_shared_b, g_exclusive];

        for idx in [0usize, 1, 2, 3] {
            let blocked = &items[idx];
            assert_eq!(
                deferred_rename_disabled_reason_with(blocked, &downloads, &settings, &rss, &items),
                Some("This folder is shared with another download or save folder"),
                "expected {} to be refused",
                blocked.dest.display()
            );
        }
        assert_eq!(
            deferred_rename_disabled_reason_with(&items[5], &downloads, &settings, &rss, &items),
            None,
            "an exclusive gallery subfolder can be renamed safely"
        );

        let reused_gallery = downloads.join("gallery-reused");
        std::fs::create_dir_all(&reused_gallery).unwrap();
        let mut g_preexisting = test_item(7, reused_gallery.clone());
        g_preexisting.gallery = Some(Default::default());
        g_preexisting.preexisting_folder = true;
        assert_eq!(
            deferred_rename_disabled_reason_with(
                &g_preexisting,
                &downloads,
                &settings,
                &rss,
                &items,
            ),
            Some("This folder holds files from outside this download"),
            "a reused folder that already held untracked files is refused"
        );

        std::fs::write(reused_gallery.join("foreign.txt"), b"leftover").unwrap();
        let mut g_untracked = test_item(8, reused_gallery.clone());
        g_untracked.gallery = Some(Default::default());
        g_untracked.written_files = Some(Vec::new());
        assert_eq!(
            deferred_rename_disabled_reason_with(&g_untracked, &downloads, &settings, &rss, &items),
            Some("This folder holds files from outside this download"),
            "a folder containing files the download did not write is refused"
        );

        // Persisted across restart (`serde_json`) and updated when `take_place` renames the folder.
        let tracked = reused_gallery.join("01.jpg");
        std::fs::write(&tracked, b"ok").unwrap();
        let mut g_persisted = test_item(9, reused_gallery.clone());
        g_persisted.gallery = Some(Default::default());
        g_persisted.written_files = Some(vec![tracked]);
        let reloaded: Item =
            serde_json::from_str(&serde_json::to_string(&g_persisted).unwrap()).unwrap();
        assert_eq!(
            deferred_rename_disabled_reason_with(&reloaded, &downloads, &settings, &rss, &items),
            Some("This folder holds files from outside this download"),
            "persisted written_files still catches foreign.txt after restart"
        );

        std::fs::remove_file(reused_gallery.join("foreign.txt")).unwrap();
        assert_eq!(
            deferred_rename_disabled_reason_with(&reloaded, &downloads, &settings, &rss, &items),
            None
        );

        let moved_gallery = downloads.join("gallery-reused-renamed");
        std::fs::rename(&reused_gallery, &moved_gallery).unwrap();
        let mut moved_item = reloaded;
        take_place(&mut moved_item, moved_gallery.clone());
        assert_eq!(
            moved_item.written_files,
            Some(vec![moved_gallery.join("01.jpg")])
        );
        assert_eq!(
            deferred_rename_disabled_reason_with(&moved_item, &downloads, &settings, &rss, &items),
            None
        );
    }

    #[test]
    fn guards_split_archives_and_single_file_torrents() {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let mut settings = Settings::default();
        let rss = crate::rss::Store::default();

        let part1 = test_item(1, downloads.join("backup.part1.rar"));
        let standalone_rar = test_item(2, downloads.join("standalone.rar"));

        let mut single_torrent = test_item(3, downloads.join("ubuntu.iso"));
        single_torrent.torrent = Some(crate::torrents::TorrentSpec {
            hash: "0123456789abcdef0123456789abcdef01234567".into(),
            folder: downloads.clone(),
            files: None,
            uploaded: 0,
            seeded_secs: 0,
            peers: 0,
            up_speed: 0.0,
            upload_limit: None,
            in_order: false,
            ends_first: false,
        });

        let pack_dir = downloads.join("PackFolder");
        std::fs::create_dir_all(&pack_dir).unwrap();
        let mut folder_torrent = test_item(4, pack_dir.clone());
        folder_torrent.torrent = Some(crate::torrents::TorrentSpec {
            hash: "fedcba9876543210fedcba9876543210fedcba98".into(),
            folder: pack_dir,
            files: None,
            uploaded: 0,
            seeded_secs: 0,
            peers: 0,
            up_speed: 0.0,
            upload_limit: None,
            in_order: false,
            ends_first: false,
        });

        let items = vec![part1, standalone_rar, single_torrent, folder_torrent];

        assert_eq!(
            deferred_rename_disabled_reason_with(&items[0], &downloads, &settings, &rss, &items),
            Some("Split archive volumes keep their names for extraction")
        );
        assert_eq!(
            deferred_rename_disabled_reason_with(&items[1], &downloads, &settings, &rss, &items),
            None
        );

        settings.torrent_keep_sharing = true;
        assert_eq!(
            deferred_rename_disabled_reason_with(&items[2], &downloads, &settings, &rss, &items),
            Some("A single-file torrent keeps its name")
        );
        assert_eq!(
            deferred_rename_disabled_reason_with(&items[3], &downloads, &settings, &rss, &items),
            None,
            "an exclusive multi-file torrent folder can be renamed and re-attached"
        );

        settings.torrent_keep_sharing = false;
        assert_eq!(
            deferred_rename_disabled_reason_with(&items[2], &downloads, &settings, &rss, &items),
            Some("A single-file torrent keeps its name"),
            "single-file torrent keeps its name even with Keep sharing off so a resume finds it"
        );
    }

    #[test]
    fn pending_mark_updates_video_subtitles_and_gallery_paths_after_rename() {
        let dir = tempfile::tempdir().unwrap();
        let from_vid = dir.path().join("raw_id.mp4");
        let sub_en = dir.path().join("raw_id.en.srt");
        std::fs::write(&from_vid, b"v").unwrap();
        std::fs::write(&sub_en, b"s").unwrap();

        let mut vid_item = test_item(1, from_vid.clone());
        vid_item.pending_mark = Some(model::PendingMark {
            main_file: Some(from_vid.clone()),
            side_files: vec![sub_en.clone()],
            gallery_files: Vec::new(),
            url: "https://videos.test/watch".into(),
            referer: None,
        });

        let (initial_to, hold) =
            crate::deferred_rename::reserve_target_path(&from_vid, "Talk 01", false).unwrap();
        let actual_to =
            crate::deferred_rename::rename_reserved_or_number(&from_vid, initial_to, hold, false)
                .unwrap();
        apply_renamed_paths_to_pending_mark(&mut vid_item, &from_vid, &actual_to, false);
        let pm = vid_item.pending_mark.unwrap();
        assert_eq!(pm.main_file, Some(dir.path().join("Talk 01.mp4")));
        assert_eq!(pm.side_files, vec![dir.path().join("Talk 01.en.srt")]);
        assert!(dir.path().join("Talk 01.en.srt").exists());
        assert!(!sub_en.exists());

        let old_gal = dir.path().join("gal_raw");
        std::fs::create_dir_all(old_gal.join("sub")).unwrap();
        let img = old_gal.join("sub").join("01.png");
        std::fs::write(&img, b"img").unwrap();
        let mut gal_item = test_item(2, old_gal.clone());
        gal_item.gallery = Some(Default::default());
        gal_item.pending_mark = Some(model::PendingMark {
            main_file: None,
            side_files: Vec::new(),
            gallery_files: vec![img],
            url: "https://gallery.test/a".into(),
            referer: None,
        });
        let (g_to, g_hold) =
            crate::deferred_rename::reserve_target_path(&old_gal, "Album 01", true).unwrap();
        let actual_gal =
            crate::deferred_rename::rename_reserved_or_number(&old_gal, g_to, g_hold, true)
                .unwrap();
        apply_renamed_paths_to_pending_mark(&mut gal_item, &old_gal, &actual_gal, true);
        let gpm = gal_item.pending_mark.unwrap();
        assert_eq!(
            gpm.gallery_files,
            vec![dir.path().join("Album 01").join("sub").join("01.png")]
        );
    }
}

#[cfg(test)]
mod access_key_tests {
    use super::*;
    use crate::chrome::{MenuEntry, access_key};

    /// One level of a menu: where it is, the catalog context of its labels, and the labels.
    struct Level {
        name: String,
        context: &'static str,
        labels: Vec<&'static str>,
    }

    /// `entries` and each of their submenus, as levels.
    fn levels<C>(name: String, entries: &[MenuEntry<C>], out: &mut Vec<Level>) {
        let mut labels = Vec::new();
        for e in entries {
            match e {
                MenuEntry::Item(l, ..) | MenuEntry::Radio(l, ..) => labels.push(*l),
                MenuEntry::Sub(l, _, children) => {
                    labels.push(*l);
                    levels(format!("{name} > {l}"), children, out);
                }
                MenuEntry::Sep => {}
            }
        }
        out.push(Level {
            name,
            context: "menu",
            labels,
        });
    }

    /// The window's state with three downloads to build the menus of: one unfinished (in the
    /// main queue), one finished, one torrent.
    fn app() -> std::mem::ManuallyDrop<App> {
        let rt = Box::leak(Box::new(
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap(),
        ));
        let url = |n: &str| url::Url::parse(&format!("https://example.com/{n}")).unwrap();
        let item = |id, name: &str, category, status| {
            let mut i = Item::new(id, url(name), PathBuf::from(name), category);
            i.status = status;
            i
        };
        let mut torrent = item(3, "c", Category::Torrents, Status::Stopped);
        torrent.torrent = Some(
            serde_json::from_value(serde_json::json!({
                "hash": "0123456789abcdef0123456789abcdef01234567",
                "folder": "c",
            }))
            .unwrap(),
        );
        let mut main = Queue::new("Main download queue", false);
        main.items.push(1);
        let loaded = store::Loaded {
            next_id: 4,
            items: vec![
                item(1, "a.zip", Category::Compressed, Status::Stopped),
                item(2, "b.zip", Category::Compressed, Status::Complete),
                torrent,
            ],
            columns: None,
            settings: Settings::default(),
            queues: vec![main, Queue::new("Synchronization queue", true)],
        };
        let (_, commands) = std::sync::mpsc::channel();
        // Never dropped: an App saves its list as it goes.
        std::mem::ManuallyDrop::new(App::assemble(
            &egui::Context::default(),
            rt.handle().clone(),
            commands,
            loaded,
            crate::Start::Shown,
        ))
    }

    /// Every menu MDM builds, in the states that change them: the menu bar, its menus, the
    /// right-click and drop-down menus, a text box's, and their submenus.
    fn every_menu() -> Vec<Level> {
        let mut app = app();
        let mut out = vec![Level {
            name: "menu bar".into(),
            context: "menubar",
            labels: crate::chrome::bar_titles(),
        }];
        let mut views = vec![
            None,
            Some(View::All),
            Some(View::Unfinished(None)),
            Some(View::Finished(None)),
            Some(View::Queue(0)),
            Some(View::Queue(1)),
        ];
        for c in Category::all() {
            views.extend([
                Some(View::Cat(c)),
                Some(View::Unfinished(Some(c))),
                Some(View::Finished(Some(c))),
            ]);
        }
        let mut popups = vec![
            Popup::Columns,
            Popup::QueueDrop(true),
            Popup::QueueDrop(false),
            Popup::TorrentDrop,
            Popup::DropTarget,
            Popup::Item(None),
        ];
        popups.extend([1, 2, 3].map(|id| Popup::Item(Some(id))));
        popups.extend(views.into_iter().map(Popup::Node));
        for categories in [false, true] {
            app.show_categories = categories;
            for (i, title) in crate::chrome::bar_titles().into_iter().enumerate() {
                levels(title.into(), &app.menu_entries(i), &mut out);
            }
            for p in &popups {
                levels(format!("{p:?}"), &app.popup_entries(*p).0, &mut out);
            }
        }
        // The progress window's tab row and a read-only field have a menu of one item each.
        levels("text box".into(), &crate::winui::text_box_menu(), &mut out);
        out
    }

    /// An access key, and the labels that have it: as shown, and in English.
    type Clash = (String, Vec<(&'static str, &'static str)>);

    /// The access keys of `level` that more than one of its labels have, in `lang` (English:
    /// None), each with those labels.
    fn clashes(level: &Level, lang: Option<crate::i18n::Lang>) -> Vec<Clash> {
        let mut by_key: Vec<Clash> = Vec::new();
        for l in &level.labels {
            let shown = match lang {
                Some(lang) => crate::i18n::i18n().tc(lang, level.context, l),
                None => l,
            };
            let Some(key) = access_key(shown) else {
                continue;
            };
            match by_key.iter_mut().find(|(k, _)| *k == key) {
                Some((_, labels)) => labels.push((shown, l)),
                None => by_key.push((key, vec![(shown, l)])),
            }
        }
        by_key.retain(|(_, labels)| labels.len() > 1);
        by_key
    }

    /// #926: two items of one menu with the same access key, and the key ran only the first. In
    /// English, and in every catalog where two of them keep the English letter (written in
    /// brackets after the text, `(&A)`, or underlined in it): there the clash is the English
    /// one's. A letter a translation chose itself is its translators' to keep apart.
    #[test]
    fn no_two_items_of_a_menu_share_an_access_key() {
        let menus = every_menu();
        let mut found = Vec::new();
        for level in &menus {
            for (key, labels) in clashes(level, None) {
                found.push(format!("en, {}: {key} in {labels:?}", level.name));
            }
        }
        for (lang, code, _) in crate::i18n::languages() {
            for level in &menus {
                for (key, labels) in clashes(level, Some(lang)) {
                    let english = labels
                        .iter()
                        .filter(|(_, source)| access_key(source).as_ref() == Some(&key))
                        .count();
                    if english > 1 {
                        found.push(format!("{code}, {}: {key} in {labels:?}", level.name));
                    }
                }
            }
        }
        found.sort();
        found.dedup();
        assert!(found.is_empty(), "{}", found.join(", "));
    }
}
