//! The crash monitor's process (`--crash-monitor`, started by crash.rs) and its report window.
//!
//! The monitor waits without a window. When MDM crashes, panics or freezes, mdm-crash writes the
//! dump and this window opens: what happened, a box for what the user was doing, the whole report
//! to read, and Send on GitHub (the browser opens the Crash report form filled in; nothing is sent
//! by itself). Behind it, the report is made readable: the release's symbols are fetched when the
//! stacks show bare addresses, and GitHub is searched for the issue already filed for the same
//! problem. An unexplained exit (it may be the system shutting down) is only saved, no window.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{Pos2, Rect, Ui, ViewportBuilder};
use mdm_crash::MonitorArgs;
use mdm_crash::github::{self, Found};
use mdm_crash::monitor::{self, Event};
use mdm_crash::protocol::{Kind, Notes};
use mdm_crash::report::{self, Made};

use crate::winui::{self, ButtonKind, r};

const CLIENT: [f32; 2] = [620.0, 540.0];

pub fn monitor_main(m: MonitorArgs) -> eframe::Result {
    mdm_crash::logs::init(&m.data, mdm_crash::logs::MONITOR);
    tracing::info!(
        "crash monitor (process {}) for {}",
        std::process::id(),
        m.parent
    );
    let (tx, rx) = channel();
    {
        let (socket, data, tx, failed) = (m.socket.clone(), m.data.clone(), tx.clone(), tx.clone());
        std::thread::spawn(move || {
            if let Err(e) = monitor::serve(&socket, &data, tx) {
                // The app can't connect: say so, rather than waiting beside it as a second
                // process that can never report anything.
                let _ = failed.send(Event::ServeFailed(e));
            }
        });
    }
    // An app that died before it connected never disconnects: leave once it is gone.
    {
        let parent = m.parent;
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(2));
                if !alive(parent) {
                    std::thread::sleep(Duration::from_secs(8));
                    let _ = tx.send(Event::Gone {
                        clean: true,
                        notes: Notes::default(),
                    });
                    return;
                }
            }
        });
    }
    let (dump, kind, notes) = loop {
        match rx.recv() {
            Ok(Event::Dump { path, kind, notes }) => break (Some(path), kind, notes),
            Ok(Event::DumpFailed { kind, notes, error }) => {
                tracing::error!("crash monitor: no dump ({error})");
                break (None, kind, notes);
            }
            Ok(Event::Gone { clean, notes }) => {
                if !clean && notes.session.is_some() {
                    // Kept for a later look, without a window (a shutdown can look the same).
                    let made = report::make(None, Kind::Exit, notes, &m.data, &[]);
                    tracing::warn!("MDM ended unexpectedly: {}", made.path.display());
                }
                return Ok(());
            }
            Ok(Event::Recovered) => {}
            Ok(Event::ServeFailed(e)) => {
                tracing::error!("crash monitor: {e}");
                return Ok(());
            }
            Err(_) => return Ok(()),
        }
    };
    tracing::warn!("crash monitor: {} report", kind.word());

    // The window speaks MDM's language and follows its theme. The language first: the title
    // below is translated before the window exists, and Windows' caption is found by it
    // (`win32::caption_dark` in `update`).
    let settings = crate::store::load().settings;
    crate::i18n::set_lang(settings.language.as_deref());
    // And its interface size (#262): the window is made before the zoom applies, so its size is
    // given zoomed (brought inside the screen at its first frame).
    let zoom = crate::scale::startup(settings.interface_size);
    crate::theme::init_smooth_text(settings.smooth_text);
    let builder = crate::quiet(
        ViewportBuilder::default()
            .with_title(crate::i18n::os_text(crate::i18n::t(
                "Mister Download Manager crash report",
            )))
            .with_inner_size([CLIENT[0] * zoom, CLIENT[1] * zoom])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_icon(crate::app_icon()),
    );
    let builder = if crate::offscreen() {
        // Where the main window goes too: on a runner's screen (MDM_OFFSCREEN_AT), whose software
        // OpenGL draws nothing off the screens (docs-shots.yml).
        builder.with_position(crate::offscreen_at())
    } else {
        builder
    };
    let options = || eframe::NativeOptions {
        viewport: builder.clone(),
        ..Default::default()
    };
    // What the app draws with (renderer.rs, MDM_RENDERER too), the next way when one can't
    // start. A way the app failed with, or is still starting with (this report may be about it),
    // is passed over: WARP when its Direct3D on the card did, OpenGL when WARP did too; only the
    // app's own starts are watched.
    let mut plan = crate::renderer::plan(
        settings.renderer,
        crate::renderer::overridden(&[]).as_deref(),
        crate::renderer::Known::suspect(),
    );
    plan.watch.clear();
    plan.gl_checked = false;
    let data = m.data.clone();
    crate::renderer::run("misterdownloadmanager-crash", &plan, options, move |cc| {
        crate::scale::start(&cc.egui_ctx, zoom);
        crate::theme::select(&crate::theme::startup_id(&settings.theme), &cc.egui_ctx);
        crate::theme::apply(&cc.egui_ctx);
        crate::theme::language_fonts(&cc.egui_ctx);
        crate::theme::update(
            settings.dark_mode_support,
            settings.always_dark,
            &cc.egui_ctx,
        );
        ReportWindow::new(
            cc.egui_ctx.clone(),
            Job {
                dump,
                kind,
                notes,
                data,
                parent: m.parent,
            },
            rx,
        )
    })
}

