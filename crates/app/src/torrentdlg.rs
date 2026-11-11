//! Add torrent (M15, D-014; ours — IDM has no torrents). A magnet link or a .torrent file, its
//! file list once known (a magnet asks the swarm for it), which files to download, and where.
//! It follows the app's mode (dark or light) like Add URL: glyphs are rendered for the main
//! window's mode, so a light dialog in dark mode would get heavy text.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Color32, Pos2, Rect, Stroke, Ui, pos2, vec2};
use mdm_torrent::{Lookup, LookupStatus, Meta, Source};

use crate::dialogs::{Outcome, window};
use crate::model::{Category, category_folder};
use crate::torrents::Host;
// `fill` takes English pixels (made where they are on screen), `text` an English pen.
use crate::winui::{self, ButtonKind, fill_l as fill, r, text};

/// How long a magnet may take to get its file list from peers.
pub const METADATA_WAIT: Duration = Duration::from_secs(180);
const ROW: f32 = 17.0;

type Slot = Arc<Mutex<Option<Result<Meta, String>>>>;

crate::theme::themed! {
    // The lists' header colour (winui::LIST_HEADER).
    LIST_HEAD => header;
    LIST_LINE => table_line;
}

/// What Download Now / Later adds.
pub struct TorrentAdd {
    pub meta: Meta,
    /// The folder chosen; a torrent of several files gets its own folder inside.
    pub dir: PathBuf,
    /// Chosen files (indexes into `meta.files`); None: all.
    pub files: Option<Vec<usize>>,
    pub category: Category,
    pub start_now: bool,
    pub from_magnet: bool,
    /// Download in order (#39): the files' start first, to watch while it downloads.
    pub in_order: bool,
    /// The queue it waits in, chosen already (a paired device's, #141); None: Download Later
    /// asks as it does for any download.
    pub queue: Option<usize>,
    /// No progress window when it starts (sent from a paired device).
    pub quiet: bool,
}

pub struct TorrentDialog {
    source: String,
    folder: String,
    folder_edited: bool,
    downloads: PathBuf,
    category_dirs: Vec<(String, String)>,
    /// The torrents the list already has (and the session must keep).
    known: HashSet<String>,
    host: Host,
    lookup: Option<Slot>,
    /// The look-up running (Stop ends it).
    lookup_task: Option<tokio::task::JoinHandle<()>>,
    /// When the lookup started (the loading state counts the seconds).
    started: Option<Instant>,
    /// How the lookup goes: peers found, from where, how many answered (#92).
    progress: Option<Arc<Lookup>>,
    meta: Option<Meta>,
    chosen: Vec<bool>,
    /// The file tree's scroll and selection.
    rows: winui::RowList,
    /// The file tree (built when the list arrives) and which folders are open.
    tree: Vec<Node>,
    open: Vec<bool>,
    error: Option<String>,
    from_magnet: bool,
    focused: bool,
    start_now: Option<bool>,
    /// Download in order (#39).
    in_order: bool,
    /// Asking to create a missing folder.
    asking: crate::dialogs::FolderGate<()>,
    /// Asking whether to use files of the torrent's names already in the folder (#40).
    asking_files: crate::dialogs::FilesGate<()>,
    /// A .torrent on the web (#39, Add URL or the browser): the request it is fetched with
    /// (the browser's cookies). One typed in the box goes as MDM's own downloads do.
    remote: Option<crate::torrents::Remote>,
    /// The clipboard's magnet link, read off the window's thread (#661).
    clip: crate::clipboard::Prefill,
}

/// The category of a torrent: its largest file's.
/// Every torrent goes to the Torrents category and its folder (D-021), whatever it holds.
pub fn category_of(_meta: &Meta) -> Category {
    Category::Torrents
}

/// A turning ring: the loading sign while something is fetched (also the video dialog's).
pub(crate) fn spinner(ui: &Ui, center: Pos2, radius: f32) {
    let t = ui.input(|i| i.time) as f32;
    let start = t * 5.0;
    let n = 24;
    let points: Vec<Pos2> = (0..=n)
        .map(|k| {
            let a = start + k as f32 / n as f32 * std::f32::consts::PI * 1.5;
            pos2(center.x + radius * a.cos(), center.y + radius * a.sin())
        })
        .collect();
    let p = ui.painter();
    p.circle_stroke(center, radius, Stroke::new(2.0, LIST_LINE()));
    p.line(points, Stroke::new(2.0, winui::accent()));
    ui.ctx().request_repaint();
}

/// How long a magnet's lookup goes on before it says what may be wrong (#92).
const HINT_AFTER: u64 = 30;

/// What a magnet's lookup shows while it waits (#92): what is happening now, where the peers
/// came from, and after `HINT_AFTER` seconds, what may be wrong.
fn lookup_lines(s: &LookupStatus, secs: u64) -> (String, String, Option<&'static str>) {
    use crate::i18n::{fmt, t, tn};
    let now = if s.connected > 0 {
        tn(
            s.connected as u64,
            // i18n: while a magnet link's file list is fetched; {n} = peers that answered
            "Connected to {n} peer, getting the file list...",
            "Connected to {n} peers, getting the file list...",
        )
    } else if s.peers > 0 {
        tn(
            s.peers as u64,
            // i18n: while a magnet link's file list is fetched; {n} = peers found so far (none answered yet)
            "Found {n} peer, connecting...",
            "Found {n} peers, connecting...",
        )
    } else {
        // i18n: while a magnet link's file list is fetched, before any peer is found
        t("Looking for peers...").to_owned()
    };
    // i18n: a number of peers (computers sharing a torrent), after where they came from
    let peers = |n: usize| tn(n as u64, "{n} peer", "{n} peers");
    let mut parts = vec![if s.trackers > 0 {
        fmt(
            &tn(
                s.trackers as u64,
                // i18n: where a magnet's peers came from: {n} = trackers asked; {peers} = "3 peers"
                "{n} tracker: {peers}",
                "{n} trackers: {peers}",
            ),
            &[("peers", &peers(s.from_trackers))],
        )
    } else {
        // i18n: the magnet link names no tracker (a server that lists a torrent's peers)
        t("No trackers").to_owned()
    }];
    parts.push(if s.dht {
        fmt(
            &tn(
                s.dht_nodes as u64,
                // i18n: where a magnet's peers came from: the DHT; {n} = the computers it knows now; {peers} = "3 peers"
                "DHT, {n} node: {peers}",
                "DHT, {n} nodes: {peers}",
            ),
            &[("peers", &peers(s.from_dht))],
        )
    } else {
        // i18n: the DHT (finding peers without trackers) is turned off in Torrent settings
        t("DHT off").to_owned()
    });
    if s.from_lsd > 0 {
        parts.push(fmt(
            // i18n: where a magnet's peers came from: computers on your own network; {peers} = "3 peers"
            t("Local network: {peers}"),
            &[("peers", &peers(s.from_lsd))],
        ));
    }
    let sources = parts.join("   ·   ");
    let hint = if secs < HINT_AFTER || s.connected > 0 {
        None
    } else if s.peers > 0 {
        // i18n: a magnet link's lookup, after 30 seconds: peers are known but none answered
        Some(t(
            "Peers were found, but none answered yet. A firewall or a VPN may be blocking the connections.",
        ))
    } else if !s.dht && s.trackers == 0 {
        // i18n: a magnet link's lookup, after 30 seconds: it can't find any peer at all
        Some(t(
            "This magnet link names no trackers and the DHT is off, so no peer can be found.",
        ))
    } else if s.dht && s.dht_nodes == 0 {
        // i18n: a magnet link's lookup, after 30 seconds: the DHT got no answer from anyone
        Some(t(
            "The DHT reached no one. A firewall, a VPN or the network may be blocking UDP, which the DHT and most trackers use.",
        ))
    } else {
        // i18n: a magnet link's lookup, after 30 seconds without a peer
        Some(t(
            "No peers yet. Few people may share this torrent, or a firewall, a VPN or the network may be blocking BitTorrent.",
        ))
    };
    (now, sources, hint)
}

