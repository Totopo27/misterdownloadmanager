//! Link grabber (#37): the links of a page (the browser's "Download all links"), of a list file
//! (Tasks → Import), copied (Tasks → Add batch download from clipboard, #709), dropped or
//! pasted, gathered into packages to tick before they are added. An address that came on its own
//! and turns out to be a web page is read for its links, which join as that page's (#705).
//! It grew out of the Import window and keeps its ways (IDM's "Download all links": every link
//! looked up for its name and size, Check all / Uncheck all, "Hide web pages", OK adds the
//! ticked ones to the queue); the packages come from JDownloader's link grabber: a split
//! archive's volumes are one package, the other links of one page another, the rest "Various
//! files". A package has its folder, an archive password and "Extract when done".

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{Pos2, Rect, Ui, pos2, vec2};
use url::Url;

use crate::dialogs::{Outcome, window};
use crate::model::{Category, category_folder};
use crate::transfer::Imported;
use crate::winui::{self, ButtonKind, LIGHT_BG, LIGHT_TEXT, fill_l as fill, r, text};

const ROW: f32 = 18.0;
/// The top of the window's last row of buttons (Add to queue, Start, Cancel).
const BUTTONS_Y: f32 = 446.0;
/// Links looked up at once, and at once from one host.
const CHECKS: usize = 8;
const CHECKS_PER_HOST: usize = 4;
/// Web pages read for their links at once.
const READS: usize = 2;

crate::theme::themed! {
    // The lists' header colour (winui::LIST_HEADER), as Add torrent's file list.
    LIST_HEAD => header;
    LIST_LINE => table_line;
}

/// What looking a link up found.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkState {
    Unchecked,
    Checking,
    Online,
    /// The server said it isn't there (404, 410).
    Offline(u16),
    /// 401 or 403: a login or the page's cookies are needed.
    NeedsLogin,
    /// The server asked to wait (429, 503).
    Busy,
    /// Anything else, in the interface language.
    Failed(String),
}

/// Which package a link goes in: a split archive's volumes together, else the links of one page,
/// else "Various files".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Volumes of one archive on one host (`movie.part1.rar`, `movie.part2.rar`).
    Volumes {
        host: String,
        base: String,
    },
    /// The page the links were on (its host and path).
    Page(String),
    Various,
    /// In the download list already.
    Known,
}

impl Origin {
    /// Where the link `url` named `name` (found on page `referer`) belongs.
    pub fn of(url: &Url, name: &str, referer: Option<&str>) -> Origin {
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        if let Some(v) = mdm_extract::volume_key(name) {
            return Origin::Volumes { host, base: v.base };
        }
        if let Some(page) = referer.and_then(|r| Url::parse(r).ok())
            && let Some(h) = page.host_str()
        {
            let path = page.path().trim_end_matches('/');
            return Origin::Page(format!("{}{path}", h.to_ascii_lowercase()));
        }
        Origin::Various
    }

    /// Volumes of one archive are one set whatever case their names are in.
    fn same(&self, other: &Origin) -> bool {
        match (self, other) {
            (Origin::Volumes { host: a, base: x }, Origin::Volumes { host: b, base: y }) => {
                a == b && x.to_lowercase() == y.to_lowercase()
            }
            _ => self == other,
        }
    }

    fn name(&self) -> String {
        match self {
            Origin::Volumes { base, .. } => base.clone(),
            Origin::Page(p) => p.clone(),
            // i18n: Link grabber: the package of links that belong to no other
            Origin::Various => crate::i18n::t("Various files").to_owned(),
            // i18n: Link grabber: the package of links the download list has already
            Origin::Known => crate::i18n::t("Already in the list").to_owned(),
        }
    }
}

/// A web page's name rather than a file's (IDM's "Hide HTML files").
pub fn web_page(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    matches!(
        ext,
        "htm" | "html" | "shtml" | "xhtml" | "php" | "asp" | "aspx" | "jsp" | "cgi"
    )
}

pub struct Package {
    origin: Origin,
    pub name: String,
    /// Where its files go (as typed).
    pub folder: String,
    /// Typed by the user: no longer follows the files' category.
    folder_set: bool,
    pub password: String,
    pub extract: bool,
    extract_set: bool,
    pub delete: bool,
    open: bool,
}

pub struct Link {
    pub found: Imported,
    url: Url,
    pkg: usize,
    pub name: String,
    pub size: Option<u64>,
    pub state: LinkState,
    pub on: bool,
    known: bool,
    /// A magnet link (#709): a torrent, added once its files are known; never looked up here.
    magnet: bool,
    check: Option<Check>,
    /// Its look-up found a web page (`ProbeInfo::web_page`).
    page: bool,
    /// Reading it for its links (#705): asked (`Some`), and its answer once it came.
    read: Option<Read>,
}

impl Link {
    /// A web page, as its look-up found or its name says (IDM's "Hide HTML files").
    fn is_page(&self) -> bool {
        self.page || web_page(&self.name)
    }

    /// A web page given on its own (pasted, copied, typed, in a list), not found on a page: one
    /// to read for its links (#705). A page's own links aren't read in turn.
    fn to_read(&self) -> bool {
        self.page
            && self.state == LinkState::Online
            && self.found.referer.is_none()
            && self.read.is_none()
            && !self.known
    }

    /// Its files' category: a torrent's are Torrents, whatever its name says.
    fn category(&self) -> Category {
        if self.magnet {
            Category::Torrents
        } else {
            Category::of(&self.name)
        }
    }
}

/// A magnet link's name: the one it gives (`dn=`), else its torrent's hash.
fn magnet_name(url: &Url) -> String {
    url.query_pairs()
        .find(|(k, _)| k == "dn")
        .map(|(_, v)| v.into_owned())
        .filter(|n| !n.trim().is_empty())
        .or_else(|| crate::transfer::magnet_hash(url.as_str()))
        .unwrap_or_else(|| url.to_string())
}

type Check = Arc<Mutex<Option<Result<mdm_engine::ProbeInfo, LinkState>>>>;
/// A page's links as they were read (`None` until then; empty when it had none or couldn't be
/// read).
type Read = Arc<Mutex<Option<Vec<Imported>>>>;