fn alive(pid: u32) -> bool {
    #[cfg(windows)]
    return crate::win32::process_alive(pid);
    #[cfg(unix)]
    return crate::unix::process_alive(pid);
}

fn end(pid: u32) -> bool {
    #[cfg(windows)]
    return crate::win32::end_process(pid);
    #[cfg(unix)]
    return crate::unix::end_process(pid);
}

struct Job {
    dump: Option<PathBuf>,
    kind: Kind,
    notes: Notes,
    data: PathBuf,
    parent: u32,
}

/// What the work behind the window has reached.
#[derive(Default)]
struct Shared {
    made: Option<Made>,
    /// Fetching the symbols: (bytes so far, total).
    fetching: Option<(u64, Option<u64>)>,
    /// The symbols couldn't be fetched (why).
    no_symbols: Option<String>,
    searching: bool,
    found: Option<Found>,
}

struct ReportWindow {
    job: Job,
    shared: Arc<Mutex<Shared>>,
    events: Receiver<Event>,
    what: String,
    attach: bool,
    recovered: bool,
    /// Something to tell under the report (copied, sent…).
    told: Option<String>,
    icon: Option<egui::TextureHandle>,
    /// Windows' caption follows the window's theme (set once the window exists).
    caption_set: bool,
    /// DWM gave the caption its light or dark (once: asked every second, it caught the window's
    /// thread in a stalled system, #633).
    #[cfg(windows)]
    caption_dwm: bool,
    /// Brought inside the screen once it exists (scale::fit_window, #262).
    fitted: bool,
}

impl ReportWindow {
    fn new(ctx: egui::Context, job: Job, events: Receiver<Event>) -> ReportWindow {
        let shared = Arc::new(Mutex::new(Shared::default()));
        prepare(&job, shared.clone(), ctx);
        ReportWindow {
            recovered: job.notes.recovered,
            job,
            shared,
            events,
            what: String::new(),
            attach: false,
            told: None,
            icon: None,
            caption_set: false,
            #[cfg(windows)]
            caption_dwm: false,
            fitted: false,
        }
    }

    fn app_alive(&self) -> bool {
        alive(self.job.parent)
    }
}