impl TorrentDialog {
    /// The magnet link, .torrent path or address in its box.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// `source`: a magnet link, a .torrent path or a .torrent's web address (`remote`: how to
    /// ask for it) to look up at once, or None (the clipboard's magnet, if any, is offered and
    /// looked up on the frame it comes: the clipboard is read off the window's thread).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source: Option<String>,
        remote: Option<crate::torrents::Remote>,
        host: Host,
        known: HashSet<String>,
        downloads: PathBuf,
        category_dirs: Vec<(String, String)>,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
    ) -> Self {
        let clip = if source.is_none() {
            crate::clipboard::Prefill::ask()
        } else {
            Default::default()
        };
        let source = source.unwrap_or_default();
        let folder = category_folder(Category::Torrents, &downloads, &category_dirs);
        let mut d = TorrentDialog {
            source,
            folder: folder.display().to_string(),
            folder_edited: false,
            downloads,
            category_dirs,
            known,
            host,
            lookup: None,
            lookup_task: None,
            started: None,
            progress: None,
            meta: None,
            chosen: Vec::new(),
            rows: Default::default(),
            tree: Vec::new(),
            open: Vec::new(),
            error: None,
            from_magnet: false,
            focused: false,
            start_now: None,
            in_order: false,
            asking: Default::default(),
            asking_files: Default::default(),
            remote,
            clip,
        };
        if !d.source.is_empty() {
            d.look_up(rt, ctx);
        }
        d
    }

    /// The clipboard's magnet link in the box, when it holds one and the box is still empty
    /// (nothing typed meanwhile); whether it was put there (and is to be looked up).
    fn fill_from_clipboard(&mut self, clip: &str) -> bool {
        let fill = self.source.is_empty() && Source::magnet(clip).is_some();
        if fill {
            clip.trim().clone_into(&mut self.source);
        }
        fill
    }

    fn look_up(&mut self, rt: &tokio::runtime::Handle, ctx: &egui::Context) {
        let text = self.source.trim().to_owned();
        let magnet = Source::magnet(&text);
        self.from_magnet = magnet.is_some();
        // A .torrent file is read with the rest of the look-up, off the window's thread (a
        // path on a slow share froze every window).
        let path = text.trim_matches('"').to_owned();
        // A web address: fetched (#39), with the request it came with when it is that one.
        let web = url::Url::parse(&text)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .map(|url| match self.remote.clone().filter(|r| r.url == url) {
                Some(r) => r,
                None => crate::torrents::Remote {
                    url,
                    parts: crate::app::RequestParts {
                        headers: Default::default(),
                        opts: crate::settings::own_net(),
                    },
                },
            });
        let not_found = crate::i18n::t("Enter a magnet link or choose a .torrent file").to_owned();
        let slot: Slot = Arc::new(Mutex::new(None));
        let progress = Arc::new(Lookup::default());
        let (out, host, known, ctx, watch) = (
            slot.clone(),
            self.host.clone(),
            self.known.clone(),
            ctx.clone(),
            progress.clone(),
        );
        let task = rt.spawn(async move {
            let src = match (magnet, web) {
                (Some(m), _) => m,
                (None, Some(web)) => match crate::torrents::fetch_torrent(web).await {
                    Ok(bytes) => Source::File(bytes),
                    Err(e) => {
                        *out.lock().unwrap() = Some(Err(e));
                        ctx.request_repaint();
                        return;
                    }
                },
                // At most mdm_torrent::MAX_TORRENT_FILE (#40): a bigger file isn't a torrent.
                (None, None) => match tokio::task::spawn_blocking(move || {
                    mdm_torrent::read_file(std::path::Path::new(&path))
                })
                .await
                {
                    Ok(Ok(bytes)) => Source::File(bytes),
                    Ok(Err(e)) if e.kind() == std::io::ErrorKind::InvalidData => {
                        let e = crate::errors::torrent(&mdm_torrent::TorrentError::NotATorrent);
                        *out.lock().unwrap() = Some(Err(e));
                        ctx.request_repaint();
                        return;
                    }
                    _ => {
                        *out.lock().unwrap() = Some(Err(not_found));
                        ctx.request_repaint();
                        return;
                    }
                },
            };
            let res = match host.session(known).await {
                Ok(t) => t
                    .metadata(&src, METADATA_WAIT, Some(&watch))
                    .await
                    .map_err(|e| crate::errors::torrent(&e)),
                Err(e) => Err(e),
            };
            *out.lock().unwrap() = Some(res);
            ctx.request_repaint();
        });
        self.lookup = Some(slot);
        self.lookup_task = Some(task);
        self.started = Some(Instant::now());
        self.progress = Some(progress);
        self.meta = None;
        self.rows = Default::default();
        self.error = None;
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        rt: &tokio::runtime::Handle,
    ) -> Outcome<TorrentAdd> {
        if let Some(clip) = self.clip.take()
            && self.fill_from_clipboard(&clip)
        {
            self.look_up(rt, ctx);
        }
        let looked = self.lookup.as_ref().and_then(|s| s.lock().unwrap().take());
        if let Some(res) = looked {
            self.lookup = None;
            self.started = None;
            match res {
                Ok(meta) if self.known.contains(&meta.info_hash) => {
                    self.error =
                        Some(crate::i18n::t("This torrent is already in the download list").into());
                }
                Ok(meta) => {
                    self.chosen = meta.files.iter().map(|f| !f.padding).collect();
                    self.tree = build_tree(&meta);
                    // Open the first two levels; deeper folders start closed.
                    self.open = self.tree.iter().map(|n| n.depth < 2).collect();
                    if !self.folder_edited {
                        let dir = category_folder(
                            category_of(&meta),
                            &self.downloads,
                            &self.category_dirs,
                        );
                        self.folder = dir.display().to_string();
                    }
                    self.meta = Some(meta);
                }
                Err(e) => self.error = Some(e),
            }
        }
        let out = window(
            ctx,
            "add-torrent",
            "Add torrent",
            [600.0, 460.0],
            false,
            winui::BG(),
            |ui, o| self.body(ui, o, rt),
        );
        let folder = crate::dialogs::resolve_path(&self.folder);
        let out = crate::dialogs::ensure_folder(ctx, &mut self.asking, out, &folder);
        // Then: files of the torrent's names already there would be written into (#40).
        // Looked for off the window's thread (#743), with what the torrent holds as it is now.
        let meta = &self.meta;
        let out =
            crate::dialogs::ensure_files_free(ctx, &mut self.asking_files, out, &folder, || {
                let (meta, there) = (meta.clone(), folder.clone());
                move || meta.map_or_else(Vec::new, |m| m.existing_in(&there))
            });
        match out {
            Outcome::Open => Outcome::Open,
            Outcome::Closed => Outcome::Closed,
            Outcome::Done(()) => {
                let Some(meta) = self.meta.take() else {
                    return Outcome::Closed;
                };
                let all = self
                    .chosen
                    .iter()
                    .zip(&meta.files)
                    .all(|(c, f)| *c || f.padding);
                let files = (!all).then(|| {
                    self.chosen
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| **c)
                        .map(|(i, _)| i)
                        .collect()
                });
                Outcome::Done(TorrentAdd {
                    category: category_of(&meta),
                    meta,
                    dir: crate::dialogs::resolve_path(&self.folder),
                    files,
                    start_now: self.start_now.unwrap_or(true),
                    from_magnet: self.from_magnet,
                    in_order: self.in_order,
                    queue: None,
                    quiet: false,
                })
            }
        }
    }

    fn body(&mut self, ui: &mut Ui, o: Pos2, rt: &tokio::runtime::Handle) -> Outcome<()> {
        let p = ui.painter().clone();
        let busy = self.lookup.is_some();
        let kind = |on: bool, default: bool| match (on, default) {
            (false, _) => ButtonKind::Disabled,
            (true, true) => ButtonKind::Default,
            (true, false) => ButtonKind::Normal,
        };

        // The source: a magnet link, or a .torrent file chosen with "...".
        // i18n: noun: label before the torrent file or magnet link
        let source_label = crate::i18n::t("Torrent");
        let folder_label = crate::i18n::t("Save to");
        // The fields start after the longer label (German "Speichern unter").
        let fx = winui::label_column(&p, 11.0, &[source_label, folder_label], 70.0);
        text(&p, o.x + 11.0, o.y + 23.0, source_label, winui::TEXT());
        let edit = winui::edit(
            ui,
            r(o, fx, 9.0, 480.0 - fx, 21.0),
            &mut self.source,
            !busy,
            false,
        );
        if !self.focused {
            edit.request_focus();
            self.focused = true;
        }
        let browse = winui::button(
            ui,
            r(o, 486.0, 9.0, 22.0, 21.0),
            "...",
            kind(!busy, false),
            false,
            None,
        )
        .clicked();
        if browse && !busy {
            // i18n: the kind of file the Open dialog lists
            let kind = crate::i18n::t("Torrent files").to_owned();
            let title = crate::i18n::t("Open torrent file").to_owned();
            winui::start_pick(ui.ctx(), "torrent-file", move || {
                rfd::FileDialog::new()
                    .add_filter(kind, &["torrent"])
                    .set_title(title)
                    .pick_file()
            });
        }
        if let Some(f) = winui::picked(ui.ctx(), "torrent-file") {
            self.source = f.display().to_string();
            self.look_up(rt, ui.ctx());
        }
        let can_look = !busy && !self.source.trim().is_empty();
        // While it looks up, the button stops it (a magnet with no peers took up to 3 minutes).
        let look = winui::button(
            ui,
            r(o, 514.0, 9.0, 75.0, 21.0),
            // i18n: button: stop looking the torrent up
            if busy {
                crate::i18n::mark("Stop")
            } else {
                crate::i18n::mark("Look up")
            },
            kind(can_look || busy, self.meta.is_none()),
            false,
            None,
        )
        .clicked()
            || (edit.has_focus()
                && can_look
                && winui::enter_pressed(ui)
                && winui::take_key(ui, egui::Key::Enter));
        if look && busy {
            if let Some(t) = self.lookup_task.take() {
                t.abort();
            }
            self.lookup = None;
            self.started = None;
        } else if look && can_look {
            self.look_up(rt, ui.ctx());
        }

        // Status: looking up (with a spinner and the seconds) / name and size / what went wrong.
        let line = o.y + 50.0;
        if busy {
            spinner(ui, pos2(o.x + 18.0, line - 4.0), 6.0);
            let secs = self.started.map_or(0, |s| s.elapsed().as_secs());
            let msg = if self.from_magnet {
                crate::i18n::fmt(
                    // i18n: still going on; {n} = the seconds it has taken so far ("s" = seconds)
                    crate::i18n::t("Getting the file list from peers...  {n} s"),
                    &[("n", &crate::i18n::integer(secs))],
                )
            } else {
                crate::i18n::t("Reading the torrent...").to_owned()
            };
            text(&p, o.x + 31.0, line, &msg, winui::TEXT());
        } else if let Some(e) = &self.error {
            let (rect, cut) = winui::text_fit(&p, o.x + 11.0, line, e, winui::ERROR(), o.x + 589.0);
            if cut {
                winui::full_on_hover(ui, rect, "torrent-error", e);
            }
        } else if let Some(m) = &self.meta {
            let shown = m.files.iter().filter(|f| !f.padding).count();
            let summary = crate::i18n::fmt(
                // i18n: the torrent's name, then {files} = "3 files" and {size} = its total size
                crate::i18n::t("{name}  ({files}, {size})"),
                &[
                    ("name", &m.name),
                    (
                        "files",
                        &crate::i18n::tn(shown as u64, "{n} file", "{n} files"),
                    ),
                    ("size", &crate::dialogs::dialog_size(m.total())),
                ],
            );
            let kind = if shown > 1 {
                crate::fileicons::Kind::Folder
            } else {
                crate::fileicons::Kind::File(&m.name)
            };
            crate::fileicons::paint_at(ui, &p, kind, egui::pos2(o.x + 11.0, line - 12.0), 16.0);
            let x = o.x + 11.0 + crate::fileicons::INDENT;
            let (rect, cut) = winui::text_fit(&p, x, line, &summary, winui::TEXT(), o.x + 589.0);
            if cut {
                winui::full_on_hover(ui, rect, "torrent-name", &summary);
            }
        }

        self.file_list(ui, r(o, 11.0, 58.0, 578.0, 262.0));

        // Select all / none, each as wide as its label in this language (#192), and what is chosen.
        let has = self.meta.is_some();
        let [all_at, none_at] = winui::buttons_from_left(
            &p,
            11.0,
            6.0,
            &[("Select all", 90.0), ("Select none", 90.0)],
            160.0,
        )[..] else {
            unreachable!("two buttons")
        };
        if winui::button(
            ui,
            r(o, all_at.0, 328.0, all_at.1, 21.0),
            "Select all",
            kind(has, false),
            false,
            None,
        )
        .clicked()
            && let Some(m) = &self.meta
        {
            self.chosen = m.files.iter().map(|f| !f.padding).collect();
        }
        if winui::button(
            ui,
            r(o, none_at.0, 328.0, none_at.1, 21.0),
            "Select none",
            kind(has, false),
            false,
            None,
        )
        .clicked()
        {
            self.chosen.iter_mut().for_each(|c| *c = false);
        }
        let (count, bytes) = self.selection();
        if has {
            winui::text_right(
                &p,
                o.x + 589.0,
                o.y + 343.0,
                &crate::i18n::fmt(
                    // i18n: the files chosen: {files} = "3 files", {size} = their total size
                    crate::i18n::t("Selected: {files}, {size}"),
                    &[
                        (
                            "files",
                            &crate::i18n::tn(count as u64, "{n} file", "{n} files"),
                        ),
                        ("size", &crate::dialogs::dialog_size(bytes)),
                    ],
                ),
                winui::TEXT(),
            );
        }

        // Where.
        text(&p, o.x + 11.0, o.y + 375.0, folder_label, winui::TEXT());
        if winui::edit(
            ui,
            r(o, fx, 361.0, 561.0 - fx, 21.0),
            &mut self.folder,
            true,
            false,
        )
        .changed()
        {
            self.folder_edited = true;
        }
        if winui::button(
            ui,
            r(o, 567.0, 361.0, 22.0, 21.0),
            "...",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked()
        {
            let from = self.folder.clone();
            winui::start_pick(ui.ctx(), "torrent-folder", move || {
                rfd::FileDialog::new().set_directory(from).pick_folder()
            });
        }
        if let Some(d) = winui::picked(ui.ctx(), "torrent-folder") {
            self.folder = d.display().to_string();
            self.folder_edited = true;
        }
        if let Some(m) = &self.meta {
            let goes = m.folder_in(&crate::dialogs::resolve_path(&self.folder));
            let s = crate::i18n::fmt(
                // i18n: {folder} = the folder the torrent's files are saved in
                crate::i18n::t("Files go to: {folder}"),
                &[("folder", &crate::i18n::ltr(&goes.display().to_string()))],
            );
            let (rect, cut) =
                winui::text_fit(&p, o.x + fx, o.y + 402.0, &s, winui::MUTED(), o.x + 589.0);
            if cut {
                winui::full_on_hover(ui, rect, "torrent-goes", &s);
            }
        }

        // The bottom row, each button as wide as its label in this language (#192).
        let [now_at, later_at, cancel_at] = winui::buttons_to_right(
            &p,
            589.0,
            6.0,
            &[("Download Now", 105.0), ("Later", 91.0), ("Cancel", 91.0)],
            160.0,
        )[..] else {
            unreachable!("three buttons")
        };
        // #39: its files' start first, to watch a video while it downloads (also later, from
        // its right-click menu). Its caption stops before the buttons.
        winui::checkbox_until(
            ui,
            pos2(o.x + 14.0, o.y + 432.0),
            &mut self.in_order,
            // i18n: Add torrent, a checkbox: download the files from their start, to watch a video while it downloads
            "Download in order",
            has,
            Some(o.x + now_at.0),
        );
        let ready = has && count > 0 && !self.folder.trim().is_empty() && !busy;
        let now = winui::button(
            ui,
            r(o, now_at.0, 428.0, now_at.1, 21.0),
            "Download Now",
            kind(ready, true),
            false,
            None,
        )
        .clicked()
            || (ready && winui::enter_pressed(ui));
        let later = winui::button(
            ui,
            r(o, later_at.0, 428.0, later_at.1, 21.0),
            // i18n: button: add it now, start downloading later
            "Later",
            kind(ready, false),
            false,
            None,
        )
        .clicked();
        let cancel = winui::button(
            ui,
            r(o, cancel_at.0, 428.0, cancel_at.1, 21.0),
            "Cancel",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked();
        if cancel || winui::escape_pressed(ui) {
            return Outcome::Closed;
        }
        if ready && (now || later) {
            self.start_now = Some(now);
            return Outcome::Done(());
        }
        Outcome::Open
    }

    fn selection(&self) -> (usize, u64) {
        let Some(m) = &self.meta else { return (0, 0) };
        m.files
            .iter()
            .zip(&self.chosen)
            .filter(|(f, c)| **c && !f.padding)
            .fold((0, 0), |(n, b), (f, _)| (n + 1, b + f.size))
    }

    /// The file tree: folders open and close, every level has a checkbox (a folder's is mixed
    /// when only some files inside are chosen; ticking it chooses or clears them all); sizes
    /// on the right; scrolled by the wheel a row at a time. While looking up: a spinner.
    fn file_list(&mut self, ui: &mut Ui, list: Rect) {
        let p = ui.painter().clone();
        winui::solid(&p, list, winui::FIELD_EDGE());
        // Laid out in English (`list` comes on screen); each part made where it is on screen:
        // names from the leading edge, sizes at the trailing one, folders indenting from the
        // leading edge.
        let list = winui::rect(list);
        let inner = list.shrink(1.0);
        p.rect_filled(winui::rect(inner), 0.0, winui::FIELD());
        let head = Rect::from_min_size(inner.min, vec2(inner.width(), 20.0));
        p.rect_filled(winui::rect(head), 0.0, LIST_HEAD());
        fill(
            &p,
            head.left(),
            head.bottom() - 1.0,
            head.right() - 1.0,
            head.bottom() - 1.0,
            LIST_LINE(),
        );
        text(
            &p,
            head.left() + 25.0,
            head.top() + 14.0,
            // i18n: column title: file name
            crate::i18n::t("Name"),
            winui::TEXT(),
        );
        winui::text_right(
            &p,
            head.right() - 24.0,
            head.top() + 14.0,
            // i18n: column title: file size
            crate::i18n::t("Size"),
            winui::TEXT(),
        );
        let body = Rect::from_min_max(pos2(inner.left(), head.bottom()), inner.max);

        if self.lookup.is_some() {
            let c = body.center();
            let centred = |s: &str, y: f32, color: Color32| {
                let w = winui::text_width(&p, s);
                text(&p, c.x - w / 2.0, y, s, color);
            };
            let progress = self.progress.as_ref().filter(|_| self.from_magnet);
            let Some(progress) = progress else {
                spinner(ui, pos2(c.x, c.y - 14.0), 12.0);
                centred(
                    crate::i18n::t("Reading the file list..."),
                    c.y + 22.0,
                    winui::MUTED(),
                );
                return;
            };
            // A magnet (#92): what is happening, where the peers come from, and after a while
            // without one, what may be wrong.
            spinner(ui, pos2(c.x, c.y - 34.0), 12.0);
            let secs = self.started.map_or(0, |s| s.elapsed().as_secs());
            let (now, sources, hint) = lookup_lines(&progress.status(), secs);
            centred(&now, c.y + 2.0, winui::TEXT());
            centred(&sources, c.y + 22.0, winui::MUTED());
            if let Some(hint) = hint {
                winui::paragraph(
                    &p,
                    body.left() + 24.0,
                    c.y + 52.0,
                    hint,
                    winui::MUTED(),
                    body.right() - 24.0,
                    16.0,
                );
            }
            return;
        }
        if self.meta.is_none() {
            // i18n: "Look up" is the button that reads the torrent: use its translation
            let note =
                crate::i18n::t("Paste a magnet link or choose a .torrent file, then Look up.");
            let w = winui::text_width(&p, note);
            text(
                &p,
                body.center().x - w / 2.0,
                body.center().y + 4.0,
                note,
                winui::MUTED(),
            );
            return;
        }
        let rows = visible_rows(&self.tree, &self.open);
        // What a screen reader calls the tree (its rows carry each file's name).
        self.rows.name = crate::i18n::t("Files");
        // The shared row list over the body (its frame on the list's): a click selects, the
        // box, Space or a double-click on a file ticks, a double-click or the arrow opens a
        // folder, Right / Left open and close the selected one.
        let (tree, open, chosen) = (&self.tree, &self.open, &self.chosen);
        let mut toggle = None;
        let mut flip_open = None;
        let ev = winui::row_list(
            ui,
            "torrent-tree",
            winui::rect(body.expand(1.0)),
            rows.len(),
            ROW,
            &mut self.rows,
            |ui, clip, k, row, look| {
                let n = rows[k];
                let node = &tree[n];
                let top = row.top();
                let x = row.left() + 6.0 + 16.0 * node.depth as f32;
                let is_dir = node.file.is_none();
                if is_dir {
                    let arrow = Rect::from_min_size(pos2(x, top), vec2(14.0, ROW));
                    let ar = ui.interact(
                        winui::rect(arrow),
                        egui::Id::unique(("torrent-arrow", n)),
                        winui::pointer_only(),
                    );
                    if ar.clicked() {
                        flip_open = Some(n);
                    }
                    winui::tree_expander(clip, pos2(x + 4.0, top + 5.0), open[n]);
                }
                let state = tri_state(node, chosen);
                let glyph = pos2(x + 16.0, top + 2.0);
                let mut on = state == Tri::All;
                let size = crate::dialogs::dialog_size(node.size);
                look.name = format!("{}, {size}", node.name);
                if winui::row_check(ui, clip, glyph, &mut on, &node.name, ("torrent-node", n)) {
                    toggle = Some(n);
                }
                if state == Tri::Some {
                    winui::row_check_mixed(clip, glyph);
                }
                let size_w = winui::text_width(clip, &size);
                // The file's type icon (a folder's for a folder), then its name.
                let kind = if is_dir {
                    crate::fileicons::Kind::Folder
                } else {
                    crate::fileicons::Kind::File(&node.name)
                };
                crate::fileicons::paint_at(
                    ui,
                    clip,
                    kind,
                    pos2(glyph.x + 19.0, top + (ROW - 16.0) / 2.0),
                    16.0,
                );
                let left = glyph.x + 19.0 + crate::fileicons::INDENT;
                // A name keeps its own direction (cut inside its isolate), whole on hover.
                let name = crate::i18n::isolate(&node.name);
                let max = row.right() - 8.0 - size_w - 12.0;
                winui::fit(
                    ui,
                    clip,
                    left,
                    top + 12.0,
                    &name,
                    look.ink,
                    max,
                    ("torrent-name", n),
                );
                let size_color = if is_dir && !look.selected {
                    winui::MUTED()
                } else {
                    look.ink
                };
                winui::text_right(clip, row.right() - 8.0, top + 12.0, &size, size_color);
            },
        );
        let is_dir = |k: usize| self.tree[rows[k]].file.is_none();
        if let Some(k) = ev.double_clicked {
            if is_dir(k) {
                flip_open = Some(rows[k]);
            } else {
                toggle = Some(rows[k]);
            }
        }
        if let Some(k) = ev.space {
            toggle = Some(rows[k]);
        }
        if ev.focused {
            let tree = &self.tree;
            self.rows.type_to(ui, "torrent-tree", rows.len(), |k| {
                tree[rows[k]].name.clone()
            });
        }
        // Right opens the selected folder, Left closes it.
        if ev.focused
            && let Some(k) = self.rows.selected.filter(|k| *k < rows.len() && is_dir(*k))
        {
            let n = rows[k];
            if (winui::take_key(ui, egui::Key::ArrowRight) && !self.open[n])
                || (winui::take_key(ui, egui::Key::ArrowLeft) && self.open[n])
            {
                flip_open = Some(n);
            }
        }
        if let Some(n) = flip_open {
            self.open[n] = !self.open[n];
        }
        if let Some(n) = toggle {
            // A folder (or file): all of it on, unless it already is all on.
            let on = tri_state(&self.tree[n], &self.chosen) != Tri::All;
            for &f in &self.tree[n].files {
                self.chosen[f] = on;
            }
        }
    }
}

/// Torrent settings (ours, D-018): sharing after a torrent finishes, whether Windows opens
/// magnet links and .torrent files with MDM (both off by default), and the torrent engine's
/// network (#40: what it does on the network is the person's to choose).
pub struct TorrentSettings {
    share: bool,
    assoc: bool,
    net: Net,
    /// The port box's text (digits; empty: any).
    port: String,
    /// Proxy / Socks has a SOCKS5 proxy torrents can use.
    socks: bool,
    /// "Limit uploading to": on, and the KBytes/sec box's text.
    upload_on: bool,
    upload_kb: String,
    /// Seeding limits: on, and their boxes' text (the ratio as typed, "2.00"; minutes).
    stop_ratio_on: bool,
    stop_ratio: String,
    stop_min_on: bool,
    stop_min: String,
}

/// Torrent settings' seeding limits as OK gives them (`Settings::torrent_stop_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seeding {
    pub ratio_on: bool,
    /// Hundredths: 200 is 2.00.
    pub ratio: u32,
    pub minutes_on: bool,
    pub minutes: u32,
}

