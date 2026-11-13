//! The Scheduler (Downloads → Scheduler; notes/ui-spec/scheduler-and-misc.md §1). Light in both
//! modes, as IDM's. Queues on the left; on the right the selected queue's Schedule and Files in
//! the queue, or the Download limits page. Edits stay in a draft until Apply, Start now or Close.

use chrono::{Datelike, Duration as Days, NaiveDate};
use egui::{Color32, Pos2, Rect, Sense, Ui, pos2, vec2};

use crate::chrome::{IconKey, ShellIcons};
use crate::dialogs::{Outcome, window};
use crate::icons::Icon;
use crate::model::{self, Item};
use crate::queue::{Queue, TurnOff};
use crate::regional::DateOrder;
use crate::settings::Settings;
// Laid out in English pixels: `fill` makes each one where it is on screen, `text` starts at an
// English pen, and a widget given a screen rect (from `r`) turns it back into its English rect
// (`winui::rect` both ways) before laying out its inside.
use crate::winui::{
    self, ButtonKind, LIGHT_BG, LIGHT_DISABLED, LIGHT_PAGE, LIGHT_TEXT, LightTab, fill_l as fill,
    r, text,
};

/// What the app does for the Scheduler.
pub enum Action {
    /// The edited queues (in order; new ones appended, deleted ones gone) and Download limits.
    Apply {
        queues: Vec<Queue>,
        limits: Limits,
    },
    Start(String),
    Stop(String),
}

/// Scheduler → Download limits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    pub on: bool,
    pub mb: u32,
    pub hours: u32,
    pub warn: bool,
}