/// Read the dump, fetch the release's symbols when its stacks lack them and read it again, then
/// look for the issue already filed: on a thread, the window repainted at each step.
fn prepare(job: &Job, shared: Arc<Mutex<Shared>>, ctx: egui::Context) {
    let (dump, kind, notes, data) = (
        job.dump.clone(),
        job.kind,
        job.notes.clone(),
        job.data.clone(),
    );
    std::thread::spawn(move || {
        let version = notes.session.as_ref().map(|s| s.version.clone());
        let from_source = notes
            .session
            .as_ref()
            .is_some_and(|s| s.install == mdm_tools::update::Install::Source.label());
        let store = version
            .as_deref()
            .map(|v| mdm_crash::symbols::store(&data, v));
        let stores: Vec<PathBuf> = store.iter().cloned().collect();
        let made = report::make(dump.as_deref(), kind, notes.clone(), &data, &stores);
        note_gl_freeze(&made);
        let missing = made
            .analysis
            .as_ref()
            .is_some_and(|a| !a.missing_symbols.is_empty());
        set(&shared, &ctx, |s| s.made = Some(made));

        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let Ok(http) = reqwest::Client::builder()
            .user_agent(concat!("MisterDownloadManager/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(120))
            .build()
        else {
            return;
        };
        // The release's symbols, when the stacks need them and they aren't here yet.
        if let (Some(v), true, false) = (version.as_deref(), missing, from_source)
            && !mdm_crash::symbols::have(&data, v)
            && dump.is_some()
        {
            set(&shared, &ctx, |s| s.fetching = Some((0, None)));
            let (sh, cx) = (shared.clone(), ctx.clone());
            let progress = move |done: u64, total: Option<u64>| {
                set(&sh, &cx, |s| s.fetching = Some((done, total)));
            };
            let fetched = rt.block_on(mdm_crash::symbols::fetch(
                &data,
                v,
                crate::settings::own_net(),
                &progress,
            ));
            set(&shared, &ctx, |s| s.fetching = None);
            match fetched {
                Ok(()) => {
                    let made = report::make(dump.as_deref(), kind, notes, &data, &stores);
                    note_gl_freeze(&made);
                    set(&shared, &ctx, |s| s.made = Some(made));
                }
                Err(e) => {
                    tracing::warn!("crash report: symbols not fetched: {e}");
                    set(&shared, &ctx, |s| s.no_symbols = Some(e));
                }
            }
        }
        // The issue already filed for the same problem.
        let signature = shared
            .lock()
            .ok()
            .and_then(|s| s.made.as_ref().map(|m| m.signature.clone()));
        if let Some(sig) = signature {
            set(&shared, &ctx, |s| s.searching = true);
            let found = rt.block_on(github::find(&http, &sig));
            set(&shared, &ctx, |s| {
                s.searching = false;
                s.found = found;
            });
        }
    });
}

/// A freeze while OpenGL drew, its window thread stuck inside OpenGL's driver (#847): remembered,
/// so the app's next start draws another way (renderer.rs; Windows only). Read from the stack's
/// modules, which need no symbols.
fn note_gl_freeze(made: &Made) {
    let drew_gl = made
        .notes
        .session
        .as_ref()
        .is_some_and(|s| s.renderer.starts_with("OpenGL"));
    let Some(thread) = made.frozen_window_thread().filter(|_| drew_gl) else {
        return;
    };
    let modules = thread.frames.iter().map(|f| f.module.as_deref());
    // MDM_TEST_GL_STALL=1 (off-screen test copies): the app's stall stands for a driver's.
    let testing =
        crate::offscreen() && std::env::var_os("MDM_TEST_GL_STALL").is_some_and(|v| v == "1");
    if let Some(module) = crate::renderer::gl_driver_frame(modules, testing) {
        let seconds = made.notes.freeze.as_ref().map_or(0, |f| f.seconds);
        crate::renderer::remember_gl_froze(&module, seconds);
    }
}

fn set(shared: &Mutex<Shared>, ctx: &egui::Context, f: impl FnOnce(&mut Shared)) {
    if let Ok(mut s) = shared.lock() {
        f(&mut s);
    }
    ctx.request_repaint();
}

impl eframe::App for ReportWindow {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        // The caption in the window's light or dark, as every MDM window's (Windows would draw
        // it in its own app mode).
        if !self.caption_set {
            // As the app logs its windows (crash.rs), for the log and the tests.
            tracing::info!("window opened: crash-report");
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::SetTheme(
                crate::theme::system_theme(crate::theme::dark_surface()),
            ));
            self.caption_set = true;
        }
        // Inside the screen at a large interface size (#262); what doesn't fit scrolls.
        if !self.fitted && ui.ctx().input(|i| i.viewport().outer_rect).is_some() {
            self.fitted = true;
            let layout = egui::vec2(CLIENT[0], CLIENT[1]);
            crate::scale::fit_window(ui.ctx(), egui::vec2(120.0, 120.0), Some(layout));
        }
        // And through DWM, as the main window's, once the window is there.
        #[cfg(windows)]
        if !self.caption_dwm {
            self.caption_dwm = crate::win32::caption_dark(
                crate::i18n::t("Mister Download Manager crash report"),
                crate::theme::dark_surface(),
            );
        }
        while let Ok(e) = self.events.try_recv() {
            if matches!(e, Event::Recovered) {
                self.recovered = true;
            }
        }
        // MDM's state is checked now and then (a frozen one may recover or be ended).
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::new().fill(winui::BG()))
            .show(ui, |ui| {
                let id = egui::Id::unique("crash-report");
                crate::scale::pane(ui, id, egui::vec2(CLIENT[0], CLIENT[1]), |ui, o| {
                    let _surface = winui::Surface::enter(o.x, o.x + CLIENT[0]);
                    self.body(ui, o);
                });
            });
    }
}

