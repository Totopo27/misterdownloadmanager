//! Click'n'Load in the app (`mdm_cnl`, #37): the links a page's Click'n'Load button posts while
//! Options → Archives and Link grabber → "Accept Click'n'Load from web pages" is on.
//!
//! Any web page can post them, so:
//! - the first post from a site asks whether that site may send links (Allow, Block, Not now);
//!   the answer is kept for the site (Options' `cnl_sites`), "Not now" only for those links;
//! - what an allowed site sends goes to Link grabber, one package as the site packed it, with
//!   its archive password, and is marked as sent by a page: Link grabber doesn't look it up (no
//!   request leaves for it, with or without the user's site logins or debrid accounts) and
//!   nothing is downloaded until the user adds it to the queue;
//! - the listener takes at most `mdm_cnl::POSTS_PER_MINUTE` posts a minute from each site, and
//!   at most [`WAITING`] wait here for an answer; the window asks one site at a time, and a
//!   site brings MDM's window up at most once a minute ([`RAISE_EVERY`]);
//! - the site is the one the browser names; a page it names none for is an unknown site, asked
//!   about every time, and links to this computer or its network never arrive (`mdm_cnl`).
//!
//! The listener runs off the window's thread; what arrives waits in an inbox the window empties
//! each frame (`App::poll_cnl`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mdm_cnl::{Found, Package};

use crate::dialogs::{Outcome, window};
use crate::transfer::Imported;
use crate::winui::{self, ButtonKind, LIGHT_BG, LIGHT_TEXT};

/// The most posts waiting for the window: the inbox and those waiting for the site's answer.
pub const WAITING: usize = 20;
/// How often one site's posts bring MDM's window up: its links still arrive in between, in
/// Link grabber, without the window coming to the front again.
pub const RAISE_EVERY: Duration = Duration::from_secs(60);

/// The listener, while on, the inbox, and the posts waiting for their site's answer.
pub struct Cnl {
    rt: tokio::runtime::Handle,
    ctx: egui::Context,
    inbox: Arc<Mutex<Vec<Found>>>,
    listener: Option<mdm_cnl::Listener>,
    waiting: Vec<Found>,
    /// When each site last brought the window up (None: the pages that name no site).
    raised: HashMap<Option<String>, Instant>,
    /// The site being asked about.
    pub ask: Option<SiteDialog>,
}

/// What the user said about a site.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Allow,
    Block,
    NotNow,
}

impl Cnl {
    pub fn new(rt: tokio::runtime::Handle, ctx: egui::Context) -> Self {
        Cnl {
            rt,
            ctx,
            inbox: Arc::default(),
            listener: None,
            waiting: Vec::new(),
            raised: HashMap::new(),
            ask: None,
        }
    }