/// A typed share ratio ("2", "1.5", "0,75") in hundredths, at least 0.01; None when it isn't a
/// number.
fn ratio_hundredths(s: &str) -> Option<u32> {
    let v: f64 = s.trim().replace(',', ".").parse().ok()?;
    (v.is_finite() && v > 0.0).then(|| ((v * 100.0).round() as u32).clamp(1, 1_000_000))
}

/// Torrent settings → its network part: the settings' `torrent_*` fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Net {
    pub incoming: bool,
    pub upnp: bool,
    pub port: u16,
    pub dht: bool,
    pub lsd: bool,
    pub proxy: bool,
    pub blocklist: String,
    pub stop_idle: bool,
}

impl Net {
    pub fn of(s: &crate::settings::Settings) -> Net {
        Net {
            incoming: s.torrent_incoming,
            upnp: s.torrent_upnp,
            port: s.torrent_port,
            dht: s.torrent_dht,
            lsd: s.torrent_lsd,
            proxy: s.torrent_proxy,
            blocklist: s.torrent_blocklist.clone(),
            stop_idle: s.torrent_stop_idle,
        }
    }

    pub fn apply(self, s: &mut crate::settings::Settings) {
        s.torrent_incoming = self.incoming;
        s.torrent_upnp = self.upnp;
        s.torrent_port = self.port;
        s.torrent_dht = self.dht;
        s.torrent_lsd = self.lsd;
        s.torrent_proxy = self.proxy;
        s.torrent_blocklist = self.blocklist.trim().to_owned();
        s.torrent_stop_idle = self.stop_idle;
    }
}