impl ReportWindow {
    fn heading(&self, alive: bool) -> &'static str {
        match self.job.kind {
            // i18n: crash report window heading: the window of MDM stopped answering, then answered again
            Kind::Freeze if self.recovered => {
                crate::i18n::t("Mister Download Manager stopped responding for a while")
            }
            // i18n: crash report window heading: MDM's window doesn't answer (frozen)
            Kind::Freeze => crate::i18n::t("Mister Download Manager stopped responding"),
            // i18n: crash report window heading: an internal error, MDM still running
            Kind::Panic if alive => crate::i18n::t("Mister Download Manager hit an internal error"),
            // i18n: crash report window heading: MDM crashed and is gone
            _ => crate::i18n::t("Mister Download Manager closed unexpectedly"),
        }
    }

    fn body(&mut self, ui: &mut Ui, o: Pos2) {
        let p = ui.painter().clone();
        let alive = self.app_alive();
        let (made, fetching, no_symbols, searching, found) = {
            let s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            (
                s.made.clone(),
                s.fetching,
                s.no_symbols.clone(),
                s.searching,
                s.found.clone(),
            )
        };

        // The app's icon and the heading.
        let icon = self.icon.get_or_insert_with(|| {
            let i = crate::app_picture();
            let img = egui::ColorImage::from_rgba_unmultiplied(
                [i.width as usize, i.height as usize],
                &i.rgba,
            );
            ui.ctx()
                .load_texture("crash-icon", img, egui::TextureOptions::LINEAR)
        });
        egui::Image::new((icon.id(), egui::vec2(32.0, 32.0)))
            .paint_at(ui, r(o, 16.0, 16.0, 32.0, 32.0));
        winui::text_font(
            &p,
            winui::tahoma_bold(),
            o.x + 60.0,
            o.y + 30.0,
            self.heading(alive),
            winui::TEXT(),
        );
        winui::paragraph(
            &p,
            o.x + 60.0,
            o.y + 50.0,
            // i18n: crash report window: what the report is and what Send does
            crate::i18n::t(
                "A report was saved. Sending it to the developers on GitHub helps them fix this; you can read everything it holds below first. Nothing is sent by itself: Send opens the report form in your browser, filled in, for you to check and submit.",
            ),
            winui::TEXT(),
            o.x + 604.0,
            15.0,
        );

        // What the user was doing.
        winui::text(
            &p,
            o.x + 16.0,
            o.y + 118.0,
            // i18n: crash report window: label of the box where the user describes what they did
            crate::i18n::t("What were you doing when it happened? (optional)"),
            winui::TEXT(),
        );
        winui::multiline_light(
            ui,
            r(o, 16.0, 124.0, 588.0, 58.0),
            &mut self.what,
            "crash-what",
            true,
        );

        // The report.
        winui::text(
            &p,
            o.x + 16.0,
            o.y + 202.0,
            // i18n: crash report window: label above the report's text
            crate::i18n::t("The report"),
            winui::TEXT(),
        );
        winui::field_frame(
            &p,
            r(o, 16.0, 208.0, 588.0, 226.0),
            winui::FieldLook::ReadOnly,
        );
        match &made {
            // The report as it is: its lines, left to right in every language (#204).
            Some(m) => winui::scroll_text(
                ui,
                egui::Id::unique("crash-report-text"),
                Rect::from_min_size(o + egui::vec2(18.0, 210.0), egui::vec2(584.0, 222.0)),
                &m.text,
                winui::TEXT(),
                14.0,
                true,
            ),
            None => {
                winui::text(
                    &p,
                    o.x + 24.0,
                    o.y + 226.0,
                    // i18n: crash report window: shown while the crash dump is being read
                    crate::i18n::t("Reading the crash data…"),
                    winui::MUTED(),
                );
            }
        }

        // What is happening behind it.
        let status: Option<String> = if let Some((done, total)) = fetching {
            let mb = |b: u64| crate::i18n::decimal(b as f64 / 1_048_576.0, 1);
            Some(crate::i18n::fmt(
                // i18n: crash report window: downloading the symbols that turn addresses into function names; {done} and {total} are megabytes
                crate::i18n::t(
                    "Making the report readable: fetching the symbols ({done} of {total} MB)…",
                ),
                &[
                    ("done", &mb(done)),
                    ("total", &total.map_or_else(|| "?".to_owned(), mb)),
                ],
            ))
        } else if let Some(f) = &found {
            Some(crate::i18n::fmt(
                // i18n: crash report window: the same problem has an issue on GitHub already; {number} and {title} are the issue's
                crate::i18n::t("Already reported as issue #{number}: {title}"),
                &[("number", &f.number.to_string()), ("title", &f.title)],
            ))
        } else if searching {
            // i18n: crash report window: looking on GitHub for an issue about the same problem
            Some(crate::i18n::t("Looking for this problem on GitHub…").to_owned())
        } else if no_symbols.is_some() {
            // i18n: crash report window: the symbols couldn't be downloaded; the report still works
            Some(crate::i18n::t("The symbols couldn't be fetched: some lines of the report show addresses instead of names (the developers can still read them).").to_owned())
        } else {
            None
        };
        if let Some(s) = self.told.clone().or(status) {
            // One line between the report and the check box: a long one (the symbols' note, a
            // translation, an issue's title) is cut at the window's right margin with "...",
            // and shown whole under the pointer (#164).
            let (rc, cut) =
                winui::text_fit(&p, o.x + 16.0, o.y + 452.0, &s, winui::TEXT(), o.x + 604.0);
            if cut {
                winui::full_on_hover(ui, rc, "crash-status", &s);
            }
        }
        if let Some(d) = made.as_ref().and_then(|m| m.dump.as_ref())
            && d.exists()
        {
            winui::checkbox(
                ui,
                o + egui::vec2(16.0, 462.0),
                &mut self.attach,
                // i18n: crash report window: checkbox; the .dmp file holds what MDM had in memory
                "Also attach the crash data (.dmp): it may hold addresses, cookies and passwords",
                true,
            );
        }

        // The buttons, each as wide as its label in this language (#204): the row fills the
        // window's width, those wider than their label giving room to those that need it.
        let y = 500.0;
        let send_label = if found.is_some() {
            // i18n: crash report window button: open the GitHub issue already filed (and copy the report to add it there)
            "Open the reported issue"
        } else {
            // i18n: crash report window button: open GitHub's crash report form filled in
            "Send on GitHub"
        };
        // A frozen MDM can be ended; a gone one started again.
        let frozen = self.job.kind == Kind::Freeze && alive && !self.recovered;
        let restart_label = if frozen {
            // i18n: crash report window button: end the frozen MDM and start it again
            "End and restart MDM"
        } else {
            // i18n: crash report window button: start MDM again
            "Restart MDM"
        };
        let [send_at, copy_at, folder_at, restart_at, close_at] = winui::button_row(
            &p,
            &[
                (send_label, 16.0, 170.0),
                ("Copy report", 192.0, 110.0),
                ("Open folder", 308.0, 110.0),
                (restart_label, 424.0, 112.0),
                ("Close", 542.0, 62.0),
            ],
            6.0,
            16.0,
            260.0,
        )[..] else {
            unreachable!("five buttons")
        };
        let send = winui::button(
            ui,
            r(o, send_at.0, y, send_at.1, 25.0),
            send_label,
            if made.is_some() {
                ButtonKind::Default
            } else {
                ButtonKind::Disabled
            },
            false,
            None,
        )
        .clicked();
        let copy = winui::button(
            ui,
            r(o, copy_at.0, y, copy_at.1, 25.0),
            // i18n: crash report window button: copy the report's text
            "Copy report",
            if made.is_some() {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            },
            false,
            None,
        )
        .clicked();
        let folder = winui::button(
            ui,
            r(o, folder_at.0, y, folder_at.1, 25.0),
            // i18n: crash report window button: show the report's file in its folder
            "Open folder",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked();
        let restart = (!alive || frozen)
            && winui::button(
                ui,
                r(o, restart_at.0, y, restart_at.1, 25.0),
                restart_label,
                ButtonKind::Normal,
                false,
                None,
            )
            .clicked();
        let close = winui::button(
            ui,
            r(o, close_at.0, y, close_at.1, 25.0),
            "Close",
            ButtonKind::Normal,
            false,
            None,
        )
        .clicked();

        let Some(made) = made else {
            if close {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        };
        let full = self.full_text(&made);
        if send {
            if let Some(f) = &found {
                crate::copy_text(ui.ctx(), full.clone());
                crate::launch::open(&f.html_url);
                // i18n: crash report window: after opening the existing issue
                self.told = Some(
                    crate::i18n::t(
                        "The report is on your clipboard: paste it in a comment on the issue.",
                    )
                    .to_owned(),
                );
            } else {
                let url = github::new_issue_url(
                    &format!(
                        "{}: {}",
                        report::description(made.kind, &made.notes),
                        made.signature
                    ),
                    &self.fields(&made),
                    &made.text,
                    "(The report was cut to fit here. The whole of it is in the .txt file in the folder that just opened: please drag it into this box.)",
                );
                crate::launch::open(url);
                crate::dialogs::show_in_folder(&made.path);
                // i18n: crash report window: after Send opened the browser
                self.told = Some(crate::i18n::t("The form opened in your browser. Drag the report's file (and the .dmp, if you chose to) into it, then submit.").to_owned());
            }
            if self.attach
                && let Some(d) = &made.dump
            {
                crate::dialogs::show_in_folder(d);
            }
            tracing::info!("crash report sent to the browser ({})", made.signature);
        }
        if copy {
            crate::copy_text(ui.ctx(), full);
            // i18n: crash report window: after Copy report
            self.told = Some(crate::i18n::t("The report is on your clipboard.").to_owned());
        }
        if folder {
            crate::dialogs::show_in_folder(&made.path);
        }
        if restart {
            if frozen {
                end(self.job.parent);
                std::thread::sleep(Duration::from_millis(500));
            }
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).spawn();
            }
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if close {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// The report with what the user wrote at its top.
    fn full_text(&self, made: &Made) -> String {
        if self.what.trim().is_empty() {
            made.text.clone()
        } else {
            format!("What I was doing:\n{}\n\n{}", self.what.trim(), made.text)
        }
    }

    fn fields(&self, made: &Made) -> github::Fields {
        fields(&made.notes, made.kind, &made.signature, &self.what)
    }
}

/// The Crash report form's fields (.github/ISSUE_TEMPLATE/crash.yml ids), the report aside.
fn fields(notes: &Notes, kind: Kind, signature: &str, what: &str) -> github::Fields {
    let s = notes.session.clone().unwrap_or_default();
    vec![
        ("version", format!("{} ({})", s.version, s.commit)),
        ("os", s.os),
        ("os-version", s.os_version),
        ("install", s.install),
        ("kind", kind.word().to_owned()),
        ("signature", signature.to_owned()),
        ("what", what.trim().to_owned()),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_report_fills_fields_the_crash_form_has() {
        // A Windows checkout may give the form CRLF line ends.
        let form = include_str!("../../../.github/ISSUE_TEMPLATE/crash.yml").replace("\r\n", "\n");
        let f = super::fields(&Default::default(), super::Kind::Freeze, "freeze in x", "");
        for (id, _) in f.iter().chain([("report", String::new())].iter()) {
            let line = format!("id: {id}\n");
            assert!(form.contains(&line), "no field {id} in crash.yml");
            // Text fields only (GitHub's page drops a prefilled dropdown's value).
            let before = form.split(&line).next().unwrap();
            assert!(
                before.ends_with("- type: input\n    ")
                    || before.ends_with("- type: textarea\n    "),
                "{id} isn't a text field"
            );
        }
    }
}