/// A package's ticked links, as the window hands them over.
pub struct Picked {
    pub name: String,
    pub folder: PathBuf,
    pub password: Option<String>,
    pub extract: bool,
    pub delete: bool,
    /// Each link, the name to save it under (looked up, else the address's), and its size.
    pub links: Vec<(Imported, String, Option<u64>)>,
}

/// What a row of the list is.
#[derive(Clone, Copy, PartialEq)]
enum Row {
    Pkg(usize),
    Link(usize),
}

pub struct Grabber {
    pub packages: Vec<Package>,
    pub links: Vec<Link>,
    source: String,
    rows: winui::RowList,
    pub hide_pages: bool,
    /// The default folders (Options → Save to), as category_folder reads them.
    downloads: PathBuf,
    dirs: Vec<(String, String)>,
    /// Extract when done, and Delete archives after extracting, for a new package (Options).
    delete_default: bool,
    /// The package the box under the list edits.
    current: usize,
    /// One of that box's text boxes had the keys (last frame).
    editing: bool,
}

impl Grabber {
    pub fn new(
        downloads: PathBuf,
        dirs: Vec<(String, String)>,
        hide_pages: bool,
        delete_default: bool,
    ) -> Self {
        Grabber {
            packages: Vec::new(),
            links: Vec::new(),
            source: String::new(),
            rows: Default::default(),
            hide_pages,
            downloads,
            dirs,
            delete_default,
            current: 0,
            editing: false,
        }
    }

    /// More links, from `source` ("Links from the browser", a file's name): each in its
    /// package; one the download list has (`known`) comes unticked under "Already in the list";
    /// a link here already is not added again.
    pub fn add(&mut self, found: Vec<Imported>, source: String, known: impl Fn(&Url) -> bool) {
        // The addresses already here, looked up once: a list of 10,000 compared each link with
        // every other one, and its packages' defaults were worked out again for each (#805).
        let mut here: std::collections::HashSet<Url> =
            self.links.iter().map(|l| address_key(&l.url)).collect();
        let mut settle = Vec::new();
        for d in found {
            let Ok(url) = Url::parse(&d.url) else {
                continue;
            };
            if !here.insert(address_key(&url)) {
                continue;
            }
            let magnet = url.scheme() == "magnet";
            let name = mdm_engine::naming::sanitize(&if magnet {
                magnet_name(&url)
            } else {
                mdm_engine::naming::file_name(None, &url)
            });
            let known = known(&url);
            let origin = if known {
                Origin::Known
            } else {
                Origin::of(&url, &name, d.referer.as_deref())
            };
            let pkg = self.package_for(origin);
            self.links.push(Link {
                found: d,
                url,
                pkg,
                name,
                size: None,
                state: LinkState::Unchecked,
                on: !known,
                known,
                magnet,
                check: None,
                page: false,
                read: None,
            });
            if !settle.contains(&pkg) {
                settle.push(pkg);
            }
        }
        for pkg in settle {
            self.settle(pkg);
        }
        if !source.is_empty() && !self.source.split(", ").any(|s| s == source) {
            self.source = if self.source.is_empty() {
                source
            } else {
                format!("{}, {source}", self.source)
            };
        }
    }

    /// `/show grabber`: a page's links as looked up (the docs' screenshots), nothing asked of
    /// the network.
    pub fn sample(&mut self) {
        let page = Some("https://example.com/files/".to_owned());
        let links = [
            (
                "https://dl.example.com/holiday.part1.rar",
                Some(1_610_612_736),
            ),
            (
                "https://dl.example.com/holiday.part2.rar",
                Some(1_610_612_736),
            ),
            ("https://dl.example.com/holiday.part3.rar", None),
            ("https://example.com/files/manual.pdf", Some(2_516_582)),
            ("https://example.com/files/setup.exe", Some(48_234_496)),
            ("https://example.com/files/index.html", Some(18_432)),
        ];
        let found = links
            .iter()
            .map(|(u, _)| Imported {
                url: (*u).to_owned(),
                referer: page.clone(),
                ..Default::default()
            })
            .collect();
        // i18n: in Link grabber, where the addresses came from: a page in the browser
        let from = crate::i18n::t("Links from the browser").to_owned();
        self.add(found, from, |_| false);
        for (l, (_, size)) in self.links.iter_mut().zip(links) {
            l.size = size;
            l.state = if size.is_some() {
                LinkState::Online
            } else {
                l.on = false;
                LinkState::Offline(404)
            };
        }
    }

    fn package_for(&mut self, origin: Origin) -> usize {
        if let Some(k) = self.packages.iter().position(|p| p.origin.same(&origin)) {
            return k;
        }
        self.packages.push(Package {
            name: origin.name(),
            origin,
            folder: String::new(),
            folder_set: false,
            password: String::new(),
            extract: false,
            extract_set: false,
            delete: self.delete_default,
            open: true,
        });
        self.packages.len() - 1
    }

    /// A package's defaults after its links changed: the folder of its files' category when
    /// they have one; else none, each file going to its own category's folder (as Import always
    /// did); "Extract when done" when it holds an archive.
    fn settle(&mut self, k: usize) {
        let links: Vec<&Link> = self.links.iter().filter(|l| l.pkg == k).collect();
        let mut cats = links.iter().map(|l| l.category());
        let first = cats.next().unwrap_or(Category::General);
        let folder = if cats.all(|c| c == first) {
            category_folder(first, &self.downloads, &self.dirs)
                .display()
                .to_string()
        } else {
            String::new()
        };
        let archives = links
            .iter()
            .any(|l| !l.magnet && crate::extractq::archive_name(&l.name));
        let p = &mut self.packages[k];
        if !p.folder_set {
            p.folder = folder;
        }
        if !p.extract_set {
            p.extract = archives;
        }
    }

    /// Shown (not a hidden web page)?
    fn shows(&self, l: &Link) -> bool {
        !(self.hide_pages && l.is_page())
    }