/// What OK gives: keep sharing, associations, the network, the upload limit (on, KBytes/sec).
pub struct TorrentChoice {
    pub share: bool,
    pub assoc: bool,
    pub net: Net,
    pub upload: (bool, u64),
    pub seeding: Seeding,
}

impl TorrentSettings {
    pub fn new(s: &crate::settings::Settings) -> Self {
        let net = Net::of(s);
        TorrentSettings {
            share: s.torrent_keep_sharing,
            assoc: s.torrent_associations,
            port: if net.port == 0 {
                String::new()
            } else {
                net.port.to_string()
            },
            net,
            socks: s.socks5_proxy().is_some(),
            upload_on: s.torrent_upload_on,
            upload_kb: s.torrent_upload_kb.max(1).to_string(),
            stop_ratio_on: s.torrent_stop_ratio_on,
            stop_ratio: format!("{:.2}", f64::from(s.torrent_stop_ratio.max(1)) / 100.0),
            stop_min_on: s.torrent_stop_minutes_on,
            stop_min: s.torrent_stop_minutes.max(1).to_string(),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<TorrentChoice> {
        window(
            ctx,
            "torrent-settings",
            "Torrent settings",
            [470.0, 492.0],
            false,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                winui::checkbox(
                    ui,
                    pos2(o.x + 14.0, o.y + 16.0),
                    &mut self.share,
                    "Keep sharing torrents after they finish",
                    true,
                );
                winui::fit(
                    ui,
                    &p,
                    o.x + 32.0,
                    o.y + 44.0,
                    crate::i18n::t("Off: a finished torrent stops, like any download."),
                    winui::MUTED(),
                    o.x + 456.0,
                    ("torrent-settings", 1),
                );
                // Seeding limits (#39): only while sharing is on; off by default.
                let share = self.share;
                winui::checkbox_until(
                    ui,
                    pos2(o.x + 32.0, o.y + 60.0),
                    &mut self.stop_ratio_on,
                    // i18n: Torrent settings, under "Keep sharing": stop sharing a finished torrent once its share ratio (sent ÷ received) reaches the number in the box after it
                    "Stop sharing at a share ratio of",
                    share,
                    Some(326.0),
                );
                winui::edit_right(
                    ui,
                    r(o, 330.0, 57.0, 60.0, 22.0),
                    &mut self.stop_ratio,
                    share && self.stop_ratio_on,
                );
                self.stop_ratio
                    .retain(|c| c.is_ascii_digit() || c == '.' || c == ',');
                self.stop_ratio.truncate(6);
                winui::checkbox_until(
                    ui,
                    pos2(o.x + 32.0, o.y + 84.0),
                    &mut self.stop_min_on,
                    // i18n: Torrent settings, under "Keep sharing": or stop sharing a finished torrent after sharing it for the number of minutes in the box after it
                    "Stop sharing after sharing for",
                    share,
                    Some(326.0),
                );
                winui::edit_right(
                    ui,
                    r(o, 330.0, 81.0, 60.0, 22.0),
                    &mut self.stop_min,
                    share && self.stop_min_on,
                );
                self.stop_min.retain(|c| c.is_ascii_digit());
                self.stop_min.truncate(6);
                winui::fit(
                    ui,
                    &p,
                    o.x + 396.0,
                    o.y + 96.0,
                    // i18n: Torrent settings: the unit after the minutes box
                    crate::i18n::t("minutes"),
                    if share && self.stop_min_on {
                        winui::TEXT()
                    } else {
                        winui::MUTED()
                    },
                    o.x + 462.0,
                    ("torrent-settings", 8),
                );
                winui::checkbox(
                    ui,
                    pos2(o.x + 14.0, o.y + 108.0),
                    &mut self.assoc,
                    "Open magnet links and .torrent files with Mister Download Manager",
                    true,
                );
                winui::fit(
                    ui,
                    &p,
                    o.x + 32.0,
                    o.y + 136.0,
                    // What the switch does is the system's: Windows asks per account, macOS and
                    // Linux register MDM as the handler.
                    if cfg!(windows) {
                        crate::i18n::t(
                            "For your Windows account only. Another app chosen in \"Open with\" stays first.",
                        )
                    } else {
                        // i18n: Torrent settings, under the switch that makes MDM open magnet links and .torrent files (macOS, Linux)
                        crate::i18n::t(
                            "For your account only. Another app set as the default stays first.",
                        )
                    },
                    winui::MUTED(),
                    o.x + 456.0,
                    ("torrent-settings", 2),
                );
                winui::etched_h(&p, o.x + 14.0, o.y + 150.0, 442.0);
                let n = &mut self.net;
                winui::checkbox(
                    ui,
                    pos2(o.x + 14.0, o.y + 160.0),
                    &mut n.incoming,
                    // i18n: Torrent settings: other peers may connect to this computer
                    "Accept incoming connections",
                    true,
                );
                winui::text_right(
                    &p,
                    o.x + 380.0,
                    o.y + 174.0,
                    // i18n: Torrent settings: the port to listen on (empty: any)
                    crate::i18n::t("Port:"),
                    winui::TEXT(),
                );
                winui::edit(
                    ui,
                    r(o, 386.0, 158.0, 70.0, 22.0),
                    &mut self.port,
                    n.incoming,
                    false,
                );
                self.port.retain(|c| c.is_ascii_digit());
                self.port.truncate(5);
                winui::checkbox(
                    ui,
                    pos2(o.x + 32.0, o.y + 184.0),
                    &mut n.upnp,
                    // i18n: Torrent settings: ask the router (UPnP) to forward the port
                    "Map the port on the router (UPnP)",
                    n.incoming,
                );
                winui::checkbox(
                    ui,
                    pos2(o.x + 14.0, o.y + 208.0),
                    &mut n.dht,
                    // i18n: Torrent settings: the DHT, the network peers use to find each other
                    "Find peers through the DHT",
                    true,
                );
                winui::checkbox(
                    ui,
                    pos2(o.x + 14.0, o.y + 232.0),
                    &mut n.lsd,
                    // i18n: Torrent settings: local peer discovery
                    "Find peers on the local network",
                    true,
                );
                winui::fit(
                    ui,
                    &p,
                    o.x + 32.0,
                    o.y + 260.0,
                    // i18n: Torrent settings, under "Find peers on the local network"
                    crate::i18n::t(
                        "It tells the computers on your network which torrents you have.",
                    ),
                    winui::MUTED(),
                    o.x + 456.0,
                    ("torrent-settings", 3),
                );
                // Asked for once and since gone from Proxy / Socks: it stays on (and torrents
                // don't start) until turned off here.
                let can_proxy = self.socks || n.proxy;
                let proxy = winui::checkbox(
                    ui,
                    pos2(o.x + 14.0, o.y + 274.0),
                    &mut n.proxy,
                    // i18n: Torrent settings: use the SOCKS5 proxy set in Options → Proxy / Socks
                    "Use the proxy (Options → Proxy / Socks, SOCKS5)",
                    can_proxy,
                );
                winui::why_disabled(
                    proxy,
                    !can_proxy,
                    // i18n: why "Use the proxy" is greyed in Torrent settings
                    "Set a socks5:// proxy in Options → Proxy / Socks first",
                );
                winui::fit(
                    ui,
                    &p,
                    o.x + 32.0,
                    o.y + 302.0,
                    // i18n: Torrent settings, under "Use the proxy"
                    crate::i18n::t(
                        "With it, incoming connections, the DHT and local peers are off.",
                    ),
                    winui::MUTED(),
                    o.x + 456.0,
                    ("torrent-settings", 4),
                );
                winui::checkbox(
                    ui,
                    pos2(o.x + 14.0, o.y + 316.0),
                    &mut n.stop_idle,
                    // i18n: Torrent settings: stop listening when nothing needs the torrent engine
                    "Stop the torrent engine when no torrent is running",
                    true,
                );
                winui::checkbox_until(
                    ui,
                    pos2(o.x + 14.0, o.y + 340.0),
                    &mut self.upload_on,
                    // i18n: Torrent settings: an upload speed limit for all torrents together, followed by a box with the speed and "KBytes/sec"
                    "Limit uploading for all torrents to",
                    true,
                    Some(326.0),
                );
                winui::edit_right(
                    ui,
                    r(o, 330.0, 337.0, 60.0, 22.0),
                    &mut self.upload_kb,
                    self.upload_on,
                );
                self.upload_kb.retain(|c| c.is_ascii_digit());
                self.upload_kb.truncate(7);
                winui::fit(
                    ui,
                    &p,
                    o.x + 396.0,
                    o.y + 352.0,
                    crate::i18n::t("KBytes/sec"),
                    if self.upload_on {
                        winui::TEXT()
                    } else {
                        winui::MUTED()
                    },
                    o.x + 462.0,
                    ("torrent-settings", 7),
                );
                winui::fit(
                    ui,
                    &p,
                    o.x + 14.0,
                    o.y + 378.0,
                    // i18n: Torrent settings: the address of an IP blocklist (empty: none)
                    crate::i18n::t("Block the addresses in this list (P2P or eMule format):"),
                    winui::TEXT(),
                    o.x + 456.0,
                    ("torrent-settings", 5),
                );
                winui::edit(
                    ui,
                    r(o, 14.0, 384.0, 442.0, 22.0),
                    &mut n.blocklist,
                    true,
                    false,
                );
                winui::fit(
                    ui,
                    &p,
                    o.x + 14.0,
                    o.y + 424.0,
                    // i18n: Torrent settings, at the bottom
                    crate::i18n::t(
                        "Fetched again every 7 days. Changes apply when no torrent is running.",
                    ),
                    winui::MUTED(),
                    o.x + 456.0,
                    ("torrent-settings", 6),
                );
                let ok = winui::button(
                    ui,
                    r(o, 298.0, 460.0, 77.0, 21.0),
                    "OK",
                    ButtonKind::Default,
                    false,
                    None,
                )
                .clicked();
                let cancel = winui::button(
                    ui,
                    r(o, 381.0, 460.0, 77.0, 21.0),
                    "Cancel",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked();
                if cancel || winui::escape_pressed(ui) {
                    Outcome::Closed
                } else if ok || winui::enter_pressed(ui) {
                    let mut net = self.net.clone();
                    net.port = self.port.parse().unwrap_or(0);
                    Outcome::Done(TorrentChoice {
                        share: self.share,
                        assoc: self.assoc,
                        net,
                        upload: (self.upload_on, self.upload_kb.parse().unwrap_or(0).max(1)),
                        seeding: Seeding {
                            ratio_on: self.stop_ratio_on,
                            // Not a number: the default, 2.00.
                            ratio: ratio_hundredths(&self.stop_ratio).unwrap_or(200),
                            minutes_on: self.stop_min_on,
                            minutes: self.stop_min.parse().unwrap_or(0).max(1),
                        },
                    })
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// Right-click → Torrent → Add trackers… (#39): tracker addresses, one a line, for download `id`.
/// OK gives them once each is one MDM may contact (`mdm_torrent::trackers::allowed`).
pub struct AddTrackers {
    pub id: u64,
    name: String,
    text: String,
    error: Option<String>,
    /// The box has had the focus (it takes it once, when the window opens).
    focused: bool,
}

impl AddTrackers {
    pub fn new(id: u64, name: &str) -> Self {
        AddTrackers {
            id,
            name: name.to_owned(),
            text: String::new(),
            error: None,
            focused: false,
        }
    }

    /// The addresses typed, each once, or why one can't be added.
    fn addresses(&self) -> Result<Vec<String>, String> {
        let mut out: Vec<String> = Vec::new();
        for line in self.text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let tracker = url::Url::parse(line)
                .ok()
                .filter(|u| matches!(u.scheme(), "http" | "https" | "udp") && u.host().is_some());
            if tracker.is_none() {
                return Err(crate::i18n::fmt(
                    // i18n: Add trackers: a line that isn't a tracker's address (http, https or udp)
                    crate::i18n::t("Not a tracker address: {line}"),
                    &[("line", &crate::i18n::ltr(line))],
                ));
            }
            // The rule every torrent's trackers follow (#40), told here rather than dropped
            // out of sight.
            if !mdm_torrent::trackers::allowed(line, false) {
                return Err(crate::i18n::fmt(
                    // i18n: Add trackers: MDM never contacts a tracker on this computer or the local network
                    crate::i18n::t(
                        "A tracker on this computer or the local network is never contacted: {line}",
                    ),
                    &[("line", &crate::i18n::ltr(line))],
                ));
            }
            if !out.iter().any(|o| o == line) {
                out.push(line.to_owned());
            }
        }
        if out.is_empty() {
            // i18n: Add trackers: OK with an empty box
            return Err(crate::i18n::t("Enter a tracker's address, one a line").to_owned());
        }
        Ok(out)
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<Vec<String>> {
        window(
            ctx,
            "add-trackers",
            "Add trackers",
            [460.0, 250.0],
            false,
            winui::BG(),
            |ui, o| {
                let p = ui.painter().clone();
                winui::fit(
                    ui,
                    &p,
                    o.x + 14.0,
                    o.y + 24.0,
                    &crate::i18n::fmt(
                        // i18n: Add trackers: above the box; {name} is the torrent's name
                        crate::i18n::t("Trackers to add to {name}, one address a line:"),
                        &[("name", &crate::i18n::isolate(&self.name))],
                    ),
                    winui::TEXT(),
                    o.x + 446.0,
                    ("add-trackers", 1),
                );
                let before = self.text.clone();
                let field = winui::multiline_light(
                    ui,
                    r(o, 14.0, 32.0, 432.0, 140.0),
                    &mut self.text,
                    "add-trackers",
                    true,
                );
                if !std::mem::replace(&mut self.focused, true) {
                    field.request_focus();
                }
                if self.text != before {
                    self.error = None;
                }
                let (line, color) = match &self.error {
                    Some(e) => (e.clone(), winui::ERROR()),
                    None => (
                        // i18n: Add trackers: under the box
                        crate::i18n::t("Kept with the torrent, after the trackers it has.")
                            .to_owned(),
                        winui::MUTED(),
                    ),
                };
                // Two lines' room: a translated reason is long.
                winui::paragraph(&p, o.x + 14.0, o.y + 190.0, &line, color, o.x + 446.0, 14.0);
                let ok = winui::button(
                    ui,
                    r(o, 288.0, 218.0, 77.0, 21.0),
                    "OK",
                    ButtonKind::Default,
                    false,
                    None,
                )
                .clicked();
                let cancel = winui::button(
                    ui,
                    r(o, 371.0, 218.0, 77.0, 21.0),
                    "Cancel",
                    ButtonKind::Normal,
                    false,
                    None,
                )
                .clicked();
                // Enter is a new line in the box: only OK adds them.
                if cancel || winui::escape_pressed(ui) {
                    Outcome::Closed
                } else if ok {
                    match self.addresses() {
                        Ok(list) => Outcome::Done(list),
                        Err(e) => {
                            self.error = Some(e);
                            Outcome::Open
                        }
                    }
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

/// A row of the file tree: a folder or a file.
struct Node {
    name: String,
    depth: usize,
    /// The file's index in `Meta::files`; None for a folder.
    file: Option<usize>,
    children: Vec<usize>,
    /// Every file at or under this node (padding files left out).
    files: Vec<usize>,
    size: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tri {
    None,
    Some,
    All,
}

fn tri_state(node: &Node, chosen: &[bool]) -> Tri {
    let on = node.files.iter().filter(|f| chosen[**f]).count();
    match on {
        0 => Tri::None,
        n if n == node.files.len() => Tri::All,
        _ => Tri::Some,
    }
}

/// The tree of a torrent's files: folders first, then files, each by name. A torrent of several
/// files has its own folder (named after it) at the top, as on disk (`Meta::folder_in`).
fn build_tree(meta: &Meta) -> Vec<Node> {
    let multi = meta.files.iter().filter(|f| !f.padding).count() > 1;
    let mut nodes = vec![Node {
        name: meta.name.clone(),
        depth: 0,
        file: None,
        children: Vec::new(),
        files: Vec::new(),
        size: 0,
    }];
    for (i, f) in meta.files.iter().enumerate().filter(|(_, f)| !f.padding) {
        let parts: Vec<String> = f
            .path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let mut at = 0;
        nodes[0].files.push(i);
        nodes[0].size += f.size;
        for (k, part) in parts.iter().enumerate() {
            let last = k + 1 == parts.len();
            let found = nodes[at]
                .children
                .iter()
                .copied()
                .find(|c| !last && nodes[*c].file.is_none() && nodes[*c].name == *part);
            let next = match found {
                Some(c) => c,
                None => {
                    nodes.push(Node {
                        name: part.clone(),
                        depth: nodes[at].depth + 1,
                        file: last.then_some(i),
                        children: Vec::new(),
                        files: Vec::new(),
                        size: 0,
                    });
                    let id = nodes.len() - 1;
                    nodes[at].children.push(id);
                    id
                }
            };
            nodes[next].files.push(i);
            nodes[next].size += f.size;
            at = next;
        }
    }
    // Folders first, then by name (case-insensitive), at every level.
    for n in 0..nodes.len() {
        let mut kids = std::mem::take(&mut nodes[n].children);
        kids.sort_by(|a, b| {
            let (x, y) = (&nodes[*a], &nodes[*b]);
            x.file
                .is_some()
                .cmp(&y.file.is_some())
                .then_with(|| x.name.to_lowercase().cmp(&y.name.to_lowercase()))
        });
        nodes[n].children = kids;
    }
    if !multi {
        // One file: just its row, no folder above it.
        let only = nodes[0].children.first().copied();
        if let Some(c) = only {
            nodes[c].depth = 0;
            nodes[0].children.clear();
            nodes.swap(0, c);
            // The old root, now at `c`, is unreachable.
            nodes[c].files.clear();
        }
    }
    nodes
}

/// The rows shown: the root, and the children of every open folder, depth first.
fn visible_rows(tree: &[Node], open: &[bool]) -> Vec<usize> {
    let mut out = Vec::new();
    if tree.is_empty() {
        return out;
    }
    let mut stack = vec![0usize];
    while let Some(n) = stack.pop() {
        out.push(n);
        if open[n] {
            stack.extend(tree[n].children.iter().rev().copied());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdm_torrent::FileEntry;

    /// #92: a magnet's wait says what is happening and where the peers came from; after
    /// `HINT_AFTER` without an answer, what may be wrong.
    #[test]
    fn a_magnets_wait_explains_itself() {
        let none = LookupStatus {
            trackers: 21,
            dht: true,
            dht_nodes: 0,
            ..Default::default()
        };
        let (now, sources, hint) = lookup_lines(&none, 5);
        assert_eq!(now, "Looking for peers...");
        assert_eq!(sources, "21 trackers: 0 peers   ·   DHT, 0 nodes: 0 peers");
        assert_eq!(hint, None, "not before {HINT_AFTER} s");
        let (_, _, hint) = lookup_lines(&none, HINT_AFTER);
        assert!(hint.unwrap().starts_with("The DHT reached no one"));
        let (_, _, hint) = lookup_lines(
            &LookupStatus {
                dht_nodes: 300,
                ..none
            },
            HINT_AFTER,
        );
        assert!(hint.unwrap().starts_with("No peers yet"));
        let (_, sources, hint) = lookup_lines(&LookupStatus::default(), HINT_AFTER);
        assert_eq!(sources, "No trackers   ·   DHT off");
        assert!(hint.unwrap().contains("DHT is off"));

        let found = LookupStatus {
            trackers: 1,
            dht: true,
            dht_nodes: 120,
            from_trackers: 1,
            from_dht: 40,
            from_lsd: 2,
            peers: 41,
            ..Default::default()
        };
        let (now, sources, hint) = lookup_lines(&found, HINT_AFTER);
        assert_eq!(now, "Found 41 peers, connecting...");
        assert_eq!(
            sources,
            "1 tracker: 1 peer   ·   DHT, 120 nodes: 40 peers   ·   Local network: 2 peers"
        );
        assert!(hint.unwrap().starts_with("Peers were found"));
        let (now, _, hint) = lookup_lines(
            &LookupStatus {
                connected: 1,
                ..found
            },
            100,
        );
        assert_eq!(now, "Connected to 1 peer, getting the file list...");
        assert_eq!(hint, None, "it is getting there");
    }

    /// Add trackers (#39): one address a line, each once; anything else, or a tracker on this
    /// computer or the local network, is refused.
    #[test]
    fn typed_tracker_addresses() {
        let mut d = AddTrackers::new(1, "x");
        d.text =
            " http://t.example/announce \n\nudp://t.example:1337\nhttp://t.example/announce".into();
        assert_eq!(
            d.addresses().unwrap(),
            ["http://t.example/announce", "udp://t.example:1337"]
        );
        for bad in [
            "udp://",
            "wss://t.example/a",
            "not a url",
            "http://192.168.1.1/a",
            "http://localhost/a",
            " ",
        ] {
            d.text = bad.into();
            assert!(d.addresses().is_err(), "{bad:?}");
        }
    }

    /// Torrent settings' share ratio box: a comma or a point, kept in hundredths.
    #[test]
    fn typed_share_ratios() {
        assert_eq!(ratio_hundredths("2"), Some(200));
        assert_eq!(ratio_hundredths("1.5"), Some(150));
        assert_eq!(ratio_hundredths("0,75"), Some(75));
        assert_eq!(ratio_hundredths("0.001"), Some(1), "at least 0.01");
        assert_eq!(ratio_hundredths(""), None);
        assert_eq!(ratio_hundredths("0"), None);
        assert_eq!(ratio_hundredths("1.2.3"), None);
    }

    fn meta(paths: &[(&str, u64)]) -> Meta {
        Meta {
            name: "Set".into(),
            info_hash: "0".repeat(40),
            files: paths
                .iter()
                .map(|(p, s)| FileEntry {
                    path: PathBuf::from(p),
                    size: *s,
                    padding: false,
                })
                .collect(),
            piece_length: 16384,
            torrent: Vec::new(),
            peers: Vec::new(),
        }
    }

    /// #40: the files that exist where the torrent would write them are asked about, ticked or
    /// not (padding never counts); a several-file torrent writes into its own folder.
    #[test]
    fn files_already_there_are_the_ones_that_exist() {
        let dir = tempfile::tempdir().unwrap();
        let m = meta(&[("a.bin", 1), ("sub/b.bin", 1), ("c.bin", 1)]);
        let base = m.folder_in(dir.path());
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("a.bin"), b"user's").unwrap();
        std::fs::write(base.join("sub/b.bin"), b"user's").unwrap();
        // c.bin isn't there.
        assert_eq!(
            m.existing_in(dir.path()),
            vec![base.join("a.bin"), base.join("sub/b.bin")]
        );
        let mut padded = m.clone();
        padded.files[0].padding = true;
        assert_eq!(padded.existing_in(dir.path()), vec![base.join("sub/b.bin")]);
        assert!(m.existing_in(&dir.path().join("elsewhere")).is_empty());
    }

    #[test]
    fn folders_first_with_sizes_and_mixed_state() {
        let m = meta(&[
            ("readme.txt", 1),
            ("disc 2/b.iso", 20),
            ("disc 1/a.iso", 10),
            ("disc 1/extras/c.txt", 3),
        ]);
        let t = build_tree(&m);
        let open = vec![true; t.len()];
        let names: Vec<(usize, &str)> = visible_rows(&t, &open)
            .into_iter()
            .map(|n| (t[n].depth, t[n].name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (0, "Set"),
                (1, "disc 1"),
                (2, "extras"),
                (3, "c.txt"),
                (2, "a.iso"),
                (1, "disc 2"),
                (2, "b.iso"),
                (1, "readme.txt"),
            ]
        );
        assert_eq!(t[0].size, 34);
        let disc1 = t.iter().position(|n| n.name == "disc 1").unwrap();
        assert_eq!(t[disc1].size, 13);
        let mut chosen = vec![true; 4];
        assert_eq!(tri_state(&t[0], &chosen), Tri::All);
        chosen[3] = false; // c.txt
        assert_eq!(tri_state(&t[disc1], &chosen), Tri::Some);
        assert_eq!(tri_state(&t[0], &chosen), Tri::Some);
        // A closed folder hides what is inside.
        let mut open = open;
        open[disc1] = false;
        assert_eq!(visible_rows(&t, &open).len(), 5);
    }

    #[test]
    fn one_file_is_one_row() {
        let t = build_tree(&meta(&[("debian.iso", 5)]));
        let open = vec![true; t.len()];
        let rows = visible_rows(&t, &open);
        assert_eq!(rows.len(), 1);
        assert_eq!(t[rows[0]].name, "debian.iso");
        assert_eq!(t[rows[0]].files, [0]);
    }

    #[test]
    fn categories_without_their_own_folder_follow_general() {
        // Absolute on every system (a path like "/Volumes/…" is drive-relative on Windows).
        let root = std::env::temp_dir().join("mdm-554");
        let at = |p: &str| root.join(p).display().to_string();
        let dl = root.join("Users/test/Downloads");
        assert_eq!(
            category_folder(Category::Torrents, &dl, &[]),
            dl.join("Torrents")
        );
        let general_only = vec![("General".to_owned(), at("Volumes/Data/Downloads"))];
        let general = root.join("Volumes/Data/Downloads");
        assert_eq!(
            category_folder(Category::General, &dl, &general_only),
            general
        );
        assert_eq!(
            category_folder(Category::Torrents, &dl, &general_only),
            general.join("Torrents")
        );
        assert_eq!(
            category_folder(Category::Video, &dl, &general_only),
            general.join("Video")
        );
        let both = vec![
            ("General".to_owned(), at("Volumes/Data/Downloads")),
            ("Torrents".to_owned(), at("Volumes/NAS/Torrents")),
        ];
        assert_eq!(
            category_folder(Category::Torrents, &dl, &both),
            root.join("Volumes/NAS/Torrents")
        );
        assert_eq!(
            category_folder(Category::Video, &dl, &both),
            general.join("Video")
        );
    }
}