impl Limits {
    pub fn from_settings(s: &Settings) -> Self {
        Limits {
            on: s.limits_on,
            mb: s.limits_mb,
            hours: s.limits_hours,
            warn: s.limits_warn,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Schedule,
    Files,
}

/// One queue's Schedule tab as edited. Times and numbers keep their values while their
/// checkbox is off, as IDM's pickers do.
#[derive(Clone, PartialEq)]
struct Form {
    start_on_startup: bool,
    start_at_on: bool,
    start_at: (u8, u8, u8),
    once: bool,
    date: NaiveDate,
    days: [bool; 7],
    stop_at_on: bool,
    stop_at: (u8, u8, u8),
    retries_on: bool,
    retries: String,
    open_on: bool,
    open_file: String,
    hang_up: bool,
    exit: bool,
    turn_off_on: bool,
    turn_off: usize,
    force: bool,
    wake: bool,
    files_at_once: String,
}

impl Form {
    fn of(q: &Queue) -> Self {
        let s = &q.schedule;
        let today = crate::clock::now().date_naive();
        Form {
            start_on_startup: s.start_on_startup,
            start_at_on: s.start_at.is_some(),
            start_at: s.start_at.unwrap_or((23, 0, 0)),
            once: s.once.is_some(),
            date: s
                .once
                .and_then(|(y, m, d)| NaiveDate::from_ymd_opt(y, m, d))
                .unwrap_or(today),
            days: s.days,
            stop_at_on: s.stop_at.is_some(),
            stop_at: s.stop_at.unwrap_or((7, 30, 0)),
            retries_on: s.retries.is_some(),
            retries: s.retries.unwrap_or(10).to_string(),
            open_on: s.open_when_done.is_some(),
            open_file: s.open_when_done.clone().unwrap_or_default(),
            hang_up: s.hang_up,
            exit: s.exit_when_done,
            turn_off_on: s.turn_off.is_some(),
            turn_off: s
                .turn_off
                .and_then(|t| TurnOff::ALL.iter().position(|x| *x == t))
                .unwrap_or(0),
            force: s.force_close,
            wake: s.wake,
            files_at_once: q.files_at_once.to_string(),
        }
    }

    /// `q` with this form's settings.
    fn apply_to(&self, q: &mut Queue) {
        let s = &mut q.schedule;
        s.start_on_startup = self.start_on_startup;
        s.start_at = self.start_at_on.then_some(self.start_at);
        s.once = (self.start_at_on && self.once)
            .then(|| (self.date.year(), self.date.month(), self.date.day()));
        s.days = self.days;
        s.stop_at = self.stop_at_on.then_some(self.stop_at);
        s.retries = self
            .retries_on
            .then(|| self.retries.parse().unwrap_or(10).max(1));
        s.open_when_done = (self.open_on && !self.open_file.trim().is_empty())
            .then(|| self.open_file.trim().to_owned());
        s.hang_up = self.hang_up;
        s.exit_when_done = self.exit;
        s.turn_off = self.turn_off_on.then(|| TurnOff::ALL[self.turn_off]);
        s.force_close = self.turn_off_on && self.force;
        s.wake = self.wake;
        q.files_at_once = self.files_at_once.parse().unwrap_or(4).clamp(1, 99);
    }
}

/// The picker field being edited: (picker id, field index), with digits typed so far.
#[derive(Default)]
struct Focus {
    at: Option<(&'static str, usize)>,
    typed: String,
}

pub struct Scheduler {
    queues: Vec<Queue>,
    forms: Vec<Form>,
    limits: Limits,
    limits_mb: String,
    limits_hours: String,
    /// What was last applied: Apply is enabled while the draft differs.
    applied: (Vec<Queue>, Limits),
    /// Index into the tree: a queue, or `queues.len()` for Download limits.
    selected: usize,
    tab: Tab,
    /// The queue list's and the files list's scroll and selection.
    tree_rows: winui::RowList,
    files_rows: winui::RowList,
    /// Each queue's downloads as the app last had them ([`Scheduler::sync_items`]).
    base: std::collections::HashMap<String, Vec<u64>>,
    focus: Focus,
    /// The "Enter queue name" prompt, while open.
    naming: Option<String>,
    /// Delete's question, for this queue, while it is asked.
    deleting: Option<(usize, crate::dialogs::Confirm)>,
}

/// How much lower everything under "Start download at" sits than in IDM's window: the room for
/// "Wake the computer to run this queue" and its note (#142).
const DY: f32 = 40.0;

// i18n: checkbox in the Scheduler, under the start time: wake the computer from sleep shortly before the queue starts, and keep it awake while the queue runs
const WAKE_LABEL: &str = crate::i18n::mark("Wake the computer to run this queue");

// i18n: the Scheduler's page (and checkbox) that caps how much is downloaded in a time
const LIMITS_LABEL: &str = crate::i18n::mark("Download limits");

/// A new queue's proposed name: "Queue # 3".
fn queue_name(n: usize) -> String {
    crate::i18n::fmt(
        // i18n: the name proposed for a new queue; {n} = its number
        crate::i18n::t("Queue # {n}"),
        &[("n", &crate::i18n::integer(n as u64))],
    )
}

impl Scheduler {
    pub fn new(queues: &[Queue], settings: &Settings) -> Self {
        let mut queues = queues.to_vec();
        for q in &mut queues {
            q.running = false;
        }
        let forms = queues.iter().map(Form::of).collect();
        let base = queues
            .iter()
            .map(|q| (q.name.clone(), q.items.clone()))
            .collect();
        let limits = Limits::from_settings(settings);
        let mut s = Scheduler {
            limits_mb: limits.mb.to_string(),
            limits_hours: limits.hours.to_string(),
            applied: (Vec::new(), limits.clone()),
            limits,
            forms,
            queues,
            selected: 0,
            tab: Tab::Schedule,
            tree_rows: Default::default(),
            files_rows: Default::default(),
            focus: Focus::default(),
            naming: None,
            deleting: None,
            base,
        };
        s.applied = s.state();
        s
    }

    /// Show queue `q`: its files (`files`) or its schedule (the queue's right-click menu).
    pub fn show_queue(&mut self, q: usize, files: bool) {
        self.selected = q.min(self.queues.len().saturating_sub(1));
        self.tab = if files { Tab::Files } else { Tab::Schedule };
        self.files_rows.selected = None;
    }

    /// Ask for a new queue's name (Create new queue in a right-click menu).
    pub fn new_queue(&mut self) {
        self.naming = Some(queue_name(self.queues.len() + 1));
        self.files_rows.selected = None;
    }

    /// The draft as the app would store it.
    fn state(&self) -> (Vec<Queue>, Limits) {
        let mut queues = self.queues.clone();
        for (q, f) in queues.iter_mut().zip(&self.forms) {
            f.apply_to(q);
        }
        let mut limits = self.limits.clone();
        limits.mb = self.limits_mb.parse().unwrap_or(200).max(1);
        limits.hours = self.limits_hours.parse().unwrap_or(5).max(1);
        (queues, limits)
    }

    fn apply(&mut self, actions: &mut Vec<Action>) {
        let (queues, limits) = self.state();
        if (&queues, &limits) != (&self.applied.0, &self.applied.1) {
            actions.push(Action::Apply {
                queues: queues.clone(),
                limits: limits.clone(),
            });
            self.applied = (queues, limits);
        }
    }

    /// The queues' downloads as the app has them now (`live`), merged into the draft: added or
    /// deleted in the main window while the Scheduler is open is kept, and so is what the
    /// Scheduler moved or removed itself.
    fn sync_items(&mut self, live: &[Queue]) {
        for (k, q) in self.queues.iter_mut().enumerate() {
            let Some(now) = live.iter().find(|l| l.name == q.name) else {
                continue;
            };
            let base = self
                .base
                .entry(q.name.clone())
                .or_insert_with(|| now.items.clone());
            if *base == now.items {
                continue;
            }
            q.items = merge_items(base, &q.items, &now.items);
            if let Some(a) = self.applied.0.get_mut(k).filter(|a| a.name == q.name) {
                a.items = merge_items(base, &a.items, &now.items);
            }
            *base = now.items.clone();
        }
    }

    /// Draw the dialog. `live` is the app's queues (what runs, their downloads now). Returns
    /// false once closed.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        icons: &mut ShellIcons,
        items: &[Item],
        live: &[Queue],
        wake_error: Option<&str>,
        actions: &mut Vec<Action>,
    ) -> bool {
        self.sync_items(live);
        let running: Vec<String> = live
            .iter()
            .filter(|q| q.running)
            .map(|q| q.name.clone())
            .collect();
        let running = running.as_slice();
        let naming = self.naming.is_some() || self.deleting.is_some();
        let outcome = window(
            ctx,
            "scheduler",
            // i18n: noun: the tool that starts and stops queues on time
            "Scheduler",
            [651.0, 465.0 + DY],
            false,
            LIGHT_BG(),
            |ui, o| self.body(ui, o, icons, items, running, wake_error, actions, naming),
        );
        if self.naming.is_some() {
            self.name_prompt(ctx);
        }
        // Delete asks first: a queue's schedule can't be had back.
        if let Some((q, ask)) = &mut self.deleting {
            match ask.show(ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.deleting = None,
                Outcome::Done(()) => {
                    let q = *q;
                    self.deleting = None;
                    if q < self.queues.len() {
                        self.queues.remove(q);
                        self.forms.remove(q);
                        self.selected = q - 1;
                        self.files_rows.selected = None;
                    }
                }
            }
        }
        if !matches!(outcome, Outcome::Open) {
            // Closed: the calendar doesn't open again with the next Scheduler.
            close_pickers(ctx, &mut self.focus);
        }
        if matches!(outcome, Outcome::Closed) {
            // Close keeps the changes (as Apply would) [IDM: unknown, lab].
            self.apply(actions);
            return false;
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn body(
        &mut self,
        ui: &mut Ui,
        o: Pos2,
        icons: &mut ShellIcons,
        items: &[Item],
        running: &[String],
        wake_error: Option<&str>,
        actions: &mut Vec<Action>,
        modal: bool,
    ) -> Outcome<()> {
        let p = ui.painter().clone();

        // ---- left pane ----------------------------------------------------------------
        let clock = r(o, 11.0, 7.0, 32.0, 32.0);
        Icon::Scheduler.image(true).paint_at(ui, clock);
        // i18n: plural noun: label above the list of queues
        text(
            &p,
            o.x + 74.0,
            o.y + 34.0,
            crate::i18n::t("Queues"),
            LIGHT_TEXT(),
        );
        let tree = r(o, 11.0, 44.0, 173.0, 379.0 + DY);
        let delete_key = self.tree(ui, icons, tree);
        // The bottom row (the queue's buttons under the tree, the window's beside it), each as
        // wide as its label in this language (#192): a longer one moves its neighbours along.
        let row = winui::button_row(
            &ui.painter().clone(),
            &[
                ("&New queue", 21.0, 73.0),
                ("&Delete", 102.0, 73.0),
                ("&Start now", 216.0, 73.0),
                ("S&top", 297.0, 73.0),
                ("&Help", 390.0, 73.0),
                ("&Apply", 484.0, 73.0),
                ("&Close", 565.0, 73.0),
            ],
            8.0,
            21.0,
            200.0,
        );
        let new_queue = winui::button_light(
            ui,
            r(o, row[0].0, 432.0 + DY, row[0].1, 21.0),
            "&New queue",
            ButtonKind::Normal,
            false,
        )
        .clicked();
        let deletable = self.selected >= 2 && self.selected < self.queues.len();
        let delete = winui::button_light(
            ui,
            r(o, row[1].0, 432.0 + DY, row[1].1, 21.0),
            // i18n: verb: delete the queue
            "&Delete",
            if deletable {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            },
            false,
        )
        .clicked();
        // The line between the panes runs down beside the buttons; it stops above them where a
        // longer label takes a button across it.
        let across = row[1].0 + row[1].1 > 197.0 - 4.0 || row[2].0 < 197.0 + 4.0;
        let line_h = if across {
            432.0 + DY - 6.0 - 11.0
        } else {
            442.0 + DY
        };
        winui::etched_v(&p, o.x + 197.0, o.y + 11.0, line_h);

        // ---- right pane ---------------------------------------------------------------
        let on_queue = self.selected < self.queues.len();
        if on_queue {
            // A built-in queue's name is in the catalog; the user's own stay as typed.
            let name = crate::i18n::t(&self.queues[self.selected].name).to_owned();
            let w = winui::text_width(&p, &name);
            let (rect, cut) = winui::text_fit(
                &p,
                (o.x + 412.0 - w / 2.0).max(o.x + 203.0).round(),
                o.y + 15.0,
                &name,
                LIGHT_TEXT(),
                o.x + 640.0,
            );
            if cut {
                winui::full_on_hover(ui, rect, "sched-header", &name);
            }
            let tabs = [
                LightTab {
                    // i18n: noun: tab title; when the queue starts and stops
                    label: "Schedule",
                    sel: (213.0, 271.0),
                    unsel: (215.0, 269.0),
                },
                LightTab {
                    label: "Files in the queue",
                    sel: (268.0, 367.0),
                    unsel: (271.0, 363.0),
                },
            ];
            let current = if self.tab == Tab::Schedule { 0 } else { 1 };
            if let Some(t) =
                winui::tabs_light(ui, o, r(o, 213.0, 23.0, 428.0, 403.0 + DY), &tabs, current)
                    .or(winui::tab_keys(ui, 2, current))
            {
                self.tab = if t == 0 { Tab::Schedule } else { Tab::Files };
            }
            match self.tab {
                Tab::Schedule => self.schedule_page(ui, o, wake_error),
                Tab::Files => self.files_page(ui, o, icons, items),
            }
        } else {
            self.limits_page(ui, o);
        }

        // ---- bottom buttons -----------------------------------------------------------
        let is_running = on_queue && running.contains(&self.queues[self.selected].name);
        let kind = |on: bool| {
            if on {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            }
        };
        let btn = |ui: &mut Ui, (x, w): (f32, f32), label: &str, k: ButtonKind| {
            winui::button_light(ui, r(o, x, 432.0 + DY, w, 21.0), label, k, false).clicked()
        };
        // i18n: verb: start the queue now
        let start = btn(
            ui,
            row[2],
            crate::i18n::mark("&Start now"),
            kind(on_queue && !is_running),
        );
        // i18n: verb: stop the queue
        let stop = btn(ui, row[3], crate::i18n::mark("S&top"), kind(is_running));
        // i18n: noun: button that opens the help
        if btn(ui, row[4], crate::i18n::mark("&Help"), ButtonKind::Normal) {
            crate::help::open_page("queues-and-scheduler.md#the-scheduler-window");
        }
        let dirty = {
            let (q, l) = self.state();
            (&q, &l) != (&self.applied.0, &self.applied.1)
        };
        // i18n: verb: save the changes
        let apply = btn(ui, row[5], crate::i18n::mark("&Apply"), kind(dirty));
        // i18n: verb: close the window
        let close = btn(ui, row[6], crate::i18n::mark("&Close"), ButtonKind::Normal);

        if modal {
            return Outcome::Open;
        }
        if new_queue {
            self.naming = Some(queue_name(self.queues.len() + 1));
        }
        if (delete || delete_key) && deletable {
            let name = crate::i18n::t(&self.queues[self.selected].name).to_owned();
            let text = crate::i18n::fmt(
                // i18n: asked before deleting a queue; its downloads stay in the list
                crate::i18n::t("Delete the queue \"{queue}\"? Its downloads stay in the list."),
                &[("queue", &crate::i18n::isolate(&name))],
            );
            self.deleting = Some((
                self.selected,
                crate::dialogs::Confirm::new(crate::i18n::mark("Delete queue"), text),
            ));
        }
        if apply {
            self.apply(actions);
        }
        if start && on_queue {
            self.apply(actions);
            actions.push(Action::Start(self.queues[self.selected].name.clone()));
        }
        if stop && on_queue {
            actions.push(Action::Stop(self.queues[self.selected].name.clone()));
        }
        // Escape first leaves a selected time or date field, then closes the window.
        if self.focus.at.is_some() && winui::take_key(ui, egui::Key::Escape) {
            self.focus.at = None;
        }
        let escape = winui::escape_pressed(ui);
        if close || escape {
            Outcome::Closed
        } else {
            Outcome::Open
        }
    }

    /// The queue list (a tree view in IDM): queues, then Download limits, in the shared row
    /// list. True when Delete is pressed on it.
    fn tree(&mut self, ui: &mut Ui, icons: &mut ShellIcons, rc: Rect) -> bool {
        let queues = &self.queues;
        let n = queues.len() + 1;
        self.tree_rows.selected = Some(self.selected);
        // Named for screen readers (#102), as the tree's caption reads.
        self.tree_rows.name = crate::i18n::t("Queues");
        let ev = winui::row_list(
            ui,
            "sched-tree",
            rc,
            n,
            16.0,
            &mut self.tree_rows,
            |ui, clip, i, row, look| {
                let (label, icon) = match queues.get(i) {
                    Some(q) => (
                        crate::i18n::t(q.name.as_str()),
                        if q.sync { "folder_link" } else { "folder" },
                    ),
                    None => (crate::i18n::t(LIMITS_LABEL), "folder_delete"),
                };
                look.name = label.to_owned();
                icons.paint_in(
                    ui,
                    clip,
                    IconKey::Silk(icon),
                    pos2(row.left() + 4.0, row.top()),
                );
                winui::fit(
                    ui,
                    clip,
                    row.left() + 24.0,
                    row.top() + 11.0,
                    label,
                    look.ink,
                    row.right() - 3.0,
                    ("sched-tree-full", i),
                );
            },
        );
        let queues = &self.queues;
        if ev.focused
            && self.tree_rows.type_to(ui, "sched-tree", n, |i| {
                queues
                    .get(i)
                    .map_or(crate::i18n::t(LIMITS_LABEL).to_owned(), |q| {
                        crate::i18n::t(&q.name).to_owned()
                    })
            })
            && let Some(i) = self.tree_rows.selected
        {
            self.selected = i;
            self.files_rows = Default::default();
        }
        if let Some(i) = ev.picked {
            self.selected = i;
            self.files_rows = Default::default();
            // Another queue: its own date, not this one's calendar left open over it.
            close_pickers(ui.ctx(), &mut self.focus);
        }
        ev.delete.is_some()
    }

    fn schedule_page(&mut self, ui: &mut Ui, o: Pos2, wake_error: Option<&str>) {
        let p = ui.painter().clone();
        let sync = self.queues[self.selected].sync;
        let f = &mut self.forms[self.selected];
        let at = |x: f32, y: f32| o + vec2(x, y);
        winui::radio_light(ui, at(228.0, 51.0), !sync, "One-time downloading", !sync);
        winui::radio_light(ui, at(429.0, 51.0), sync, "Periodic synchronization", sync);
        winui::etched_h(&p, o.x + 228.0, o.y + 71.0, 399.0);
        winui::checkbox_light(
            ui,
            at(228.0, 79.0),
            &mut f.start_on_startup,
            "Start download on Mister Download Manager startup",
            true,
        );
        winui::etched_h(&p, o.x + 228.0, o.y + 99.0, 399.0);
        // A box after a caption starts where IDM has it, or after a longer translated caption,
        // as far as the page allows (`last`: its rightmost start); the caption ends in "..."
        // before it when even that is not enough.
        let after = |glyph_x: f32, caption: &str, idm: f32, last: f32| {
            let end = glyph_x + 16.0 + winui::text_width(&p, crate::i18n::t(caption)).ceil() + 6.0;
            end.clamp(idm, last.max(idm))
        };
        let start_x = after(228.0, "Start download at", 382.0, 627.0 - 89.0);
        winui::checkbox_light_until(
            ui,
            at(228.0, 114.0),
            &mut f.start_at_on,
            "Start download at",
            true,
            Some(o.x + start_x),
        );
        let on = f.start_at_on;
        time_picker(
            ui,
            r(o, start_x, 108.0, 89.0, 24.0),
            &mut f.start_at,
            on,
            "start-at",
            &mut self.focus,
        );
        let date_x = after(235.0, "Once at", 337.0, 627.0 - 188.0);
        let once = winui::radio_light_until(
            ui,
            at(235.0, 144.0),
            f.once,
            "Once at",
            on,
            Some(o.x + date_x),
        );
        if once.clicked() {
            f.once = true;
        }
        date_picker(
            ui,
            r(o, date_x, 141.0, 188.0, 24.0),
            &mut f.date,
            on && f.once,
            "once-at",
            &mut self.focus,
        );
        // The week's columns move right together after a longer "Daily", while the last one
        // still fits the page.
        let last_day = 16.0 + winui::text_width(&p, crate::i18n::t(WEEKDAYS[6])).ceil() + 4.0;
        let shift = after(235.0, "Daily", 310.0, 627.0 - last_day - 221.0) - 310.0;
        // i18n: adverb: every day (the other choice is once, on a date)
        let daily = winui::radio_light_until(
            ui,
            at(235.0, 173.0),
            !f.once,
            "Daily",
            on,
            Some(o.x + 310.0 + shift),
        );
        if daily.clicked() {
            f.once = false;
        }
        // The arrows between them, as in a Windows radio group.
        if let Some(i) = winui::radio_keys(ui, &[once, daily]) {
            f.once = i == 0;
        }
        let days = [
            (WEEKDAYS[0], 310.0, 173.0),
            (WEEKDAYS[1], 310.0, 189.0),
            (WEEKDAYS[2], 310.0, 205.0),
            (WEEKDAYS[3], 420.0, 173.0),
            (WEEKDAYS[4], 420.0, 189.0),
            (WEEKDAYS[5], 420.0, 205.0),
            (WEEKDAYS[6], 531.0, 173.0),
        ];
        for (i, (day, x, y)) in days.iter().enumerate() {
            // A long day name stops before the next column.
            let next = match *x as i32 {
                310 => Some(o.x + 420.0 + shift),
                420 => Some(o.x + 531.0 + shift),
                _ => None,
            };
            winui::checkbox_light_until(
                ui,
                at(*x + shift, *y),
                &mut f.days[i],
                day,
                on && !f.once,
                next,
            );
        }
        // A start that can never come says so where it is set (the queue just never ran).
        if f.start_at_on {
            let today = crate::clock::now().date_naive();
            let (bad, rc, why) = if f.once {
                (
                    f.date < today,
                    r(o, date_x, 141.0, 188.0, 24.0),
                    // i18n: tooltip on the Scheduler's date when it is in the past
                    "This date has passed: the queue will not start",
                )
            } else {
                (
                    !f.days.iter().any(|d| *d),
                    r(o, 306.0 + shift, 169.0, 321.0 - shift, 54.0),
                    // i18n: tooltip on the Scheduler's weekdays when none is ticked
                    "No day is ticked: the queue will not start",
                )
            };
            if bad {
                winui::error_frame(&p, rc);
                ui.interact(
                    rc,
                    egui::Id::unique(("start-warning", self.selected)),
                    Sense::hover(),
                )
                .on_hover_text(crate::i18n::t(why));
            }
        }
        self.wake_row(ui, o, wake_error);
        let f = &mut self.forms[self.selected];
        winui::etched_h(&p, o.x + 229.0, o.y + 227.0 + DY, 398.0);
        let stop_x = after(229.0, "Stop download at", 384.0, 627.0 - 89.0);
        winui::checkbox_light_until(
            ui,
            at(229.0, 243.0 + DY),
            &mut f.stop_at_on,
            "Stop download at",
            true,
            Some(o.x + stop_x),
        );
        time_picker(
            ui,
            r(o, stop_x, 238.0 + DY, 89.0, 23.0),
            &mut f.stop_at,
            f.stop_at_on,
            "stop-at",
            &mut self.focus,
        );
        // The caption stops before the box after it on the row (long translations are cut).
        winui::checkbox_light_until(
            ui,
            at(229.0, 267.0 + DY),
            &mut f.retries_on,
            "Number of retries for each file if downloading failed :",
            true,
            Some(o.x + 541.0),
        );
        winui::number_field(
            ui,
            r(o, 541.0, 260.0 + DY, 48.0, 23.0),
            &mut f.retries,
            1,
            9999,
            f.retries_on,
            LIGHT_PAGE(),
        );
        winui::etched_h(&p, o.x + 229.0, o.y + 290.0 + DY, 398.0);
        winui::checkbox_light(
            ui,
            at(228.0, 300.0 + DY),
            &mut f.open_on,
            "Open the following file when done:",
            true,
        );
        winui::edit_light_on(
            ui,
            r(o, 228.0, 318.0 + DY, 356.0, 23.0),
            &mut f.open_file,
            f.open_on,
            false,
            LIGHT_PAGE(),
        );
        let browse = winui::button_light(
            ui,
            r(o, 592.0, 319.0 + DY, 21.0, 21.0),
            "...",
            if f.open_on {
                ButtonKind::Normal
            } else {
                ButtonKind::Disabled
            },
            false,
        )
        .clicked();
        if browse && f.open_on {
            let title = crate::i18n::t("Open the following file when done").to_owned();
            winui::start_pick(ui.ctx(), "sched-open-file", move || {
                crate::winui::file_dialog(&title).pick_file()
            });
        }
        if let Some(path) = winui::picked(ui.ctx(), "sched-open-file") {
            f.open_file = path.display().to_string();
        }
        // Dial-up is hung up through Windows' rasdial; macOS and Linux have no such command.
        let hang_up = winui::checkbox_light(
            ui,
            at(228.0, 348.0 + DY),
            &mut f.hang_up,
            "Hang up modem when done",
            cfg!(windows),
        );
        winui::why_disabled(hang_up, !cfg!(windows), crate::progress::NO_DIAL_UP);
        winui::checkbox_light(
            ui,
            at(228.0, 366.0 + DY),
            &mut f.exit,
            "Exit Mister Download Manager when done",
            true,
        );
        // The drop-down as wide as its choices in this language need (#192), widening to the
        // left (its right end kept), up to 8 px after the caption, which stays whole.
        let caption_end = winui::checkbox_caption_end(&p, 228.0, "Turn off computer when done");
        let turn_off_w =
            winui::combo_fit_width(&p, &TurnOff::LABELS, 119.0, 627.0 - (caption_end + 8.0));
        let turn_off_x = 627.0 - turn_off_w;
        winui::checkbox_light_until(
            ui,
            at(228.0, 384.0 + DY),
            &mut f.turn_off_on,
            "Turn off computer when done",
            true,
            Some(o.x + turn_off_x),
        );
        if let Some(i) = winui::combo_light(
            ui,
            r(o, turn_off_x, 383.0 + DY, turn_off_w, 21.0),
            &TurnOff::LABELS,
            f.turn_off,
            f.turn_off_on,
            "turn-off",
        ) {
            f.turn_off = i;
        }
        winui::checkbox_light(
            ui,
            at(246.0, 402.0 + DY),
            &mut f.force,
            "Force processes to terminate",
            f.turn_off_on,
        );
    }

    /// "Wake the computer to run this queue" under the start time, and a line under it: when
    /// the computer will be woken, or why it can't be (#142).
    fn wake_row(&mut self, ui: &mut Ui, o: Pos2, wake_error: Option<&str>) {
        use crate::wake::Support;
        let p = ui.painter().clone();
        let support = crate::wake::support();
        let mut q = self.queues[self.selected].clone();
        let f = &mut self.forms[self.selected];
        let can = support.can_tick();
        let enabled = f.start_at_on && can;
        // Where waking can't be, the box shows it off (the setting is kept, for a data folder
        // carried to a system that can).
        let mut shown = f.wake && can;
        let resp =
            winui::checkbox_light(ui, o + vec2(228.0, 229.0), &mut shown, WAKE_LABEL, enabled);
        if enabled {
            f.wake = shown;
        }
        let why = if !can {
            support_reason(&support).unwrap_or("")
        } else {
            // i18n: tooltip on the Scheduler's "Wake the computer to run this queue" while "Start download at" is off
            "Tick \"Start download at\" first: the computer is woken for the start time"
        };
        let resp = winui::why_disabled(resp, !enabled, why);
        if enabled {
            resp.on_hover_text(crate::i18n::t(
                // i18n: tooltip on the Scheduler's "Wake the computer to run this queue"
                "The computer is woken a minute before the start time and kept awake while the queue runs. Mister Download Manager has to be running (it can wait in the notification area).",
            ));
        }
        // The line under it: a problem (in the error colour when it stops the wake), else when
        // the computer wakes.
        let (note, link, error): (String, Option<&str>, bool) = match &support {
            Support::NeedsAdmin | Support::NoAlarm | Support::MacAdmin => (
                crate::i18n::t(support_reason(&support).unwrap_or("")).to_owned(),
                // i18n: link after why the computer can't be woken (Linux, macOS): opens the guide's page that shows how an administrator can allow it
                (support != Support::NoAlarm).then_some(crate::i18n::mark("How to allow it")),
                false,
            ),
            _ if !(f.wake && f.start_at_on) => (String::new(), None, false),
            Support::TimersOff { .. } | Support::NotOnBattery => (
                crate::i18n::t(support_reason(&support).unwrap_or("")).to_owned(),
                // i18n: link after "Windows' power options don't allow wake timers": opens Power Options' advanced settings, where it can be changed
                Some(crate::i18n::mark("Change")),
                matches!(support, Support::TimersOff { .. }),
            ),
            Support::Ready => match wake_error {
                Some(e) => (e.to_owned(), None, true),
                None => {
                    f.apply_to(&mut q);
                    let now = crate::clock::now().naive_local();
                    match crate::wake::next_start(&q.schedule, now) {
                        Some(start) => (wake_note(crate::wake::wake_for(start, now)), None, false),
                        None => (String::new(), None, false),
                    }
                }
            },
        };
        if note.is_empty() {
            return;
        }
        let (x, right, baseline) = (244.0, 627.0, 257.0);
        let lw = link.map_or(0.0, |l| {
            winui::text_width(&p, crate::i18n::t(l)).ceil() + 8.0
        });
        let room = (right - x - lw).max(0.0);
        let color = if error { winui::ERROR() } else { LIGHT_TEXT() };
        winui::fit(
            ui,
            &p,
            o.x + x,
            o.y + baseline,
            &note,
            color,
            o.x + x + room,
            "sched-wake-note",
        );
        let Some(l) = link else {
            return;
        };
        let end = x + winui::text_width(&p, &note).ceil().min(room) + 8.0;
        if winui::link(ui, &p, o.x + end, o.y + baseline, l, "sched-wake-link") {
            match support {
                #[cfg(windows)]
                Support::TimersOff { .. } | Support::NotOnBattery => {
                    crate::win32::open_power_options()
                }
                // The guide's section on it (never from an off-screen test run).
                _ if !crate::offscreen() => crate::help::open_page(
                    "queues-and-scheduler.md#wake-the-computer-to-run-a-queue",
                ),
                _ => {}
            }
        }
    }

    fn files_page(&mut self, ui: &mut Ui, o: Pos2, icons: &mut ShellIcons, items: &[Item]) {
        let p = ui.painter().clone();
        let boxes = winui::sentence(
            ui,
            &p,
            // i18n: {files} is a number box (with up/down arrows), drawn where it stands in the translation
            crate::i18n::t("Download {files} files at the same time"),
            &[winui::SentenceLine {
                lead: winui::Lead::Left(o.x + 242.0),
                lead_baseline: o.y + 66.0,
                boxes: o + vec2(297.0, 53.0),
                gap: 5.0,
                baseline: o.y + 66.0,
            }],
            &[("files", vec2(39.0, 23.0))],
            |p, x, baseline, s| {
                text(p, x, baseline, s, LIGHT_TEXT());
            },
        );
        let f = &mut self.forms[self.selected];
        // A spin box: the number and its arrows, one control.
        winui::spin_box(
            ui,
            boxes[0],
            &mut f.files_at_once,
            1,
            99,
            true,
            "files-at-once",
        );

        // The queue's files: File Name, Size, Status, Time left.
        let list = r(o, 229.0, 82.0, 396.0, 294.0 + DY);
        crate::options::list_frame(&p, list);
        fill(
            &p,
            o.x + 230.0,
            o.y + 83.0,
            o.x + 624.0,
            o.y + 106.0,
            winui::LIST_HEADER(),
        );
        let cols = [
            (crate::i18n::mark("File Name"), 230.0, 421.0),
            // i18n: column title: the file's size
            (crate::i18n::mark("Size"), 421.0, 488.0),
            // i18n: column title: the download's state
            (crate::i18n::mark("Status"), 488.0, 540.0),
            (crate::i18n::mark("Time left"), 540.0, 607.0),
        ];
        let grid = winui::GRID();
        for (title, x0, x1) in cols {
            let title = crate::i18n::t(title);
            text(&p, o.x + x0 + 6.0, o.y + 99.0, title, LIGHT_TEXT());
            fill(
                &p,
                o.x + x1,
                o.y + 83.0,
                o.x + x1,
                o.y + 106.0,
                winui::HEADER_LINE(),
            );
        }
        // The rows: the shared row list under the header (its frame on the list's).
        let body = Rect::from_min_max(o + vec2(230.0, 108.0), o + vec2(624.0, 375.0 + DY));
        let ids = self.queues[self.selected].items.clone();
        // Looked up once a frame (a long queue in a long list was slow to draw).
        let by_id: std::collections::HashMap<u64, &Item> =
            items.iter().map(|i| (i.id, i)).collect();
        let rows: Vec<&Item> = ids.iter().filter_map(|id| by_id.get(id).copied()).collect();
        // Named for screen readers (#102), as its tab reads.
        self.files_rows.name = crate::i18n::t("Files in the queue");
        let ev = winui::row_list(
            ui,
            "sched-files",
            winui::rect(body.expand(1.0)),
            rows.len(),
            17.0,
            &mut self.files_rows,
            |ui, clip, k, row, look| {
                let item = rows[k];
                let top = row.top();
                fill(clip, row.left(), top + 16.0, row.right(), top + 16.0, grid);
                look.name = [
                    item.file_name.clone(),
                    item.size.map(model::size_list).unwrap_or_default(),
                    item.status_text(),
                ]
                .into_iter()
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
                let cells = [
                    // Its own direction (an Arabic name right to left in any window).
                    crate::i18n::isolate(&item.file_name),
                    item.size.map(model::size_list).unwrap_or_default(),
                    item.status_text(),
                    item.time_left(),
                ];
                for (c, ((_, x0, x1), cell)) in cols.iter().zip(&cells).enumerate() {
                    fill(clip, o.x + x1, top, o.x + x1, top + 16.0, grid);
                    let cell_clip = clip.with_clip_rect(
                        winui::rect(Rect::from_min_max(
                            pos2(o.x + x0, top),
                            pos2(o.x + x1 - 1.0, top + 16.0),
                        ))
                        .intersect(clip.clip_rect()),
                    );
                    // The name column: the file's type icon, then its name.
                    let mut tx = o.x + x0 + 6.0;
                    if c == 0 {
                        crate::fileicons::paint_at(
                            ui,
                            &cell_clip,
                            crate::fileicons::Kind::File(&item.file_name),
                            pos2(o.x + x0 + 4.0, top),
                            16.0,
                        );
                        tx = o.x + x0 + 4.0 + crate::fileicons::INDENT;
                    }
                    winui::fit(
                        ui,
                        &cell_clip,
                        tx,
                        top + 12.0,
                        cell,
                        look.ink,
                        o.x + x1 - 3.0,
                        ("sched-cell", k, *x0 as i32),
                    );
                }
            },
        );
        // Delete on a file: Remove from the queue.
        let delete_key = ev.delete.is_some();
        if ev.focused {
            self.files_rows
                .type_to(ui, "sched-files", rows.len(), |i| rows[i].file_name.clone());
        }

        // Move down, move up, remove from the queue.
        let sel = self.files_rows.selected;
        let n = rows.len();
        let buttons = [
            // i18n: tooltip of an icon button in the Scheduler's list of files
            (
                "arrow_down",
                233.0,
                sel.is_some_and(|s| s + 1 < n),
                crate::i18n::mark("Move down"),
            ),
            // i18n: tooltip of an icon button in the Scheduler's list of files
            (
                "arrow_up",
                263.0,
                sel.is_some_and(|s| s > 0),
                crate::i18n::mark("Move up"),
            ),
            // i18n: tooltip of an icon button in the Scheduler's list of files
            (
                "cross",
                290.0,
                sel.is_some(),
                crate::i18n::mark("Remove from the queue"),
            ),
        ];
        for (icon, x, enabled, tip) in buttons {
            let rc = r(o, x, 387.0 + DY, 19.0, 21.0);
            let clicked = winui::icon_button_light(ui, rc, icon, enabled, tip).clicked()
                || (icon == "cross" && delete_key);
            // English, as the icon and its wash take it.
            let img = winui::rect(rc).min + vec2(2.0, 2.0);
            // The theme's icon set may draw them (move up, move down, remove).
            let need = match icon {
                "arrow_up" => "misc-up",
                "arrow_down" => "misc-down",
                _ => "misc-remove",
            };
            icons.paint(ui, IconKey::Themed(need, icon), img);
            if !enabled {
                // Disabled: wash the icon out.
                fill(&p, img.x, img.y, img.x + 15.0, img.y + 15.0, {
                    // The dialog's colour over the icon: greyed.
                    let b = LIGHT_BG();
                    Color32::from_rgba_unmultiplied(b.r(), b.g(), b.b(), 170)
                });
            }
            if !(clicked && enabled) {
                continue;
            }
            let (Some(s), q) = (sel, &mut self.queues[self.selected]) else {
                continue;
            };
            // Positions in `rows` map to the queue's ids in order (missing items are skipped).
            let id = rows[s].id;
            let Some(pos) = q.items.iter().position(|x| *x == id) else {
                continue;
            };
            match icon {
                "arrow_down" => {
                    let next = rows[s + 1].id;
                    let npos = q.items.iter().position(|x| *x == next).unwrap_or(pos);
                    q.items.swap(pos, npos);
                    self.files_rows.selected = Some(s + 1);
                }
                "arrow_up" => {
                    let prev = rows[s - 1].id;
                    let ppos = q.items.iter().position(|x| *x == prev).unwrap_or(pos);
                    q.items.swap(pos, ppos);
                    self.files_rows.selected = Some(s - 1);
                }
                _ => {
                    q.items.remove(pos);
                    self.files_rows.selected = None;
                }
            }
        }
    }

    fn limits_page(&mut self, ui: &mut Ui, o: Pos2) {
        let p = ui.painter().clone();
        let l = &mut self.limits;
        winui::checkbox_light(ui, o + vec2(239.0, 33.0), &mut l.on, LIMITS_LABEL, true);
        let c = if l.on { LIGHT_TEXT() } else { LIGHT_DISABLED() };
        // The labels end where `text_right(380)` ended them: at 381.
        let line = |y: f32, baseline: f32, gap: f32| winui::SentenceLine {
            lead: winui::Lead::Right(o.x + 381.0),
            lead_baseline: o.y + baseline,
            boxes: o + vec2(387.0, y),
            gap,
            baseline: o.y + baseline,
        };
        let boxes = winui::sentence(
            ui,
            &p,
            crate::i18n::t(crate::quota::LIMITS_SENTENCE),
            &[line(64.0, 77.0, 8.0), line(95.0, 108.0, 11.0)],
            &[("mb", vec2(36.0, 20.0)), ("hours", vec2(36.0, 20.0))],
            |p, x, baseline, s| {
                text(p, x, baseline, s, c);
            },
        );
        winui::number_field(
            ui,
            boxes[0],
            &mut self.limits_mb,
            1,
            999_999,
            l.on,
            LIGHT_BG(),
        );
        winui::number_field(
            ui,
            boxes[1],
            &mut self.limits_hours,
            1,
            999,
            l.on,
            LIGHT_BG(),
        );
        winui::checkbox_light(
            ui,
            o + vec2(239.0, 127.0),
            &mut l.warn,
            "Show warning before stopping downloads",
            l.on,
        );
        self.limits_mb.retain(|c| c.is_ascii_digit());
        self.limits_mb.truncate(6);
        self.limits_hours.retain(|c| c.is_ascii_digit());
        self.limits_hours.truncate(3);
    }

    /// New queue → "Enter queue name" (client 279 × 120).
    fn name_prompt(&mut self, ctx: &egui::Context) {
        let Some(mut name) = self.naming.take() else {
            return;
        };
        // The names in use, as stored and as shown (a built-in queue's in the interface's
        // language).
        let names: Vec<String> = self
            .queues
            .iter()
            .flat_map(|q| [q.name.clone(), crate::i18n::t(&q.name).to_owned()])
            .collect();
        let outcome = window(
            ctx,
            "queue-name",
            "Enter queue name",
            [279.0, 120.0],
            false,
            LIGHT_BG(),
            |ui, o| {
                let p = ui.painter().clone();
                text(
                    &p,
                    o.x + 11.0,
                    o.y + 20.0,
                    crate::i18n::t("Enter a name for the new queue that will be"),
                    LIGHT_TEXT(),
                );
                text(
                    &p,
                    o.x + 11.0,
                    o.y + 33.0,
                    crate::i18n::t("displayed in the list of queues"),
                    LIGHT_TEXT(),
                );
                let edit =
                    winui::edit_light(ui, r(o, 11.0, 46.0, 258.0, 23.0), &mut name, true, false);
                if !ui.data(|d| {
                    d.get_temp::<bool>(egui::Id::unique("queue-name-focused"))
                        .unwrap_or(false)
                }) {
                    edit.request_focus();
                    ui.data_mut(|d| d.insert_temp(egui::Id::unique("queue-name-focused"), true));
                }
                // An empty name or one in use holds OK back, saying which.
                let typed = name.trim();
                let why = if typed.is_empty() {
                    // i18n: why OK is greyed when naming a new queue
                    Some("Type the queue's name")
                } else if names
                    .iter()
                    .any(|n| n.to_lowercase() == typed.to_lowercase())
                {
                    // i18n: why OK is greyed when naming a new queue
                    Some("There is already a queue with this name")
                } else {
                    None
                };
                let ok = winui::button_light(
                    ui,
                    r(o, 55.0, 87.0, 73.0, 21.0),
                    "OK",
                    if why.is_some() {
                        ButtonKind::Disabled
                    } else {
                        ButtonKind::Default
                    },
                    false,
                );
                let ok = winui::why_disabled(ok, why.is_some(), why.unwrap_or("")).clicked()
                    && why.is_none();
                if let Some(w) = why.filter(|_| !typed.is_empty()) {
                    winui::fit(
                        ui,
                        &p,
                        o.x + 11.0,
                        o.y + 80.0,
                        crate::i18n::t(w),
                        winui::ERROR(),
                        o.x + 269.0,
                        "queue-name-taken",
                    );
                }
                let cancel = winui::button_light(
                    ui,
                    r(o, 148.0, 87.0, 73.0, 21.0),
                    "Cancel",
                    ButtonKind::Normal,
                    false,
                )
                .clicked();
                if cancel || winui::escape_pressed(ui) {
                    Outcome::Closed
                } else if ok || (why.is_none() && winui::enter_pressed(ui)) {
                    Outcome::Done(())
                } else {
                    Outcome::Open
                }
            },
        );
        match outcome {
            Outcome::Open => self.naming = Some(name),
            Outcome::Done(()) => {
                let name = name.trim();
                let taken = self
                    .queues
                    .iter()
                    .any(|q| q.name.to_lowercase() == name.to_lowercase());
                if name.is_empty() || taken {
                    self.naming = Some(name.to_owned()); // keep asking
                } else {
                    let q = Queue::new(name, false);
                    self.forms.push(Form::of(&q));
                    self.queues.push(q);
                    self.selected = self.queues.len() - 1;
                    self.tab = Tab::Schedule;
                    ctx.data_mut(|d| d.remove::<bool>(egui::Id::unique("queue-name-focused")));
                }
            }
            Outcome::Closed => {
                ctx.data_mut(|d| d.remove::<bool>(egui::Id::unique("queue-name-focused")));
            }
        }
    }
}

/// Why the computer can't be woken, or only partly (English, translated where shown).
fn support_reason(s: &crate::wake::Support) -> Option<&'static str> {
    use crate::wake::Support;
    Some(match s {
        Support::Ready => return None,
        Support::TimersOff { battery: true } => {
            // i18n: the Scheduler, under "Wake the computer to run this queue" (Windows): its power option "Allow wake timers" is off, on battery
            crate::i18n::mark("On battery, Windows' power options don't allow wake timers")
        }
        Support::TimersOff { battery: false } => {
            // i18n: the Scheduler, under "Wake the computer to run this queue" (Windows): its power option "Allow wake timers" is off
            crate::i18n::mark("Windows' power options don't allow wake timers")
        }
        Support::NotOnBattery => {
            // i18n: the Scheduler, under "Wake the computer to run this queue" (Windows): "Allow wake timers" is on when plugged in, off on battery
            crate::i18n::mark("Windows allows wake timers only while plugged in")
        }
        Support::NeedsAdmin => {
            // i18n: the Scheduler, under "Wake the computer to run this queue" (Linux): the computer's wake alarm can be set by an administrator only
            crate::i18n::mark("Only an administrator can allow this")
        }
        Support::NoAlarm => {
            // i18n: the Scheduler, under "Wake the computer to run this queue" (Linux): the computer has no clock chip with a wake alarm that Linux shows
            crate::i18n::mark("This computer has no wake alarm that programs can set")
        }
        Support::MacAdmin => {
            // i18n: the Scheduler, under "Wake the computer to run this queue" (macOS): scheduling a wake needs an administrator's password
            crate::i18n::mark("On macOS, only an administrator can schedule a wake")
        }
    })
}

/// "The computer will be woken on 06/11/2026 at 22:59:00", in the formats in use.
fn wake_note(at: chrono::NaiveDateTime) -> String {
    use chrono::Timelike;
    let time = crate::regional::time_text(
        at.hour(),
        at.minute(),
        at.second(),
        crate::regional::current().clock24,
    );
    crate::i18n::fmt(
        // i18n: the Scheduler, under "Wake the computer to run this queue": when it will happen; {date} = e.g. 06/11/2026, {time} = e.g. 22:59:00
        crate::i18n::t("The computer will be woken on {date} at {time}"),
        &[
            (
                "date",
                &crate::i18n::ltr(&crate::regional::short_date(at.date())),
            ),
            ("time", &crate::i18n::ltr(&time)),
        ],
    )
}

// ---- pickers ----------------------------------------------------------------------------

/// Field text at English pen `x` with the selection look when it is the focused field; returns
/// its rect on screen.
fn field(p: &egui::Painter, x: f32, base: f32, s: &str, selected: bool, enabled: bool) -> Rect {
    let w = winui::text_width(p, s);
    let rc = winui::rect(Rect::from_min_max(
        pos2(x - 1.0, base - 11.0),
        pos2(x + w + 1.0, base + 3.0),
    ));
    if selected && enabled {
        p.rect_filled(rc, 0.0, winui::SELECTION());
    }
    let c = match (enabled, selected) {
        (false, _) => LIGHT_DISABLED(),
        (true, true) => winui::SELECTION_TEXT(),
        (true, false) => LIGHT_TEXT(),
    };
    text(p, x, base, s, c);
    rc
}

/// Keyboard and wheel input for the focused picker field: (step, digit typed, move by).
/// Every picker's field and calendar closed (another queue, the Scheduler closing).
fn close_pickers(ctx: &egui::Context, focus: &mut Focus) {
    focus.at = None;
    focus.typed.clear();
    let key = egui::Id::unique(("date-picker", "once-at"));
    ctx.data_mut(|d| {
        d.remove::<bool>(key);
        d.remove::<NaiveDate>(key.with("month"));
        d.remove::<NaiveDate>(key.with("cursor"));
    });
}

/// A time or date picker is one Tab stop: its field is chosen while it has the keys (Tab in:
/// the first), and none when they go elsewhere (it took arrows and digits meant for the box
/// Tab moved to).
fn picker_focus(ui: &Ui, hold: &egui::Response, id: &'static str, focus: &mut Focus) {
    let ours = focus.at.is_some_and(|(fid, _)| fid == id);
    if hold.has_focus() {
        winui::keep_keys(ui, hold, true, true, false);
        if !ours {
            focus.at = Some((id, 0));
            focus.typed.clear();
        }
    } else if ours && !ui.input(|i| i.pointer.any_down()) {
        focus.at = None;
    }
}

fn picker_keys(ui: &Ui, hovered: bool) -> (i32, Option<char>, i32) {
    ui.input(|i| {
        let mut step = 0;
        if i.key_pressed(egui::Key::ArrowUp) {
            step += 1;
        }
        if i.key_pressed(egui::Key::ArrowDown) {
            step -= 1;
        }
        if hovered {
            for e in &i.events {
                if let egui::Event::MouseWheel { delta, .. } = e
                    && delta.y != 0.0
                {
                    step += delta.y.signum() as i32;
                }
            }
        }
        let digit = i.events.iter().find_map(|e| match e {
            egui::Event::Text(t) => t.chars().find(|c| c.is_ascii_alphanumeric()),
            _ => None,
        });
        let mut mv = 0;
        if i.key_pressed(egui::Key::ArrowRight) {
            mv += 1;
        }
        if i.key_pressed(egui::Key::ArrowLeft) {
            mv -= 1;
        }
        (step, digit, mv)
    })
}

/// IDM's time picker (`h:mm:ss tt` with an up-down): click a field, then the arrows, the
/// wheel, Up/Down or digits change it; Left/Right move between fields.
fn time_picker(
    ui: &mut Ui,
    rc: Rect,
    value: &mut (u8, u8, u8),
    enabled: bool,
    id: &'static str,
    focus: &mut Focus,
) {
    let p = ui.painter().clone();
    let (h, m, s) = *value;
    // Options → Regional: a 24-hour clock has no AM/PM field.
    let clock24 = crate::regional::current().clock24;
    let last = if clock24 { 2 } else { 3 };
    let ampm = if h < 12 {
        // i18n: before noon, after a time of day (12-hour clock). Its pair is "PM": write both the same way (this language's own short marks, or both kept as AM and PM)
        crate::i18n::t("AM")
    } else {
        // i18n: after noon, after a time of day (12-hour clock). Its pair is "AM": write both the same way (this language's own short marks, or both kept as AM and PM)
        crate::i18n::t("PM")
    };
    // IDM's box holds "12:00:00 PM"; an AM/PM longer than the English ("مساء") widens it by the
    // difference at its trailing edge, into the room after it.
    let rtl = winui::rtl();
    let w2 = winui::text_width(&p, "00");
    let colon = winui::text_width(&p, ":");
    let space = winui::text_width(&p, " ");
    let english = winui::text_width(&p, "AM").max(winui::text_width(&p, "PM"));
    let extra = if clock24 {
        0.0
    } else {
        (winui::text_width(&p, ampm) - english).max(0.0).ceil()
    };
    let rc = if rtl {
        Rect::from_min_max(rc.min - vec2(extra, 0.0), rc.max)
    } else {
        Rect::from_min_max(rc.min, rc.max + vec2(extra, 0.0))
    };
    let hover = ui.interact(
        rc,
        egui::Id::unique(("time-picker", id)),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    picker_focus(ui, &hover, id, focus);
    // A spin box's look (the number boxes' too): a text box, its arrows inside its end; focused,
    // the accent along its bottom. A time reads left to right in every language: its fields are
    // laid out on screen as they are; in RTL the arrows are at the left and the fields follow.
    winui::edit_frame(&p, rc, enabled);
    if hover.has_focus() {
        winui::focus_bottom(&p, rc);
    }
    let spin = winui::spin_zone(rc);
    let fields_left = if rtl { rc.left() + 17.0 } else { rc.left() };
    let _ltr = winui::Surface::enter_if(false, 0.0, 0.0);
    let sel = match focus.at {
        Some((fid, f)) if fid == id && enabled => Some(f),
        _ => None,
    };
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    let base = winui::baseline_in(rc.top(), rc.height());
    let hs = if clock24 {
        format!("{h:02}")
    } else {
        h12.to_string()
    };
    // The hour is right-aligned in a two-digit field.
    let mut x = fields_left + 4.0 + w2 - winui::text_width(&p, &hs);
    let mut rects = Vec::new();
    rects.push(field(&p, x, base, &hs, sel == Some(0), enabled));
    x = fields_left + 4.0 + w2;
    text(
        &p,
        x,
        base,
        ":",
        if enabled {
            LIGHT_TEXT()
        } else {
            LIGHT_DISABLED()
        },
    );
    x += colon;
    rects.push(field(
        &p,
        x,
        base,
        &format!("{m:02}"),
        sel == Some(1),
        enabled,
    ));
    x += w2;
    text(
        &p,
        x,
        base,
        ":",
        if enabled {
            LIGHT_TEXT()
        } else {
            LIGHT_DISABLED()
        },
    );
    x += colon;
    rects.push(field(
        &p,
        x,
        base,
        &format!("{s:02}"),
        sel == Some(2),
        enabled,
    ));
    x += w2 + space;
    if !clock24 {
        rects.push(field(&p, x, base, ampm, sel == Some(3), enabled));
    }
    let step_btn = winui::spin_arrows(ui, spin, enabled, id);
    if !enabled {
        return;
    }
    for (i, fr) in rects.iter().enumerate() {
        if ui
            .interact(
                *fr,
                egui::Id::unique(("time-field", id, i)),
                winui::pointer_only(),
            )
            .clicked()
        {
            focus.at = Some((id, i));
            focus.typed.clear();
            hover.request_focus();
        }
    }
    if ui.input(|i| i.pointer.any_pressed()) && !ui.rect_contains_pointer(rc) && sel.is_some() {
        focus.at = None;
    }
    let f = sel.unwrap_or(0);
    let (mut step, typed, mv) = if sel.is_some() {
        picker_keys(ui, hover.hovered())
    } else {
        (0, None, 0)
    };
    step += step_btn;
    if step_btn != 0 {
        if sel.is_none() {
            focus.at = Some((id, 0));
        }
        hover.request_focus();
    }
    if step != 0 {
        let (h, m, s) = value;
        match f {
            0 => *h = ((*h as i32 + step).rem_euclid(24)) as u8,
            1 => *m = ((*m as i32 + step).rem_euclid(60)) as u8,
            2 => *s = ((*s as i32 + step).rem_euclid(60)) as u8,
            _ => *h = (*h + 12) % 24,
        }
    }
    if mv != 0 && sel.is_some() {
        focus.at = Some((id, (f as i32 + mv).clamp(0, last as i32) as usize));
        focus.typed.clear();
    }
    if let (Some(c), Some(f)) = (typed, sel) {
        let (h, m, s) = value;
        if f == 3 && !clock24 {
            match c.to_ascii_lowercase() {
                'a' if *h >= 12 => *h -= 12,
                'p' if *h < 12 => *h += 12,
                _ => {}
            }
        } else if let Some(d) = c.to_digit(10) {
            focus.typed.push(c);
            let n: u32 = focus.typed.parse().unwrap_or(d);
            match f {
                0 if clock24 => *h = n.min(23) as u8,
                0 => {
                    let pm = *h >= 12;
                    let n = n.clamp(1, 12) as u8 % 12;
                    *h = if pm { n + 12 } else { n };
                }
                1 => *m = n.min(59) as u8,
                _ => *s = n.min(59) as u8,
            }
            if focus.typed.len() >= 2 {
                focus.typed.clear();
                focus.at = Some((id, (f + 1).min(last)));
            }
        }
    }
}

// Names in English, translated where drawn. The fields keep IDM's en-US long-date layout
// (`Sunday, October 29, 2026`); the region's date order is M16's number-format step.
const WEEKDAYS: [&str; 7] = [
    crate::i18n::mark("Sunday"),
    crate::i18n::mark("Monday"),
    crate::i18n::mark("Tuesday"),
    crate::i18n::mark("Wednesday"),
    crate::i18n::mark("Thursday"),
    crate::i18n::mark("Friday"),
    crate::i18n::mark("Saturday"),
];
const MONTHS: [&str; 12] = [
    crate::i18n::mark("January"),
    crate::i18n::mark("February"),
    crate::i18n::mark("March"),
    crate::i18n::mark("April"),
    // i18n: the month (the fifth of the year)
    crate::i18n::mark("May"),
    crate::i18n::mark("June"),
    crate::i18n::mark("July"),
    crate::i18n::mark("August"),
    crate::i18n::mark("September"),
    crate::i18n::mark("October"),
    crate::i18n::mark("November"),
    crate::i18n::mark("December"),
];
/// The calendar's column heads, Sunday first.
const SHORT_DAYS: [&str; 7] = [
    // i18n: Sunday, as short as a calendar's column head (two letters in English)
    crate::i18n::mark("Su"),
    // i18n: Monday, as short as a calendar's column head (two letters in English)
    crate::i18n::mark("Mo"),
    // i18n: Tuesday, as short as a calendar's column head (two letters in English)
    crate::i18n::mark("Tu"),
    // i18n: Wednesday, as short as a calendar's column head (two letters in English)
    crate::i18n::mark("We"),
    // i18n: Thursday, as short as a calendar's column head (two letters in English)
    crate::i18n::mark("Th"),
    // i18n: Friday, as short as a calendar's column head (two letters in English)
    crate::i18n::mark("Fr"),
    // i18n: Saturday, as short as a calendar's column head (two letters in English)
    crate::i18n::mark("Sa"),
];

/// `date` moved by `n` months, the day clamped to the month's length.
fn add_months(date: NaiveDate, n: i32) -> NaiveDate {
    let total = date.year() * 12 + date.month0() as i32 + n;
    let (y, m) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    let mut d = date.day();
    loop {
        if let Some(x) = NaiveDate::from_ymd_opt(y, m, d) {
            return x;
        }
        d -= 1;
    }
}

/// IDM's long-date picker (`Wednesday,  October  28, 2026`, fixed-width fields, like
/// Windows'): fields change with the wheel, Up/Down and digits; the chevron opens a month
/// calendar.
fn date_picker(
    ui: &mut Ui,
    mut rc: Rect,
    date: &mut NaiveDate,
    enabled: bool,
    id: &'static str,
    focus: &mut Focus,
) {
    let p = ui.painter().clone();
    // The four fields (0 weekday, 1 month, 2 day, 3 year) in the order Options → Regional
    // writes a date: `Thursday, October 30, 2026` (IDM's), `Thursday, 30 October 2026` or
    // `2026 October 30, Thursday`. `visual`: the fields from the leading edge.
    let order = crate::regional::current().order;
    let visual: [usize; 4] = match order {
        DateOrder::Mdy => [0, 1, 2, 3],
        DateOrder::Dmy => [0, 2, 1, 3],
        DateOrder::Ymd => [3, 1, 2, 0],
    };
    let wd_comma = if order == DateOrder::Ymd { "" } else { "," };
    let day_comma = if order == DateOrder::Dmy { "" } else { "," };
    // Where the fields start, from the box's leading edge: one after the other as the date reads
    // (Windows' date picker flows its text too), from 5 px in as every text box's text, the box
    // growing at its trailing edge when they need more room. IDM's fixed places let "September"
    // (53 px) run into the day; fields sized for the widest names left gaps after short ones.
    let mut xs: [f32; 4] = [5.0, 71.0, 121.0, 140.0];
    {
        let name = |n: &str, tail: &str| {
            winui::text_width(&p, &format!("{}{tail}", crate::i18n::t(n))).ceil()
        };
        let widths = [
            name(
                WEEKDAYS[date.weekday().num_days_from_sunday() as usize],
                wd_comma,
            ),
            name(MONTHS[date.month0() as usize], ""),
            winui::text_width(&p, &format!("00{day_comma}")).ceil(),
            winui::text_width(&p, "0000").ceil(),
        ];
        // The gap after each field (IDM's: 5 after the names, 4 after the numbers).
        let gaps = [5.0, 5.0, 4.0, 4.0];
        let mut x = 5.0;
        for &f in &visual {
            xs[f] = x;
            x = xs[f] + widths[f] + gaps[f];
        }
        let need = x + 20.0;
        let mut english = winui::rect(rc);
        english.max.x = english.max.x.max(english.min.x + need);
        rc = winui::rect(english);
    }
    let key = egui::Id::unique(("date-picker", id));
    let hover = ui.interact(
        rc,
        key,
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    picker_focus(ui, &hover, id, focus);
    let mut open = ui.data(|d| d.get_temp::<bool>(key).unwrap_or(false)) && enabled;
    winui::dropdown_frame_light(&p, rc, winui::State::of(&hover, enabled).open_if(open));
    let sel = match focus.at {
        Some((fid, f)) if fid == id && enabled => Some(f),
        _ => None,
    };
    let base = winui::baseline_in(rc.top(), rc.height());
    let wd = format!(
        "{}{wd_comma}",
        crate::i18n::t(WEEKDAYS[date.weekday().num_days_from_sunday() as usize])
    );
    // A date in words: its fields from the leading edge (the weekday at the right in RTL), the
    // chevron at the trailing edge.
    let screen = rc;
    let rc = winui::rect(rc);
    let rects = [
        field(&p, rc.left() + xs[0], base, &wd, sel == Some(0), enabled),
        field(
            &p,
            rc.left() + xs[1],
            base,
            crate::i18n::t(MONTHS[date.month0() as usize]),
            sel == Some(1),
            enabled,
        ),
        field(
            &p,
            rc.left() + xs[2],
            base,
            &format!("{}{day_comma}", date.day()),
            sel == Some(2),
            enabled,
        ),
        field(
            &p,
            rc.left() + xs[3],
            base,
            &date.year().to_string(),
            sel == Some(3),
            enabled,
        ),
    ];
    let chevron = winui::at(pos2(rc.right() - 13.0, rc.top() + 10.0), 8.0);
    winui::chevron_light(&p, chevron, enabled);
    let rc = screen;
    if !enabled {
        return;
    }
    let drop = winui::rect(Rect::from_min_max(
        pos2(winui::rect(rc).right() - 20.0, rc.top()),
        winui::rect(rc).max,
    ));
    let drop_resp = ui.interact(drop, key.with("drop"), winui::pointer_only());
    winui::tint_part(
        &p,
        drop.shrink(1.0),
        winui::subtle_fill(
            winui::State::of(&drop_resp, true).open_if(open),
            crate::theme::colors(),
        ),
        |p| winui::chevron_light(p, chevron, true),
    );
    if drop_resp.clicked() {
        open = !open;
    }
    for (i, fr) in rects.iter().enumerate() {
        if ui
            .interact(*fr, key.with(("field", i)), winui::pointer_only())
            .clicked()
        {
            focus.at = Some((id, i));
            focus.typed.clear();
            hover.request_focus();
        }
    }
    if ui.input(|i| i.pointer.any_pressed()) && !ui.rect_contains_pointer(rc) && sel.is_some() {
        focus.at = None;
    }
    // The fields' keys wait while the calendar is open (it has its own).
    if let Some(f) = sel.filter(|_| !open) {
        let (step, typed, mv) = picker_keys(ui, hover.hovered());
        if step != 0 {
            *date = match f {
                0 | 2 => *date + Days::days(step as i64),
                1 => add_months(*date, step),
                _ => add_months(*date, 12 * step),
            };
        }
        if mv != 0 {
            // Left and right go through the fields in reading order.
            let at = visual.iter().position(|&v| v == f).unwrap_or(0) as i32;
            focus.at = Some((id, visual[(at + mv).clamp(0, 3) as usize]));
            focus.typed.clear();
        }
        if let Some(d) = typed.and_then(|c| c.to_digit(10)) {
            focus.typed.push(char::from_digit(d, 10).unwrap_or('0'));
            let n: i32 = focus.typed.parse().unwrap_or(0);
            let candidate = match f {
                1 => NaiveDate::from_ymd_opt(date.year(), n.clamp(1, 12) as u32, 1)
                    .map(|first| add_months(first, 0).with_day(date.day()).unwrap_or(first)),
                2 => date.with_day(n.max(1) as u32),
                3 if focus.typed.len() == 4 => date.with_year(n),
                _ => None,
            };
            if let Some(c) = candidate {
                *date = c;
            }
            let full = if f == 3 { 4 } else { 2 };
            if focus.typed.len() >= full {
                focus.typed.clear();
            }
        }
    }
    if open {
        let (picked, over) = calendar(ui, key, rc, *date);
        if let Some(picked) = picked {
            *date = picked;
            open = false;
        }
        // The calendar answers Escape itself (the Scheduler under it doesn't close), and a
        // click anywhere else closes it.
        let outside =
            ui.input(|i| i.pointer.any_pressed()) && !over && !ui.rect_contains_pointer(rc);
        if winui::take_key(ui, egui::Key::Escape) || outside {
            open = false;
        }
    }
    if !open {
        // Opened again, it shows the chosen date's month.
        ui.data_mut(|d| {
            d.remove::<NaiveDate>(key.with("month"));
            d.remove::<NaiveDate>(key.with("cursor"));
        });
    }
    ui.data_mut(|d| d.insert_temp(key, open));
}

/// The month calendar under a date picker, as Windows' has it: the arrows move a day cursor
/// (Left / Right a day, Up / Down a week), Page Up / Down a month, Enter picks it, and Today
/// at the bottom. Returns the day picked, and whether the pointer is over the calendar.
fn calendar(ui: &mut Ui, key: egui::Id, under: Rect, date: NaiveDate) -> (Option<NaiveDate>, bool) {
    let month_key = key.with("month");
    let cursor_key = key.with("cursor");
    let mut cursor: NaiveDate = ui.data(|d| d.get_temp(cursor_key)).unwrap_or(date);
    let mut month: NaiveDate = ui
        .data(|d| d.get_temp(month_key))
        .unwrap_or_else(|| date.with_day(1).unwrap_or(date));
    let (cw, ch) = (28.0, 19.0);
    let size = vec2(cw * 7.0 + 12.0, 26.0 + 18.0 + ch * 6.0 + 8.0 + 18.0);
    let mut picked = None;
    let today = crate::clock::now().date_naive();
    // The keys (the week runs from the right in RTL: Left and Right swap there).
    let back = if winui::rtl() { 1 } else { -1 };
    let moves = [
        (egui::Key::ArrowLeft, Days::days(1), back),
        (egui::Key::ArrowRight, Days::days(1), -back),
        (egui::Key::ArrowUp, Days::days(7), -1),
        (egui::Key::ArrowDown, Days::days(7), 1),
    ];
    let before = cursor;
    for (k, by, dir) in moves {
        if winui::take_key(ui, k) {
            cursor = if dir > 0 { cursor + by } else { cursor - by };
        }
    }
    if winui::take_key(ui, egui::Key::PageUp) {
        cursor = add_months(cursor, -1);
    }
    if winui::take_key(ui, egui::Key::PageDown) {
        cursor = add_months(cursor, 1);
    }
    if winui::take_key(ui, egui::Key::Enter) {
        picked = Some(cursor);
    }
    // A moved cursor brings its month.
    if cursor != before {
        month = cursor.with_day(1).unwrap_or(month);
    }
    // Under the picker's leading edge; the month laid out in English (the week from the right
    // in RTL), each part made where it is on screen.
    let at = winui::at(
        pos2(winui::rect(under).left(), under.bottom() + 1.0),
        size.x,
    );
    let area = egui::Area::new(key.with("calendar"))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ui.ctx(), |ui| {
            let (rect, p) = ui.allocate_painter(size, Sense::hover());
            let shown = rect.rect;
            crate::chrome::popup_surface(&p, shown);
            let rect = winui::rect(shown);
            let title = crate::i18n::fmt(
                // i18n: the calendar's title: {month} = a month's name, {year} = its year
                crate::i18n::t("{month} {year}"),
                &[
                    ("month", crate::i18n::t(MONTHS[month.month0() as usize])),
                    ("year", &crate::i18n::ltr(&month.year().to_string())),
                ],
            );
            let tw = winui::text_width(&p, &title);
            text(
                &p,
                (rect.center().x - tw / 2.0).round(),
                rect.top() + 17.0,
                &title,
                LIGHT_TEXT(),
            );
            for (dir, x, glyph) in [(-1, rect.left() + 8.0, "<"), (1, rect.right() - 22.0, ">")] {
                let b = Rect::from_min_size(pos2(x, rect.top() + 5.0), vec2(14.0, 16.0));
                let r = ui.interact(
                    winui::rect(b),
                    key.with(("cal-nav", dir)),
                    winui::pointer_only(),
                );
                if let Some(c) =
                    winui::highlight_fill(winui::State::of(&r, true), crate::theme::colors())
                {
                    p.rect_filled(winui::rect(b), crate::theme::metrics().part_radius, c);
                }
                text(&p, b.left() + 4.0, b.top() + 12.0, glyph, LIGHT_TEXT());
                if r.clicked() {
                    month = add_months(month, dir);
                }
            }
            let x0 = rect.left() + 6.0;
            let y0 = rect.top() + 26.0;
            // The week from its first day (Options → Regional).
            let start = crate::regional::current().week_start as usize;
            for i in 0..7 {
                let d = crate::i18n::t(SHORT_DAYS[(start + i) % 7]);
                let w = winui::text_width(&p, d);
                text(
                    &p,
                    x0 + cw * i as f32 + (cw - w) / 2.0,
                    y0 + 12.0,
                    d,
                    LIGHT_DISABLED(),
                );
            }
            let lead = (month.weekday().num_days_from_sunday() as i64 + 7 - start as i64) % 7;
            let first = month - Days::days(lead);
            for k in 0..42i64 {
                let day = first + Days::days(k);
                let (col, row) = ((k % 7) as f32, (k / 7) as f32);
                let cell =
                    Rect::from_min_size(pos2(x0 + cw * col, y0 + 18.0 + ch * row), vec2(cw, ch));
                let r = ui.interact(
                    winui::rect(cell),
                    key.with(("cal-day", k)),
                    winui::pointer_only(),
                );
                let label = day.day().to_string();
                let w = winui::text_width(&p, &label);
                let chosen = day == date;
                let mark = winui::rect(cell.shrink(1.0));
                if chosen {
                    p.rect_filled(mark, winui::control_radius(), winui::SELECTION());
                } else if let Some(c) =
                    winui::highlight_fill(winui::State::of(&r, true), crate::theme::colors())
                {
                    p.rect_filled(mark, winui::control_radius(), c);
                }
                if day == today && !chosen {
                    p.rect_stroke(
                        mark,
                        3.0,
                        egui::Stroke::new(1.0, winui::SELECTION()),
                        egui::StrokeKind::Inside,
                    );
                }
                if day == cursor && cursor != date {
                    winui::focus_ring(&p, mark);
                }
                let c = if chosen {
                    winui::SELECTION_TEXT()
                } else if day.month() != month.month() {
                    winui::TEXT_DISABLED()
                } else {
                    LIGHT_TEXT()
                };
                text(
                    &p,
                    cell.left() + (cw - w) / 2.0,
                    cell.top() + 13.0,
                    &label,
                    c,
                );
                if r.clicked() {
                    picked = Some(day);
                }
            }
            // Today, under the month: a click picks it.
            let label = crate::i18n::fmt(
                // i18n: link under the calendar: {date} = today's date
                crate::i18n::t("Today: {date}"),
                &[(
                    "date",
                    &crate::i18n::ltr(&crate::regional::short_date(today)),
                )],
            );
            let ty = y0 + 18.0 + ch * 6.0 + 16.0;
            let tw = winui::text_width(&p, &label);
            if winui::link(
                ui,
                &p,
                (rect.center().x - tw / 2.0).round(),
                ty,
                &label,
                "cal-today",
            ) {
                picked = Some(today);
            }
        });
    let over = area.response.contains_pointer();
    if picked.is_some() {
        ui.data_mut(|d| d.remove::<NaiveDate>(month_key));
    } else {
        ui.data_mut(|d| {
            d.insert_temp(month_key, month);
            d.insert_temp(cursor_key, cursor);
        });
    }
    (picked, over)
}

/// A queue's downloads after a three-way merge: the draft's order, less what left the list
/// since `base`, plus what was added to the queue since (at the end).
fn merge_items(base: &[u64], draft: &[u64], live: &[u64]) -> Vec<u64> {
    let mut out: Vec<u64> = draft
        .iter()
        .copied()
        .filter(|i| live.contains(i) || !base.contains(i))
        .collect();
    for i in live {
        if !base.contains(i) && !out.contains(i) {
            out.push(*i);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_draft_keeps_its_order_and_the_lists_changes() {
        // Opened with 1 2 3; the Scheduler moved 3 up and removed 1; meanwhile the main window
        // deleted 2 and added 4.
        assert_eq!(merge_items(&[1, 2, 3], &[3, 2], &[1, 3, 4]), vec![3, 4]);
        // Nothing changed elsewhere: the draft as it is.
        assert_eq!(merge_items(&[1, 2], &[2, 1], &[1, 2]), vec![2, 1]);
    }

    #[test]
    fn a_form_round_trips_a_queue() {
        let mut q = Queue::new("Q", false);
        q.schedule.start_at = Some((6, 15, 0));
        q.schedule.once = Some((2026, 11, 2));
        q.schedule.turn_off = Some(TurnOff::Hibernate);
        q.schedule.retries = Some(3);
        q.files_at_once = 2;
        let mut back = Queue::new("Q", false);
        Form::of(&q).apply_to(&mut back);
        assert_eq!(back, q);
    }

    #[test]
    fn unchecked_options_are_off_but_keep_their_values_in_the_form() {
        let q = Queue::new("Q", false);
        let mut f = Form::of(&q);
        assert_eq!(f.start_at, (23, 0, 0), "IDM's default time");
        assert_eq!(f.stop_at, (7, 30, 0));
        f.start_at = (5, 0, 0);
        let mut out = q.clone();
        f.apply_to(&mut out);
        assert_eq!(out.schedule.start_at, None);
        assert_eq!(out.files_at_once, 4);
    }

    #[test]
    fn months_clamp_their_last_day() {
        let jan31 = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        assert_eq!(
            add_months(jan31, 1),
            NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()
        );
        assert_eq!(
            add_months(jan31, -2),
            NaiveDate::from_ymd_opt(2025, 11, 30).unwrap()
        );
    }
}