    fn visible_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (k, p) in self.packages.iter().enumerate() {
            let mut links = self
                .links
                .iter()
                .enumerate()
                .filter(|(_, l)| l.pkg == k && self.shows(l))
                .peekable();
            if links.peek().is_none() {
                continue;
            }
            rows.push(Row::Pkg(k));
            if p.open {
                rows.extend(links.map(|(i, _)| Row::Link(i)));
            }
        }
        rows
    }

    /// Links to look up now (at most `CHECKS` at once, `CHECKS_PER_HOST` from one host): the
    /// caller starts each ([`Grabber::checking`]) with the link's request.
    pub fn to_check(&self) -> Vec<(usize, Url, Imported)> {
        let busy: Vec<&str> = self
            .links
            .iter()
            .filter(|l| l.state == LinkState::Checking)
            .map(|l| l.url.host_str().unwrap_or_default())
            .collect();
        let mut room = CHECKS.saturating_sub(busy.len());
        let mut hosts = busy.clone();
        let mut out = Vec::new();
        for (i, l) in self.links.iter().enumerate() {
            if room == 0 {
                break;
            }
            let host = l.url.host_str().unwrap_or_default();
            if l.state != LinkState::Unchecked
                || l.known
                || l.magnet
                || hosts.iter().filter(|h| **h == host).count() >= CHECKS_PER_HOST
            {
                continue;
            }
            hosts.push(host);
            room -= 1;
            out.push((i, l.url.clone(), l.found.clone()));
        }
        out
    }

    /// Link `i` is being looked up; its answer comes in `slot`.
    pub fn checking(&mut self, i: usize, slot: Check) {
        if let Some(l) = self.links.get_mut(i) {
            l.state = LinkState::Checking;
            l.check = Some(slot);
        }
    }

    /// Web pages to read for their links now (#705), at most `READS` at once: the caller reads
    /// each ([`Grabber::reading`]) with the page's request.
    pub fn to_read(&self) -> Vec<(usize, Url, Imported)> {
        let busy = self
            .links
            .iter()
            .filter(|l| l.read.as_ref().is_some_and(|r| r.lock().unwrap().is_none()))
            .count();
        self.links
            .iter()
            .enumerate()
            .filter(|(_, l)| l.to_read())
            .take(READS.saturating_sub(busy))
            .map(|(i, l)| (i, l.url.clone(), l.found.clone()))
            .collect()
    }

    /// Page `i` is being read; its links come in `slot`.
    pub fn reading(&mut self, i: usize, slot: Read) {
        if let Some(l) = self.links.get_mut(i) {
            l.read = Some(slot);
        }
    }

    /// The links of the pages read since the last call, each page's as one list. A page that
    /// had links is unticked: its links were what was wanted, not the page.
    pub fn pages_read(&mut self) -> Vec<Vec<Imported>> {
        let mut out = Vec::new();
        for l in &mut self.links {
            let Some(found) = l.read.as_ref().and_then(|r| r.lock().unwrap().take()) else {
                continue;
            };
            if !found.is_empty() {
                l.on = false;
                out.push(found);
            }
        }
        out
    }

    /// Answers that came: the server's name and size, or why the link is no good.
    fn take_answers(&mut self) {
        let mut moved = Vec::new();
        for l in &mut self.links {
            let Some(slot) = &l.check else { continue };
            let Some(answer) = slot.lock().unwrap().take() else {
                continue;
            };
            l.check = None;
            match answer {
                Ok(info) => {
                    l.state = LinkState::Online;
                    l.page = info.web_page;
                    l.size = info.size;
                    let named = mdm_engine::naming::sanitize(&info.file_name);
                    if !named.is_empty() && named != l.name {
                        l.name = named;
                        moved.push(l.pkg);
                    }
                }
                Err(state) => {
                    // Not there: unticked, as IDM leaves a broken link out.
                    if matches!(state, LinkState::Offline(_)) {
                        l.on = false;
                    }
                    l.state = state;
                }
            }
        }
        for k in moved {
            self.settle(k);
        }
    }

    /// The package `k`'s links shown: all ticked, some, or none.
    fn ticks(&self, k: usize) -> (usize, usize) {
        let shown = self.links.iter().filter(|l| l.pkg == k && self.shows(l));
        shown.fold((0, 0), |(on, n), l| (on + l.on as usize, n + 1))
    }

    fn tick_package(&mut self, k: usize) {
        let (on, n) = self.ticks(k);
        let to = on < n;
        let hide = self.hide_pages;
        for l in self.links.iter_mut().filter(|l| l.pkg == k) {
            if !(hide && l.is_page()) {
                l.on = to;
            }
        }
    }

    fn picked(&self) -> Vec<Picked> {
        self.packages
            .iter()
            .enumerate()
            .filter_map(|(k, p)| {
                let links: Vec<(Imported, String, Option<u64>)> = self
                    .links
                    .iter()
                    .filter(|l| l.pkg == k && l.on && self.shows(l))
                    .map(|l| (l.found.clone(), l.name.clone(), l.size))
                    .collect();
                (!links.is_empty()).then(|| Picked {
                    name: p.name.clone(),
                    folder: PathBuf::from(p.folder.trim()),
                    password: Some(p.password.clone()).filter(|s| !s.is_empty()),
                    extract: p.extract,
                    delete: p.extract && p.delete,
                    links,
                })
            })
            .collect()
    }

    /// `Done((start, packages))`: Start (`true`) or Add to queue (`false`) with the ticked
    /// links of each package.
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<(bool, Vec<Picked>)> {
        self.take_answers();
        if self.links.iter().any(|l| l.state == LinkState::Checking) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        let out = window(
            ctx,
            "grabber",
            "Link grabber",
            [640.0, BUTTONS_Y + 30.0],
            true,
            LIGHT_BG(),
            |ui, o| self.body(ui, o),
        );
        match out {
            Outcome::Open => Outcome::Open,
            Outcome::Closed => Outcome::Closed,
            Outcome::Done(start) => Outcome::Done((start, self.picked())),
        }
    }

    fn body(&mut self, ui: &mut Ui, o: Pos2) -> Outcome<bool> {
        let p = ui.painter().clone();
        // Ctrl+V: the addresses in the pasted text, as Import finds them in a file.
        let pasted: Option<String> = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Paste(t) => Some(t.clone()),
                _ => None,
            })
        });
        // Not while a box of the package has the keys: the text goes there.
        if let Some(t) = pasted.filter(|_| !self.editing) {
            let found = crate::transfer::import(&t);
            // i18n: Link grabber: where links came from: the clipboard (pasted with Ctrl+V)
            let from = crate::i18n::t("Pasted").to_owned();
            self.add(found, from, |_| false);
        }
        let line = crate::i18n::fmt(
            // i18n: Link grabber: {source} is where the links came from ("Links from the browser", a file's name, "Pasted")
            crate::i18n::t("Source: {source}"),
            &[("source", &self.source)],
        );
        winui::fit(
            ui,
            &p,
            o.x + 11.0,
            o.y + 20.0,
            &line,
            LIGHT_TEXT(),
            o.x + 629.0,
            "grabber-source",
        );
        self.list(
            ui,
            Rect::from_min_size(o + vec2(11.0, 30.0), vec2(618.0, 258.0)),
        );

        // Under the list: Hide web pages, the count, Check all / Uncheck all.
        let mut hide = self.hide_pages;
        // i18n: Link grabber check box (IDM's "Hide HTML files"): links to web pages are not shown
        winui::checkbox_light(ui, o + vec2(11.0, 299.0), &mut hide, "Hide web pages", true);
        self.hide_pages = hide;
        let shown: Vec<&Link> = self.links.iter().filter(|l| self.shows(l)).collect();
        let ticked = shown.iter().filter(|l| l.on).count();
        let count = crate::i18n::fmt(
            crate::i18n::t("{n} of {total} selected"),
            &[
                ("n", &crate::i18n::integer(ticked as u64)),
                ("total", &crate::i18n::integer(shown.len() as u64)),
            ],
        );
        // After the check box's caption, however long it is in this language.
        let after = 11.0 + 18.0 + winui::text_width(&p, crate::i18n::t("Hide web pages")) + 24.0;
        text(&p, o.x + after, o.y + 310.0, &count, LIGHT_TEXT());
        let picks = winui::buttons_light_right(
            ui,
            o,
            629.0,
            296.0,
            &[
                // i18n: Link grabber button (IDM's): tick every link
                (crate::i18n::mark("Check all"), ButtonKind::Normal),
                // i18n: Link grabber button (IDM's): untick every link
                (crate::i18n::mark("Uncheck all"), ButtonKind::Normal),
            ],
        );
        if picks[0].clicked() {
            let hide = self.hide_pages;
            for l in &mut self.links {
                l.on = !(hide && l.is_page());
            }
        }
        if picks[1].clicked() {
            self.links.iter_mut().for_each(|l| l.on = false);
        }

        self.package_box(ui, o);

        let any = ticked > 0;
        let kind = |default: bool| match (any, default) {
            (false, _) => ButtonKind::Disabled,
            (true, true) => ButtonKind::Default,
            (true, false) => ButtonKind::Normal,
        };
        let row = winui::buttons_light_right(
            ui,
            o,
            629.0,
            BUTTONS_Y,
            &[
                // i18n: Link grabber button: add the ticked links to the download queue without starting them (IDM's OK)
                (crate::i18n::mark("Add to queue"), kind(false)),
                // i18n: Link grabber button: download the ticked links now
                (crate::i18n::mark("Start"), kind(true)),
                (crate::i18n::mark("Cancel"), ButtonKind::Normal),
            ],
        );
        let (later, start, cancel) = (row[0].clicked(), row[1].clicked(), row[2].clicked());
        if cancel || winui::escape_pressed(ui) {
            Outcome::Closed
        } else if any && (start || winui::enter_pressed(ui)) {
            Outcome::Done(true)
        } else if any && later {
            Outcome::Done(false)
        } else {
            Outcome::Open
        }
    }

    /// The packages and their links, as a tree: a package's box ticks all its links (mixed
    /// when only some are), its arrow, a double-click, Right / Left open and close it; a link's
    /// name, size and what looking it up found.
    fn list(&mut self, ui: &mut Ui, list: Rect) {
        let p = ui.painter().clone();
        winui::solid(&p, winui::rect(list), winui::FIELD_EDGE());
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
        let (size_x, state_x) = (head.right() - 132.0, head.right() - 124.0);
        // i18n: column title: file name
        text(
            &p,
            head.left() + 25.0,
            head.top() + 14.0,
            crate::i18n::t("Name"),
            winui::TEXT(),
        );
        // i18n: column title: file size
        winui::text_right(
            &p,
            size_x,
            head.top() + 14.0,
            crate::i18n::t("Size"),
            winui::TEXT(),
        );
        // i18n: column title: Link grabber: what looking the link up found (Online, Offline…)
        text(
            &p,
            state_x,
            head.top() + 14.0,
            crate::i18n::t("Status"),
            winui::TEXT(),
        );
        let body = Rect::from_min_max(pos2(inner.left(), head.bottom()), inner.max);
        if self.links.is_empty() {
            // i18n: Link grabber, while it is empty
            let note = crate::i18n::t(
                "Paste links here (Ctrl+V), or send a page's links from the browser.",
            );
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
        let rows = self.visible_rows();
        // i18n: what a screen reader calls Link grabber's list
        self.rows.name = crate::i18n::t("Links");
        let mut toggle_link = None;
        let mut toggle_pkg = None;
        let mut flip = None;
        let this = &*self;
        let ev = {
            let (packages, links) = (&this.packages, &this.links);
            let mut st = this.rows;
            let ev = winui::row_list(
                ui,
                "grabber-list",
                winui::rect(body.expand(1.0)),
                rows.len(),
                ROW,
                &mut st,
                |ui, clip, k, row, look| {
                    let top = row.top();
                    match rows[k] {
                        Row::Pkg(n) => {
                            let pk = &packages[n];
                            let x = row.left() + 6.0;
                            let arrow = Rect::from_min_size(pos2(x, top), vec2(14.0, ROW));
                            if ui
                                .interact(
                                    winui::rect(arrow),
                                    egui::Id::unique(("grabber-arrow", n)),
                                    winui::pointer_only(),
                                )
                                .clicked()
                            {
                                flip = Some(n);
                            }
                            winui::tree_expander(clip, pos2(x + 4.0, top + 5.0), pk.open);
                            let (on_n, all) = this.ticks(n);
                            let glyph = pos2(x + 16.0, top + 2.0);
                            let mut on = on_n == all;
                            // i18n: Link grabber: a package's row: {n} is its number of links
                            let files = crate::i18n::tn(all as u64, "{n} file", "{n} files");
                            look.name = format!("{}, {files}", pk.name);
                            if winui::row_check(
                                ui,
                                clip,
                                glyph,
                                &mut on,
                                &pk.name,
                                ("grabber-pkg", n),
                            ) {
                                toggle_pkg = Some(n);
                            }
                            if on_n > 0 && on_n < all {
                                winui::row_check_mixed(clip, glyph);
                            }
                            crate::fileicons::paint_at(
                                ui,
                                clip,
                                crate::fileicons::Kind::Folder,
                                pos2(glyph.x + 19.0, top + 1.0),
                                16.0,
                            );
                            let left = glyph.x + 19.0 + crate::fileicons::INDENT;
                            let name = crate::i18n::isolate(&pk.name);
                            // The name's English extent (fit's rect is on screen: mirrored in RTL).
                            let shown_w =
                                winui::text_width(clip, &name).min(state_x - 140.0 - left);
                            winui::fit(
                                ui,
                                clip,
                                left,
                                top + 13.0,
                                &name,
                                look.ink,
                                state_x - 140.0,
                                ("grabber-pkg-name", n),
                            );
                            let tint = if look.selected {
                                look.ink
                            } else {
                                winui::MUTED()
                            };
                            let total: Option<u64> = links
                                .iter()
                                .filter(|l| l.pkg == n && this.shows(l))
                                .map(|l| l.size)
                                .sum();
                            let what = match total {
                                Some(b) => format!("({files}, {})", crate::dialogs::dialog_size(b)),
                                None => format!("({files})"),
                            };
                            winui::fit(
                                ui,
                                clip,
                                left + shown_w + 8.0,
                                top + 13.0,
                                &what,
                                tint,
                                row.right() - 8.0,
                                ("grabber-pkg-what", n),
                            );
                        }
                        Row::Link(i) => {
                            let l = &links[i];
                            let glyph = pos2(row.left() + 40.0, top + 2.0);
                            let mut on = l.on;
                            let size = l.size.map(crate::dialogs::dialog_size).unwrap_or_default();
                            let state = if l.magnet && l.state == LinkState::Unchecked {
                                // i18n: Link grabber, a link's status: a magnet link (a torrent, its files looked up once it is added)
                                crate::i18n::t("Magnet link").to_owned()
                            } else {
                                state_text(&l.state)
                            };
                            look.name = [l.name.as_str(), &size, &state]
                                .iter()
                                .filter(|s| !s.is_empty())
                                .copied()
                                .collect::<Vec<_>>()
                                .join(", ");
                            if winui::row_check(
                                ui,
                                clip,
                                glyph,
                                &mut on,
                                &l.name,
                                ("grabber-link", i),
                            ) {
                                toggle_link = Some(i);
                            }
                            crate::fileicons::paint_at(
                                ui,
                                clip,
                                crate::fileicons::Kind::File(&l.name),
                                pos2(glyph.x + 19.0, top + 1.0),
                                16.0,
                            );
                            let left = glyph.x + 19.0 + crate::fileicons::INDENT;
                            let name = crate::i18n::isolate(&l.name);
                            winui::fit(
                                ui,
                                clip,
                                left,
                                top + 13.0,
                                &name,
                                look.ink,
                                size_x - 70.0,
                                ("grabber-name", i),
                            );
                            winui::text_right(clip, size_x, top + 13.0, &size, look.ink);
                            let ink = if look.selected {
                                look.ink
                            } else {
                                match l.state {
                                    LinkState::Online => winui::SUCCESS(),
                                    LinkState::Offline(_) | LinkState::Failed(_) => winui::ERROR(),
                                    _ => winui::MUTED(),
                                }
                            };
                            winui::fit(
                                ui,
                                clip,
                                state_x,
                                top + 13.0,
                                &state,
                                ink,
                                row.right() - 6.0,
                                ("grabber-state", i),
                            );
                        }
                    }
                },
            );
            (ev, st)
        };
        let (ev, st) = ev;
        self.rows = st;
        if let Some(k) = ev.space.or(ev.double_clicked) {
            match rows[k] {
                Row::Pkg(n) if ev.double_clicked == Some(k) => flip = Some(n),
                Row::Pkg(n) => toggle_pkg = Some(n),
                Row::Link(i) => toggle_link = Some(i),
            }
        }
        if let Some(k) = self.rows.selected.filter(|k| *k < rows.len()) {
            self.current = match rows[k] {
                Row::Pkg(n) => n,
                Row::Link(i) => self.links[i].pkg,
            };
            if ev.focused
                && let Row::Pkg(n) = rows[k]
            {
                let open = self.packages[n].open;
                if (winui::take_key(ui, egui::Key::ArrowRight) && !open)
                    || (winui::take_key(ui, egui::Key::ArrowLeft) && open)
                {
                    flip = Some(n);
                }
            }
        }
        if ev.focused {
            let names: Vec<String> = rows
                .iter()
                .map(|r| match *r {
                    Row::Pkg(n) => self.packages[n].name.clone(),
                    Row::Link(i) => self.links[i].name.clone(),
                })
                .collect();
            self.rows
                .type_to(ui, "grabber-list", rows.len(), |k| names[k].clone());
        }
        if let Some(n) = flip {
            self.packages[n].open = !self.packages[n].open;
        }
        if let Some(n) = toggle_pkg {
            self.tick_package(n);
        }
        if let Some(i) = toggle_link {
            self.links[i].on = !self.links[i].on;
        }
    }

    /// The selected package's name, folder, password and what happens to its archives.
    fn package_box(&mut self, ui: &mut Ui, o: Pos2) {
        let p = ui.painter().clone();
        let shown = self.visible_rows();
        let has = |k: usize| shown.contains(&Row::Pkg(k));
        if !has(self.current) {
            self.current = (0..self.packages.len()).find(|k| has(*k)).unwrap_or(0);
        }
        let enabled = has(self.current);
        // i18n: Link grabber: the box that edits the selected package
        let caption = crate::i18n::t("Package");
        let cw = winui::text_width(&p, caption);
        winui::group_box(
            &p,
            r(o, 11.0, 330.0, 618.0, 106.0),
            Some((o.x + 18.0, o.x + 22.0 + cw)),
        );
        text(&p, o.x + 20.0, o.y + 334.0, caption, LIGHT_TEXT());
        let mut blank = (String::new(), String::new(), String::new(), false, false);
        let pk = self.packages.get_mut(self.current).filter(|_| enabled);
        let (name, folder, password, extract, delete) = match pk {
            Some(pk) => (
                &mut pk.name,
                &mut pk.folder,
                &mut pk.password,
                &mut pk.extract,
                &mut pk.delete,
            ),
            None => (
                &mut blank.0,
                &mut blank.1,
                &mut blank.2,
                &mut blank.3,
                &mut blank.4,
            ),
        };
        let ink = if enabled {
            LIGHT_TEXT()
        } else {
            winui::LIGHT_DISABLED()
        };
        // Labels before their boxes: a column as wide as this language's longest (up to 160 px).
        // i18n: Link grabber: the package's name (a label before its box)
        let name_label = crate::i18n::t("Name:");
        // i18n: Link grabber: the folder the package's files go to (a label before its box)
        let folder_label = crate::i18n::t("Save to:");
        // i18n: Link grabber: the package's archives' password (a label before its box)
        let pw_label = crate::i18n::t("Password:");
        let wide = |s: &str| winui::text_width(&p, s);
        let bx = (21.0 + wide(name_label).max(wide(folder_label)) + 8.0).clamp(80.0, 160.0);
        let label = |ui: &Ui, x: f32, y: f32, s: &str, right: f32| {
            winui::fit(
                ui,
                &p,
                o.x + x,
                o.y + y,
                s,
                ink,
                o.x + right,
                ("grabber-label", y as i32, x as i32),
            );
        };
        // Row 1: the name, then the password.
        label(ui, 21.0, 360.0, name_label, bx - 4.0);
        let pw_x = 340.0;
        let pbx = (pw_x + wide(pw_label) + 8.0).min(470.0);
        let named = winui::edit_light(
            ui,
            r(o, bx, 345.0, pw_x - 12.0 - bx, 23.0),
            name,
            enabled,
            false,
        );
        label(ui, pw_x, 360.0, pw_label, pbx - 4.0);
        let pw = winui::password_light(ui, r(o, pbx, 345.0, 621.0 - pbx, 23.0), password, enabled);
        // i18n: what a screen reader calls Link grabber's password box
        let pw_name = crate::i18n::t("Archive password");
        ui.ctx().accesskit_node_builder(pw.id, |n| {
            n.set_label(pw_name);
            // UI Automation's id: tools tell the one password box apart by it (its name is
            // not read out to them).
            n.set_author_id("grabber-password");
        });
        // Row 2: the folder, and Browse.
        label(ui, 21.0, 391.0, folder_label, bx - 4.0);
        // i18n: verb: choose a folder
        let bw = winui::button_width(&p, "Browse", 73.0);
        let typed = winui::edit_light(
            ui,
            r(o, bx, 376.0, 621.0 - bw - 8.0 - bx, 23.0),
            folder,
            enabled,
            false,
        );
        if enabled && folder.is_empty() && !typed.has_focus() {
            // i18n: Link grabber, in the empty "Save to" box: each file goes to its category's folder (Options → Save to)
            let each = crate::i18n::t("Each file's category folder");
            winui::fit(
                ui,
                &p,
                o.x + bx + 5.0,
                o.y + 391.0,
                each,
                winui::MUTED(),
                o.x + 621.0 - bw - 12.0,
                "grabber-each",
            );
        }
        let browse = winui::button_light(
            ui,
            r(o, 621.0 - bw, 377.0, bw, 21.0),
            "Browse",
            if enabled {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            },
            false,
        )
        .clicked();
        if browse {
            let from = folder.clone();
            winui::start_pick(ui.ctx(), "grabber-folder", move || {
                rfd::FileDialog::new().set_directory(from).pick_folder()
            });
        }
        let mut chose = false;
        if let Some(dir) = winui::picked(ui.ctx(), "grabber-folder") {
            *folder = dir.display().to_string();
            chose = true;
        }
        self.editing = named.has_focus() || typed.has_focus() || pw.has_focus();
        // Row 3: what becomes of its archives.
        let was = *extract;
        // i18n: Link grabber check box: extract the package's archives once they are downloaded
        let extract_label = crate::i18n::mark("Extract when done");
        winui::checkbox_light(ui, o + vec2(21.0, 411.0), extract, extract_label, enabled);
        let extract_now = *extract;
        let next = 21.0 + 18.0 + wide(crate::i18n::t(extract_label)) + 24.0;
        // i18n: Link grabber check box: after extracting, the archives go to the Recycle Bin
        winui::checkbox_light(
            ui,
            o + vec2(next, 411.0),
            delete,
            "Delete archives after extracting",
            enabled && extract_now,
        );
        if enabled {
            let pk = &mut self.packages[self.current];
            pk.folder_set |= typed.changed() || chose;
            pk.extract_set |= was != extract_now;
        }
    }
}