    /// Listen (`on`) or stop. Starting can fail, the port being taken (by JDownloader, most
    /// often): why, in the interface language.
    pub fn listen(&mut self, on: bool) -> Result<(), String> {
        if !on {
            self.listener = None;
            self.waiting.clear();
            self.ask = None;
            return Ok(());
        }
        if self.listener.is_some() {
            return Ok(());
        }
        let (inbox, ctx) = (self.inbox.clone(), self.ctx.clone());
        let on_found: mdm_cnl::OnFound = Arc::new(move |found| {
            let mut inbox = inbox.lock().unwrap();
            if inbox.len() >= WAITING {
                return false;
            }
            inbox.push(found);
            ctx.request_repaint();
            true
        });
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], mdm_cnl::PORT));
        match mdm_cnl::Listener::start(&self.rt, addr, on_found) {
            Ok(l) => {
                self.listener = Some(l);
                Ok(())
            }
            Err(e) => {
                tracing::warn!("Click'n'Load can't listen on {addr}: {e}");
                Err(if e.kind() == std::io::ErrorKind::AddrInUse {
                    crate::i18n::fmt(
                        // i18n: notice: Click'n'Load couldn't start; {port} is 9666; JDownloader is another download manager that uses the same port
                        crate::i18n::t(
                            "Click'n'Load can't start: another program (JDownloader?) is using port {port}. Close it, then turn Click'n'Load off and on again in Options.",
                        ),
                        &[("port", &mdm_cnl::PORT.to_string())],
                    )
                } else {
                    crate::i18n::fmt(
                        // i18n: notice: Click'n'Load couldn't start; {error} the system's reason
                        crate::i18n::t("Click'n'Load can't start: {error}"),
                        &[("error", &e.to_string())],
                    )
                })
            }
        }
    }

    /// What arrived since the last frame, sorted by `sites` (Options' answers, site → allowed):
    /// an allowed site's links for Link grabber now; a blocked site's dropped; the others wait,
    /// and the first of them is asked about. With them, whether MDM's window comes up for it:
    /// for an allowed site's links or a new question, once every [`RAISE_EVERY`] for each site.
    pub fn take(&mut self, sites: &[(String, bool)]) -> (Vec<Imported>, bool) {
        self.take_at(sites, Instant::now())
    }

    fn take_at(&mut self, sites: &[(String, bool)], at: Instant) -> (Vec<Imported>, bool) {
        let arrived = std::mem::take(&mut *self.inbox.lock().unwrap());
        let asking = self.ask.is_some();
        let mut raise = false;
        let mut now = Vec::new();
        for found in arrived {
            let said = found
                .site
                .as_ref()
                .and_then(|s| sites.iter().find(|(k, _)| k == s))
                .map(|(_, allowed)| *allowed);
            match said {
                Some(true) => {
                    raise |= self.may_raise(&found.site, at);
                    now.extend(links(found.package));
                }
                Some(false) => {
                    tracing::info!("Click'n'Load: a post from a blocked site, dropped");
                }
                None if self.waiting.len() < WAITING => self.waiting.push(found),
                None => tracing::info!("Click'n'Load: too many posts waiting, one dropped"),
            }
        }
        self.ask_next();
        if !asking && let Some(site) = self.ask.as_ref().map(|a| a.site.clone()) {
            raise |= self.may_raise(&site, at);
        }
        (now, raise)
    }

    /// Whether `site` may bring the window up now: it hasn't within [`RAISE_EVERY`].
    fn may_raise(&mut self, site: &Option<String>, at: Instant) -> bool {
        self.raised
            .retain(|_, t| at.duration_since(*t) < RAISE_EVERY);
        if self.raised.contains_key(site) {
            return false;
        }
        self.raised.insert(site.clone(), at);
        true
    }

    /// The window's answer about the site asked: its waiting links (on Allow), and the answer
    /// to keep for the site (Allow, Block; none on Not now, or for a page that named no site).
    pub fn answer(&mut self, answer: Answer) -> (Vec<Imported>, Option<(String, bool)>) {
        let Some(ask) = self.ask.take() else {
            return (Vec::new(), None);
        };
        let (theirs, others): (Vec<Found>, Vec<Found>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|f| f.site == ask.site);
        self.waiting = others;
        let links = match answer {
            Answer::Allow => theirs.into_iter().flat_map(|f| links(f.package)).collect(),
            Answer::Block | Answer::NotNow => Vec::new(),
        };
        let keep = match (answer, ask.site) {
            (Answer::NotNow, _) | (_, None) => None,
            (a, Some(site)) => Some((site, a == Answer::Allow)),
        };
        self.ask_next();
        (links, keep)
    }

    fn ask_next(&mut self) {
        if self.ask.is_some() {
            return;
        }
        if let Some(first) = self.waiting.first() {
            let site = first.site.clone();
            let count = self
                .waiting
                .iter()
                .filter(|f| f.site == site)
                .map(|f| f.package.links.len())
                .sum();
            self.ask = Some(SiteDialog { site, count });
        }
    }
}

/// Link grabber's rows for a package: each link with its package's name (the site's, else
/// "Click'n'Load") and its first archive password, marked as sent by a page. The page's own
/// address isn't used as their referrer: a page could name any.
fn links(p: Package) -> Vec<Imported> {
    let name = if p.name.trim().is_empty() {
        "Click'n'Load".to_owned()
    } else {
        p.name.trim().to_owned()
    };
    let password = p.passwords.first().cloned();
    p.links
        .into_iter()
        .map(|url| Imported {
            url,
            package: Some(name.clone()),
            password: password.clone(),
            from_page: true,
            ..Default::default()
        })
        .collect()
}