/// A link's address as Link grabber tells links apart: without its `#fragment` (the same file);
/// a magnet link by its torrent's hash alone (its name and trackers aside).
fn address_key(u: &Url) -> Url {
    if let Some(hash) = crate::transfer::magnet_hash(u.as_str())
        && let Ok(key) = Url::parse(&format!("magnet:?xt=urn:btih:{hash}"))
    {
        return key;
    }
    let mut u = u.clone();
    u.set_fragment(None);
    u
}

/// What looking a link up found, as its row says it.
fn state_text(s: &LinkState) -> String {
    match s {
        LinkState::Unchecked => String::new(),
        LinkState::Checking => crate::i18n::t("Checking...").to_owned(),
        // i18n: Link grabber, a link's status: the server has the file
        LinkState::Online => crate::i18n::t("Online").to_owned(),
        LinkState::Offline(code) => crate::i18n::fmt(
            // i18n: Link grabber, a link's status: the server hasn't the file; {status} is the HTTP code (404)
            crate::i18n::t("Offline ({status})"),
            &[("status", &code.to_string())],
        ),
        // i18n: Link grabber, a link's status: the server wants a login (401, 403)
        LinkState::NeedsLogin => crate::i18n::t("Needs a login").to_owned(),
        // i18n: Link grabber, a link's status: the server asked to try later (429, 503)
        LinkState::Busy => crate::i18n::t("Busy, try later").to_owned(),
        LinkState::Failed(why) => why.clone(),
    }
}

/// A look-up's failure as a link's status.
pub fn state_of(e: &mdm_engine::Error) -> LinkState {
    match e {
        mdm_engine::Error::Status(s) => match s.as_u16() {
            404 | 410 => LinkState::Offline(s.as_u16()),
            401 | 403 => LinkState::NeedsLogin,
            429 | 503 => LinkState::Busy,
            _ => LinkState::Failed(crate::errors::describe(e)),
        },
        mdm_engine::Error::Throttled(..) => LinkState::Busy,
        _ => LinkState::Failed(crate::errors::describe(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str, referer: Option<&str>) -> Imported {
        Imported {
            url: url.into(),
            referer: referer.map(str::to_owned),
            ..Default::default()
        }
    }

    fn grabber() -> Grabber {
        Grabber::new(PathBuf::from("D:\\dl"), Vec::new(), true, false)
    }

    #[test]
    fn a_long_list_is_added_at_once_each_link_once() {
        // #805: 20,000 links, each compared with every other, held the window's thread.
        let lists = |n: usize| {
            let many: Vec<Imported> = (0..n)
                .map(|i| link(&format!("https://dl.example.com/f/{i}.zip"), None))
                .collect();
            // Added again (and with a #fragment): nothing more.
            let again: Vec<Imported> = many
                .iter()
                .map(|d| link(&format!("{}#again", d.url), None))
                .collect();
            (grabber(), many, again)
        };
        let add = |(mut g, many, again): (Grabber, Vec<Imported>, Vec<Imported>)| {
            g.add(many, "list.txt".into(), |_| false);
            g.add(again, "list.txt".into(), |_| false);
            g
        };
        // Each link compared once: 8 times the links cost about 8 times as long (not 64).
        mdm_labserver::growth::assert_linear(1_000, &lists, &add);
        let g = add(lists(20_000));
        assert_eq!(g.links.len(), 20_000);
        // Settled as before: one package of archives, to extract.
        assert_eq!(g.packages.len(), 1);
        assert!(g.packages[0].extract);
    }

    #[test]
    fn packages_by_volumes_then_page_then_various() {
        let page = Some("https://site.org/files/?sort=1");
        let mut g = grabber();
        g.add(
            vec![
                link("https://cdn.site.org/Movie.2024.part1.rar", page),
                link("https://cdn.site.org/movie.2024.PART2.rar", page),
                link("https://cdn.site.org/movie.2024.part3.rar#x", page),
                link("https://site.org/notes.txt", page),
                link("https://site.org/setup.exe", page),
                link("https://other.org/x.7z.001", None),
                link("https://other.org/x.7z.002", None),
                link("https://other.org/alone.zip", None),
                link("https://site.org/a.zip", None),
                // Again: added once.
                link("https://cdn.site.org/Movie.2024.part1.rar#again", page),
                link("https://known.org/k.zip", page),
            ],
            "Links from the browser".into(),
            |u| u.host_str() == Some("known.org"),
        );
        let names: Vec<(&str, usize)> = g
            .packages
            .iter()
            .enumerate()
            .map(|(k, p)| {
                (
                    p.name.as_str(),
                    g.links.iter().filter(|l| l.pkg == k).count(),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("Movie.2024", 3),
                ("site.org/files", 2),
                ("x", 2),
                ("Various files", 2),
                ("Already in the list", 1),
            ]
        );
        assert!(g.packages[0].extract, "a package of archives is extracted");
        assert!(!g.packages[1].extract);
        assert!(g.packages[3].extract, "Various holds two zips");
        let known = g.links.iter().find(|l| l.known).unwrap();
        assert!(!known.on);
        // The ticked links, each package to its folder.
        let picked = g.picked();
        assert_eq!(picked.len(), 4);
        assert_eq!(picked[0].links.len(), 3);
    }

    /// IDM's export file through Tasks → Import: every download keeps its referrer, cookies
    /// and User-Agent, and a list of several kinds of files goes each to its category's folder
    /// (the package has no folder of its own), as Import always did.
    #[test]
    fn an_idm_export_file_keeps_its_request_context() {
        let ef2 = "<\r\nhttps://files.example.org/setup.exe\r\nreferer: https://example.org/get\r\n\
                   cookie: sid=42; theme=dark\r\nUser-Agent: Mozilla/5.0 Test\r\n>\r\n\
                   <\r\nhttps://files.example.org/manual.pdf\r\nreferer: https://example.org/get\r\n\
                   cookie: sid=42\r\n>\r\n<\r\nhttps://other.org/song.mp3\r\n>\r\n";
        let found = crate::transfer::import(ef2);
        assert_eq!(found.len(), 3);
        let mut g = grabber();
        g.add(found, "list.ef2".into(), |_| false);
        let picked = g.picked();
        let all: Vec<&(Imported, String, Option<u64>)> =
            picked.iter().flat_map(|p| &p.links).collect();
        assert_eq!(all.len(), 3);
        let setup = &all.iter().find(|l| l.1 == "setup.exe").unwrap().0;
        assert_eq!(setup.referer.as_deref(), Some("https://example.org/get"));
        assert_eq!(setup.cookies.as_deref(), Some("sid=42; theme=dark"));
        assert_eq!(setup.user_agent.as_deref(), Some("Mozilla/5.0 Test"));
        let manual = &all.iter().find(|l| l.1 == "manual.pdf").unwrap().0;
        assert_eq!(manual.cookies.as_deref(), Some("sid=42"));
        // The page's package (a program and a document): no folder, each to its category's.
        let page = picked.iter().find(|p| p.name == "example.org/get").unwrap();
        assert_eq!(page.folder, PathBuf::new());
        assert!(!page.extract);
        // One kind of file: its category's folder.
        let various = picked.iter().find(|p| p.links[0].1 == "song.mp3").unwrap();
        assert_ne!(various.folder, PathBuf::new());
    }

    #[test]
    fn same_name_on_two_hosts_is_two_packages() {
        let a = Origin::of(
            &"https://a.org/x.part1.rar".parse().unwrap(),
            "x.part1.rar",
            None,
        );
        let b = Origin::of(
            &"https://b.org/x.part2.rar".parse().unwrap(),
            "x.part2.rar",
            None,
        );
        assert!(!a.same(&b));
        let c = Origin::of(
            &"https://A.org/X.part2.rar".parse().unwrap(),
            "X.part2.rar",
            None,
        );
        assert!(a.same(&c));
    }

    #[test]
    fn a_page_given_on_its_own_is_read_for_its_links() {
        let mut g = grabber();
        let page = "https://example.com/water/epanet";
        let found_on = "https://example.com/files/setup.exe";
        g.add(
            vec![
                link(page, None),
                link(found_on, Some("https://example.com/other")),
            ],
            "Pasted".into(),
            |_| false,
        );
        assert!(
            g.to_read().is_empty(),
            "nothing is read before it is looked up"
        );
        // The look-ups: the address is a web page, and so is the link found on a page.
        for (i, url, _) in g.to_check() {
            let slot: Check = Default::default();
            *slot.lock().unwrap() = Some(Ok(mdm_engine::ProbeInfo {
                final_url: url.clone(),
                size: None,
                file_name: "epanet.html".into(),
                server: None,
                web_page: true,
            }));
            g.checking(i, slot);
        }
        g.take_answers();
        // Hidden as a web page, though its address doesn't end in one.
        assert!(g.links.iter().all(|l| !g.shows(l)));
        // Only the page given on its own is read, once.
        let reads = g.to_read();
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].1.as_str(), page);
        let slot: Read = Default::default();
        g.reading(reads[0].0, slot.clone());
        assert!(g.to_read().is_empty());
        assert!(g.pages_read().is_empty(), "nothing came yet");
        let on_page = |u: &str| Imported {
            url: u.into(),
            referer: Some(page.into()),
            ..Default::default()
        };
        *slot.lock().unwrap() = Some(vec![
            on_page("https://dl.example.org/epanet2.2_setup.exe"),
            on_page("https://dl.example.org/epanet2.2.zip"),
        ]);
        let read = g.pages_read();
        assert_eq!(read.len(), 1);
        g.add(read.into_iter().flatten().collect(), String::new(), |_| {
            false
        });
        assert!(g.pages_read().is_empty() && g.to_read().is_empty());
        // The page's links, as one package named after the page; the page itself unticked.
        let shown: Vec<&str> = g
            .links
            .iter()
            .filter(|l| g.shows(l))
            .map(|l| l.name.as_str())
            .collect();
        assert_eq!(shown, ["epanet2.2_setup.exe", "epanet2.2.zip"]);
        let page_link = g.links.iter().find(|l| l.url.as_str() == page).unwrap();
        assert!(!page_link.on);
        let pkg = g
            .links
            .iter()
            .find(|l| l.name == "epanet2.2.zip")
            .unwrap()
            .pkg;
        assert!(g.links.iter().filter(|l| l.pkg == pkg).count() == 2);
        assert!(g.source.contains("Pasted"));
    }

    #[test]
    fn web_pages_hide_and_stay_out() {
        let mut g = grabber();
        g.add(
            vec![
                link("https://s.org/index.html", Some("https://s.org/")),
                link("https://s.org/a.zip", Some("https://s.org/")),
            ],
            String::new(),
            |_| false,
        );
        assert!(web_page("index.HTML") && !web_page("a.zip") && web_page("view.php"));
        assert_eq!(g.picked()[0].links.len(), 1);
        g.hide_pages = false;
        assert_eq!(g.picked()[0].links.len(), 2);
        assert_eq!(g.packages[0].name, "s.org");
    }

    #[test]
    fn checks_are_limited() {
        let mut g = grabber();
        let many: Vec<Imported> = (0..10)
            .map(|k| link(&format!("https://one.org/f{k}.bin"), None))
            .chain((0..10).map(|k| link(&format!("https://h{k}.org/f.bin"), None)))
            .collect();
        g.add(many, String::new(), |_| false);
        let first = g.to_check();
        assert_eq!(first.len(), CHECKS);
        assert_eq!(
            first
                .iter()
                .filter(|(_, u, _)| u.host_str() == Some("one.org"))
                .count(),
            CHECKS_PER_HOST
        );
        for (i, _, _) in first {
            g.checking(i, Arc::default());
        }
        assert!(g.to_check().is_empty());
    }

    /// #709, Tasks → Add batch download from clipboard, as the window takes it: the clipboard
    /// read off the window's thread, its 15 links (written twice, among other text) and a
    /// magnet link become 16 ticked rows in two packages, the magnet one never looked up;
    /// Start hands them all over.
    #[test]
    fn copied_links_become_ticked_rows() {
        let mut post = String::from("Season 1, all episodes:\n");
        for k in 1..=15 {
            post.push_str(&format!(
                "- [Episode {k}](https://media.example.org/s1/ep{k:02}.mp4)\n"
            ));
        }
        post.push_str("Mirror of the first: https://media.example.org/s1/ep01.mp4.\n");
        post.push_str("Or the torrent: magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Season%201\n");
        let text = post.clone();
        let reader: &'static crate::clipboard::Reader = Box::leak(Box::new(
            crate::clipboard::Reader::new(move || Some(text.clone()), || {}),
        ));
        let mut asked = crate::clipboard::Prefill::ask_of(reader);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let got = loop {
            if let Some(got) = asked.answer() {
                break got;
            }
            assert!(
                std::time::Instant::now() < until,
                "the clipboard never answered"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        let crate::clipbatch::Clip::Links(found) = crate::clipbatch::read(got.as_deref()) else {
            panic!("links expected");
        };
        let mut g = grabber();
        g.add(found, "Clipboard".into(), |_| false);
        assert_eq!(g.links.len(), 16);
        assert!(g.links.iter().all(|l| l.on), "every row ticked");
        let rows = g.visible_rows();
        assert_eq!(rows.len(), 16 + 1, "one package: Various files");
        assert_eq!(g.packages[0].name, "Various files");
        let magnet = g.links.iter().find(|l| l.magnet).unwrap();
        assert_eq!(magnet.name, "Season 1");
        assert_eq!(magnet.category(), Category::Torrents);
        assert!(
            g.to_check().iter().all(|(_, u, _)| u.scheme() == "https"),
            "a magnet link is not looked up over HTTP"
        );
        let picked = g.picked();
        let all: Vec<&str> = picked
            .iter()
            .flat_map(|p| p.links.iter().map(|l| l.0.url.as_str()))
            .collect();
        assert_eq!(all.len(), 16);
        assert_eq!(all[0], "https://media.example.org/s1/ep01.mp4");
        assert!(all[15].starts_with("magnet:?"));
        // The same torrent copied again with its trackers: not a second row.
        g.add(
            vec![link(
                "magnet:?xt=urn:btih:0123456789ABCDEF0123456789ABCDEF01234567&tr=udp%3A%2F%2Ft.example%3A80",
                None,
            )],
            String::new(),
            |_| false,
        );
        assert_eq!(g.links.len(), 16);
    }
}