/// "Click'n'Load from {site}": may this site send links to Link grabber?
pub struct SiteDialog {
    /// None: the browser named no site for the page (an unknown site).
    pub site: Option<String>,
    /// The links it sent.
    count: usize,
}

impl SiteDialog {
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<Answer> {
        window(
            ctx,
            "cnl-site",
            // i18n: window title: a web page used Click'n'Load (a link site's button that sends its links to this program)
            "Click'n'Load",
            [420.0, 116.0],
            false,
            LIGHT_BG(),
            |ui, o| {
                let p = ui.painter().clone();
                let n = self.count as u64;
                let line = match &self.site {
                    Some(site) => crate::i18n::fmt(
                        // i18n: Click'n'Load window: {site} is a web site's name such as "example.com"
                        &crate::i18n::tn(
                            n,
                            "{site} wants to send {n} link to Link grabber.",
                            "{site} wants to send {n} links to Link grabber.",
                        ),
                        &[("site", &crate::i18n::isolate(site))],
                    ),
                    // i18n: Click'n'Load window: a page whose web site the browser doesn't name, so MDM can't tell which site it is
                    None => crate::i18n::tn(
                        n,
                        "An unknown site wants to send {n} link to Link grabber.",
                        "An unknown site wants to send {n} links to Link grabber.",
                    ),
                };
                winui::paragraph(
                    &p,
                    o.x + 11.0,
                    o.y + 21.0,
                    &line,
                    LIGHT_TEXT(),
                    o.x + 409.0,
                    14.0,
                );
                winui::paragraph(
                    &p,
                    o.x + 11.0,
                    o.y + 48.0,
                    // i18n: Click'n'Load window, under the question: nothing happens to the links before the user adds them in Link grabber
                    crate::i18n::t(
                        "Allow only sites you use. Nothing is looked up or downloaded before you add it in Link grabber.",
                    ),
                    winui::MUTED(),
                    o.x + 409.0,
                    14.0,
                );
                let row = winui::buttons_light_right(
                    ui,
                    o,
                    409.0,
                    84.0,
                    &[
                        // i18n: button in the Click'n'Load window: let this site send links (remembered)
                        (crate::i18n::mark("Allow"), ButtonKind::Default),
                        // i18n: button in the Click'n'Load window: refuse this site's links (remembered)
                        (crate::i18n::mark("Block"), ButtonKind::Normal),
                        // i18n: button in the Click'n'Load window: refuse these links, ask again next time
                        (crate::i18n::mark("Not now"), ButtonKind::Normal),
                    ],
                );
                if row[0].clicked() {
                    Outcome::Done(Answer::Allow)
                } else if row[1].clicked() {
                    Outcome::Done(Answer::Block)
                } else if row[2].clicked() || winui::escape_pressed(ui) {
                    Outcome::Done(Answer::NotNow)
                } else {
                    Outcome::Open
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(site: Option<&str>, name: &str, links: &[&str]) -> Found {
        Found {
            package: Package {
                name: name.into(),
                passwords: vec!["pw".into()],
                links: links.iter().map(|l| l.to_string()).collect(),
            },
            site: site.map(str::to_owned),
        }
    }

    fn cnl() -> (tokio::runtime::Runtime, Cnl) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let c = Cnl::new(rt.handle().clone(), egui::Context::default());
        (rt, c)
    }

    #[test]
    fn a_site_is_asked_once_and_its_answer_kept() {
        let (_rt, mut c) = cnl();
        let mut sites = vec![
            ("ok.example".to_owned(), true),
            ("no.example".to_owned(), false),
        ];
        c.inbox.lock().unwrap().extend([
            post(Some("ok.example"), "Holiday", &["https://h.example/a.rar"]),
            post(Some("no.example"), "Spam", &["https://s.example/x"]),
            post(
                Some("new.example"),
                "",
                &["https://n.example/1", "https://n.example/2"],
            ),
            post(Some("new.example"), "", &["https://n.example/3"]),
        ]);
        // Allowed: now, with its package and password, marked as sent by a page, no referrer.
        let (now, raise) = c.take(&sites);
        assert!(raise);
        assert_eq!(now.len(), 1);
        assert_eq!(now[0].package.as_deref(), Some("Holiday"));
        assert_eq!(now[0].password.as_deref(), Some("pw"));
        assert!(now[0].from_page && now[0].referer.is_none());
        // Blocked: gone. New: asked once, for both its posts.
        let ask = c.ask.as_ref().unwrap();
        assert_eq!((ask.site.as_deref(), ask.count), (Some("new.example"), 3));
        let (links, keep) = c.answer(Answer::Allow);
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].package.as_deref(), Some("Click'n'Load"));
        assert_eq!(keep, Some(("new.example".to_owned(), true)));
        sites.push(keep.unwrap());
        assert!(c.ask.is_none());
        // Next time it isn't asked.
        c.inbox
            .lock()
            .unwrap()
            .push(post(Some("new.example"), "", &["https://n.example/4"]));
        assert_eq!(c.take(&sites).0.len(), 1);
        assert!(c.ask.is_none());
    }

    #[test]
    fn a_site_brings_the_window_up_once_a_minute() {
        let (_rt, mut c) = cnl();
        let sites = vec![("ok.example".to_owned(), true)];
        let t = Instant::now();
        let send = |c: &mut Cnl, site: Option<&str>, at: Instant| {
            c.inbox
                .lock()
                .unwrap()
                .push(post(site, "", &["https://h.example/a.rar"]));
            c.take_at(&sites, at)
        };
        // An allowed site: its links arrive every time, the window comes up once.
        let (links, raise) = send(&mut c, Some("ok.example"), t);
        assert_eq!((links.len(), raise), (1, true));
        for s in [1, 20, 59] {
            let (links, raise) = send(&mut c, Some("ok.example"), t + Duration::from_secs(s));
            assert_eq!((links.len(), raise), (1, false), "after {s} s");
        }
        let (_, raise) = send(&mut c, Some("ok.example"), t + Duration::from_secs(61));
        assert!(raise);
        // A site answered "Not now" that posts again is asked again, without the window.
        let (_, raise) = send(&mut c, Some("new.example"), t);
        assert!(raise && c.ask.is_some());
        c.answer(Answer::NotNow);
        let (_, raise) = send(&mut c, Some("new.example"), t + Duration::from_secs(5));
        assert!(!raise && c.ask.is_some());
    }

    #[test]
    fn not_now_and_unnamed_pages_are_not_remembered() {
        let (_rt, mut c) = cnl();
        c.inbox.lock().unwrap().extend([
            post(Some("a.example"), "", &["https://a.example/1"]),
            post(None, "", &["https://b.example/1"]),
        ]);
        assert!(c.take(&[]).0.is_empty());
        let (links, keep) = c.answer(Answer::NotNow);
        assert!(links.is_empty() && keep.is_none());
        // The next one waiting is asked: an unknown site, never remembered.
        assert_eq!(c.ask.as_ref().unwrap().site, None);
        let (links, keep) = c.answer(Answer::Allow);
        assert_eq!((links.len(), keep), (1, None));
        assert!(c.ask.is_none());
    }

    #[test]
    fn a_flood_waits_no_more_than_its_room() {
        let (_rt, mut c) = cnl();
        for i in 0..WAITING + 5 {
            c.inbox.lock().unwrap().push(post(
                Some(&format!("s{i}.example")),
                "",
                &["https://x.example/1"],
            ));
        }
        c.take(&[]);
        assert_eq!(c.waiting.len(), WAITING);
        // One window at a time, Block keeps the site.
        let (_, keep) = c.answer(Answer::Block);
        assert_eq!(keep, Some(("s0.example".to_owned(), false)));
        assert_eq!(c.waiting.len(), WAITING - 1);
        assert!(c.ask.is_some());
    }
}
