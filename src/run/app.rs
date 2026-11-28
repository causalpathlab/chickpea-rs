//! The run screen's state and keys: a step's count files (the data pane)
//! and its other flags as a form, then its run.

use super::data::{self, Data, SIDES};
use super::pick::{Outcome, Pick, Want};
use super::worker::{Ended, Job};
use super::{checked, checked_given, Step, STEPS};
use crate::p2g::samples::Holds;
use crate::tui::{wheel, ListHit, TOOL};
use legume_mcp::job::{End, Reporter};
use legume_tui::browse::shown;
use legume_tui::form::{Form, Kind, Many};
use legume_tui::keys::{is_click_or_wheel, is_stray, list_move, wheel_key};
use legume_tui::{is_run_key, Round};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, MouseEvent};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

/// How long a round waits for a key.
const TICK: Duration = Duration::from_millis(100);

/// What the screen shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// The step's count files, a row per sample.
    Data,
    /// The step's other flags.
    Form,
    /// The run and its log.
    Run,
}

/// Where the last draw put what a click can land on.
#[derive(Clone, Debug, Default)]
pub struct Drawn {
    /// The tabs drawn in the head line, and the columns each spans.
    pub tabs: Vec<(Pane, std::ops::Range<u16>)>,
    /// The pane's list.
    pub list: ListHit,
    /// The data pane's columns: where each side's starts.
    pub cols: Vec<u16>,
}

/// The screen.
pub struct App {
    pub step: &'static Step,
    /// The data pane, for a step with count files.
    pub data: Option<Data>,
    /// The files being told apart off the screen's thread (those whose
    /// names do not say what they hold are opened), for the data pane.
    reading: Option<Receiver<Vec<(PathBuf, Holds)>>>,
    /// The step's flags as given after `--`, which run as they are: the
    /// form then only shows them.
    pub given: Option<Vec<String>>,
    pub form: Form,
    /// The row under the cursor.
    pub at: usize,
    /// The text being typed into the row under the cursor.
    pub editing: Option<String>,
    /// The file browser, when open.
    pub pick: Option<Pick>,
    /// Where the browser opens next: where files were last taken from.
    pub browse_dir: PathBuf,
    pub pane: Pane,
    pub job: Option<Job>,
    /// What the form's line would write, or why it cannot run.
    pub check: Result<Vec<PathBuf>, String>,
    /// A line for the user: why a run did not start, say.
    pub note: Option<String>,
    /// Log lines scrolled back from the newest.
    pub scroll: usize,
    pub quit: bool,
    pub drawn: RefCell<Drawn>,
}

impl App {
    /// The screen for `step`, its form filled from `argv` (its flags).
    pub fn new(step: &'static Step, argv: &[String]) -> Self {
        let cmd = (step.command)();
        let mut form = Form::new(&cmd, |a| !a.is_hide_set());
        let first = |long: &str| {
            step.inputs
                .iter()
                .position(|l| *l == long)
                .unwrap_or(step.inputs.len())
        };
        form.fields.sort_by_key(|f| first(&f.long));
        let line: Vec<String> = std::iter::once(step.name.to_string())
            .chain(argv.iter().cloned())
            .collect();
        let m = legume_tui::form::parse(&cmd, &line).ok();
        if let Some(m) = &m {
            form.prefill(m);
        }
        // The count files are the data pane's, not rows of the form.
        let data = step.data.then(|| {
            let mut data = Data::default();
            data.prefill(|side| {
                m.as_ref()
                    .and_then(|m| legume_tui::form::given_words(m, side.long()))
                    .unwrap_or_default()
            });
            form.fields
                .retain(|f| !SIDES.iter().any(|s| s.long() == f.long));
            data
        });
        let pane = if data.as_ref().is_some_and(|d| d.pairs.is_empty()) {
            Pane::Data
        } else {
            Pane::Form
        };
        let mut app = Self {
            step,
            data,
            reading: None,
            given: None,
            form,
            at: 0,
            editing: None,
            pick: None,
            browse_dir: std::env::current_dir().unwrap_or_default(),
            pane,
            job: None,
            check: Err(String::new()),
            note: None,
            scroll: 0,
            quit: false,
            drawn: RefCell::new(Drawn::default()),
        };
        app.recheck();
        app
    }

    /// The screen for `step` given whole after `--` with `argv` (its
    /// flags): that line runs unchanged, checked as chickpea parses it and
    /// its input files there, and the form shows it.
    pub fn given(step: &'static Step, argv: &[String]) -> Self {
        let mut app = Self::new(step, argv);
        app.given = Some(argv.to_vec());
        app.recheck();
        app
    }

    /// The step's flags the screen stands for: the count files, then the
    /// form's.
    #[must_use]
    pub fn argv(&self) -> Vec<String> {
        self.data
            .iter()
            .flat_map(Data::argv)
            .chain(self.form.argv())
            .collect()
    }

    /// The line a run starts, as typed after `chickpea`: the step's name,
    /// then its flags as given after `--`, else as the screen has them.
    #[must_use]
    pub fn command(&self) -> Vec<String> {
        std::iter::once(self.step.name.to_string())
            .chain(self.given.clone().unwrap_or_else(|| self.argv()))
            .collect()
    }

    /// Whether the form's `--data` is set: the count files given as a flag.
    fn data_flag(&self) -> bool {
        self.form
            .fields
            .iter()
            .any(|f| &*f.long == "data" && !f.value.trim().is_empty())
    }

    fn recheck(&mut self) {
        let by_flag = self.data_flag();
        self.check = if let Some(argv) = &self.given {
            checked_given(self.step, argv)
        } else if self.reading.is_some() {
            Err("reading the files chosen…".into())
        } else {
            match self.data.as_ref().and_then(|d| d.problem(by_flag)) {
                Some(why) => Err(why),
                None => checked(self.step, &self.argv()),
            }
        };
    }

    /// What goes on between keys: the browser's listing, and the files
    /// being told apart, folded into the data pane once they are.
    pub fn tick(&mut self) {
        if let Some(pick) = self.pick.as_mut() {
            pick.tick();
        }
        let Some(rx) = &self.reading else { return };
        let told = match rx.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(told) => Some(told),
            Err(TryRecvError::Disconnected) => None,
        };
        self.reading = None;
        self.note = match (told, self.data.as_mut()) {
            (Some(told), Some(d)) => Some(d.add_told(told)),
            _ => Some("the files chosen could not be told apart".into()),
        };
        self.recheck();
    }

    /// Add `files` to the data pane: at once when their names say what
    /// they hold, else read on a thread of their own, so the screen stays
    /// live, and folded in by [`App::tick`].
    fn add_files(&mut self, files: Vec<PathBuf>) {
        let Some(d) = self.data.as_mut() else { return };
        if self.reading.is_some() {
            self.note = Some("still reading the files chosen before".into());
            return;
        }
        if data::told_by_name(&files) {
            self.note = Some(d.add(files));
            return;
        }
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("chickpea-read".into())
            .spawn(move || {
                let _ = tx.send(data::told(files));
            });
        self.note = Some(match spawned {
            Ok(_) => {
                self.reading = Some(rx);
                "reading…".into()
            }
            Err(e) => format!("cannot read the files: {e}"),
        });
    }

    /// Start the run the form stands for, unless the line is refused or it
    /// would write over a run.
    pub fn start(&mut self, env: legume_tui::ChildEnv) {
        self.recheck();
        self.note = match &self.check {
            Err(why) => Some(why.clone()),
            Ok(made) => match crate::mcp::not_over(made, "change --out") {
                Err(e) => Some(format!("{e:#}")),
                Ok(()) => match Job::start(self.command(), env) {
                    Ok(job) => {
                        self.job = Some(job);
                        self.pane = Pane::Run;
                        self.scroll = 0;
                        None
                    }
                    Err(e) => Some(format!("cannot start: {e:#}")),
                },
            },
        };
    }

    /// Whether a run is going on.
    #[must_use]
    pub fn running(&self) -> bool {
        self.job.as_ref().is_some_and(Job::running)
    }

    /// The panes Tab goes through.
    pub(super) fn panes(&self) -> Vec<Pane> {
        let mut v = Vec::with_capacity(3);
        if self.data.is_some() {
            v.push(Pane::Data);
        }
        v.push(Pane::Form);
        if self.job.is_some() {
            v.push(Pane::Run);
        }
        v
    }

    fn next_pane(&mut self, d: isize) {
        let panes = self.panes();
        let at = panes.iter().position(|p| *p == self.pane).unwrap_or(0) as isize;
        self.pane = panes[(at + d).rem_euclid(panes.len() as isize) as usize];
    }

    /// Open the browser for `want`, at `at` when a file is set.
    fn browse(&mut self, want: Want, at: Option<PathBuf>) {
        self.pick = Some(Pick::new(&self.browse_dir, want, at.as_deref()));
    }

    /// What the browser chose goes where it was opened for.
    fn chosen(&mut self, want: Want, files: Vec<PathBuf>) {
        if let Some(dir) = files.first().and_then(|f| f.parent()) {
            self.browse_dir = dir.to_path_buf();
        }
        match want {
            Want::Data => self.add_files(files),
            Want::Cell => {
                if let (Some(d), Some(f)) = (self.data.as_mut(), files.into_iter().next()) {
                    d.set(f);
                }
            }
            Want::Flag { long, .. } => {
                if let Some(field) = self.form.get_mut(&long).filter(|_| !files.is_empty()) {
                    let names: Vec<String> = files.iter().map(|f| shown(f)).collect();
                    field.set_text(&names.join(" "));
                }
            }
        }
        self.recheck();
    }

    /// Take a key.
    pub fn key(&mut self, k: KeyEvent, env: &legume_tui::ChildEnv) {
        if let Some(pick) = self.pick.as_mut() {
            let out = pick.key(k);
            self.picked(out);
            return;
        }
        if let Some(text) = self.editing.as_mut() {
            match k.code {
                KeyCode::Enter => {
                    let text = std::mem::take(text);
                    self.form.fields[self.at].set_text(&text);
                    self.editing = None;
                    self.recheck();
                }
                KeyCode::Esc => self.editing = None,
                KeyCode::Backspace => {
                    text.pop();
                }
                // Typed text: a character as it is, shifted, or by AltGr
                // (sent as Ctrl+Alt); not one held with Ctrl or Alt alone.
                KeyCode::Char(c) if crate::tui::typed(k.modifiers) => {
                    text.push(c);
                }
                _ => {}
            }
            return;
        }
        if is_run_key(&k) {
            if !self.running() {
                self.start(env.clone());
            }
            return;
        }
        // Ctrl+x is not x (which stops a run), nor Alt+q q.
        if is_stray(&k) {
            return;
        }
        match k.code {
            KeyCode::Tab => return self.next_pane(1),
            KeyCode::BackTab => return self.next_pane(-1),
            _ => {}
        }
        // A line given after `--` runs as given: its panes show it, and
        // only move, so no edit there is silently left out of the run.
        let edits = !matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
            && legume_tui::keys::list_delta(&k, true).is_none();
        if self.given.is_some() && self.pane != Pane::Run && edits {
            self.note =
                Some("this line was given after `--` and runs as given; change it there".into());
            return;
        }
        match self.pane {
            Pane::Data => self.data_key(k),
            Pane::Form => self.form_key(k),
            Pane::Run => self.run_key(k),
        }
    }

    fn picked(&mut self, out: Outcome) {
        match out {
            Outcome::Open => {}
            Outcome::Cancelled => self.pick = None,
            Outcome::Chosen(files) => {
                if let Some(p) = self.pick.take() {
                    self.chosen(p.want, files);
                }
            }
        }
    }

    fn quit_key(&mut self) {
        if self.running() {
            self.note = Some("a run is going on: x on the run pane stops it, then q quits".into());
        } else {
            self.quit = true;
        }
    }

    fn data_key(&mut self, k: KeyEvent) {
        if matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) {
            return self.quit_key();
        }
        let Some(d) = self.data.as_mut() else { return };
        let n = d.pairs.len();
        // The rows, and the "add" row after them.
        if let Some(at) = list_move(&k, d.at, n + 1, true) {
            d.at = at;
            return;
        }
        match k.code {
            KeyCode::Left | KeyCode::Char('h') => d.col = d.col.saturating_sub(1),
            KeyCode::Right | KeyCode::Char('l') => d.col = (d.col + 1).min(SIDES.len() - 1),
            KeyCode::Char('a') | KeyCode::Char('+') => self.browse(Want::Data, None),
            KeyCode::Enter | KeyCode::Char(' ') if d.at >= n => self.browse(Want::Data, None),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let at = d.pairs[d.at].get(d.side()).cloned();
                self.browse(Want::Cell, at);
            }
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
                d.remove();
                d.at = d.at.min(d.pairs.len());
                self.recheck();
            }
            _ => {}
        }
    }

    fn form_key(&mut self, k: KeyEvent) {
        if let Some(at) = list_move(&k, self.at, self.form.fields.len(), true) {
            self.at = at;
            return;
        }
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit_key(),
            KeyCode::Char('d') => {
                self.form.fields[self.at].reset();
                self.recheck();
            }
            KeyCode::Char('e') => self.editing = Some(self.form.fields[self.at].value.clone()),
            KeyCode::Enter | KeyCode::Char(' ') => self.change(),
            KeyCode::Left | KeyCode::Right => {
                let d = if k.code == KeyCode::Left { -1 } else { 1 };
                self.form.fields[self.at].cycle(d);
                self.recheck();
            }
            _ => {}
        }
    }

    /// Change the row under the cursor: flip it, step it, browse for its
    /// file, or type it.
    fn change(&mut self) {
        let f = &mut self.form.fields[self.at];
        match f.kind {
            Kind::Flag { .. } => f.toggle(),
            Kind::Choice(_) => f.cycle(1),
            Kind::Text if f.takes_path() => {
                let many = f.many != Many::One;
                let at = if many {
                    f.value.split([',', ' ']).find(|v| !v.is_empty())
                } else {
                    Some(f.value.trim()).filter(|v| !v.is_empty())
                };
                let at = at.map(PathBuf::from);
                let want = Want::Flag {
                    long: f.long.clone(),
                    many,
                };
                return self.browse(want, at);
            }
            Kind::Text => self.editing = Some(f.value.clone()),
        }
        self.recheck();
    }

    fn run_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('x') => {
                if let Some(job) = &self.job {
                    job.stop();
                }
            }
            KeyCode::Char('q') | KeyCode::Esc => self.quit_key(),
            KeyCode::Char('e') => self.pane = Pane::Form,
            KeyCode::Up | KeyCode::Char('k') => self.scroll += 1,
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageUp => self.scroll += 20,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(20),
            KeyCode::End => self.scroll = 0,
            _ => {}
        }
        // No further back than the oldest line kept.
        let n = self.job.as_ref().map_or(0, |j| j.log().lines.len());
        self.scroll = self.scroll.min(n.saturating_sub(1));
    }

    /// Take the mouse: the wheel scrolls what is under the cursor's pane,
    /// a click on a tab opens its pane, a click on a row puts the cursor
    /// there, and a click on the row it is on changes it, as enter does.
    pub fn mouse(&mut self, m: MouseEvent, env: &legume_tui::ChildEnv) {
        if !is_click_or_wheel(&m) {
            return;
        }
        if let Some(pick) = self.pick.as_mut() {
            let out = match wheel(&m) {
                Some(d) => {
                    pick.scroll(d * 3);
                    Outcome::Open
                }
                None => pick.click(m.column, m.row),
            };
            return self.picked(out);
        }
        if self.editing.is_some() {
            return;
        }
        if let Some(k) = wheel_key(&m) {
            // As the arrow keys, but the log three lines a turn.
            let turns = if self.pane == Pane::Run { 3 } else { 1 };
            for _ in 0..turns {
                self.key(k, env);
            }
            return;
        }
        let drawn = self.drawn.borrow().clone();
        if m.row == 0 {
            if let Some((pane, _)) = drawn.tabs.iter().find(|(_, r)| r.contains(&m.column)) {
                self.pane = *pane;
            }
            return;
        }
        let Some(i) = drawn.list.row_at(m.column, m.row) else {
            return;
        };
        match self.pane {
            Pane::Data => {
                let Some(d) = self.data.as_mut().filter(|d| i <= d.pairs.len()) else {
                    return;
                };
                let col = drawn.cols.iter().rposition(|&x| m.column >= x).unwrap_or(0);
                if i == d.at && (col == d.col || i == d.pairs.len()) {
                    self.data_key(KeyEvent::from(KeyCode::Enter));
                } else {
                    d.at = i;
                    d.col = col;
                }
            }
            Pane::Form if i < self.form.fields.len() => {
                if i == self.at {
                    self.change();
                } else {
                    self.at = i;
                }
            }
            _ => {}
        }
    }
}

/// Run the screen for `args`.
pub fn run(args: &super::RunArgs) -> anyhow::Result<()> {
    legume_tui::view::need_terminal(
        "run the step itself (chickpea peak-to-gene ...), or p2g_run on chickpea mcp",
    )?;
    let (step, argv): (&'static Step, &[String]) = match args.step.split_first() {
        Some((name, rest)) => (super::step(name)?, rest),
        None => (&STEPS[0], &[]),
    };
    // A step given whole, from `chickpea mcp` say: the screen ends with it,
    // and its log and end go to its job, also when it cannot start. A step
    // named alone opens its form.
    let given = args.step.len() > 1;
    let reporter = given.then(Reporter::from_env).flatten();
    let session = TOOL.session("run", &args.session())?;
    let env = session.child_env();
    let mut app = if given {
        App::given(step, argv)
    } else {
        App::new(step, argv)
    };
    if given {
        app.start(env.clone());
        if app.job.is_none() {
            let why = app.note.clone().unwrap_or_default();
            if let Some(r) = &reporter {
                r.finish(End::Failed(why.clone()));
            }
            anyhow::bail!("chickpea {}: {why}", legume_mcp::command_line(&args.step));
        }
    }
    let mut sent = 0usize;
    let end: anyhow::Result<_> = session.with_terminal(crate::tui::modes(), |terminal| {
        loop {
            // Lines and end taken under one lock, so no last line is left
            // unsent, and written after it.
            let mut new: Vec<String> = Vec::new();
            let ended = app.job.as_ref().and_then(|job| {
                let log = job.log();
                if reporter.is_some() {
                    new.extend(log.lines_from(sent).cloned());
                    sent = log.said();
                }
                log.end.clone()
            });
            if let Some(r) = &reporter {
                new.iter().for_each(|l| r.line(l));
            }
            // A step given for a job, or a `chickpea kill`: done once the
            // run is.
            if (reporter.is_some() || session.stop_requested())
                && !app.running()
                && (ended.is_some() || session.stop_requested())
            {
                return Ok(ended);
            }
            app.tick();
            session.status(&status(&app));
            session.draw(terminal, |f| super::draw::draw(f, &app))?;
            match session.poll(terminal, TICK)? {
                Round::Stopped => {
                    // `chickpea kill`: the run first, then the screen.
                    if let Some(job) = &app.job {
                        job.stop();
                    }
                    if !app.running() {
                        return Ok(ended);
                    }
                }
                Round::Event(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                    app.key(k, &env);
                }
                Round::Event(Event::Mouse(m)) => app.mouse(m, &env),
                _ => {}
            }
            if app.quit {
                return Ok(ended);
            }
        }
    });
    if let Some(r) = &reporter {
        r.finish(match &end {
            Ok(Some(Ok(()))) => {
                End::Done(format!("chickpea {}", legume_mcp::command_line(&args.step)))
            }
            Ok(Some(Err(Ended::Stopped))) => End::Cancelled,
            Ok(Some(Err(Ended::Failed(why)))) => End::Failed(why.clone()),
            Ok(None) => End::Cancelled,
            Err(e) => End::Failed(format!("{e:#}")),
        });
    }
    match end? {
        Some(Err(Ended::Failed(why))) if given => anyhow::bail!("{why}"),
        _ => Ok(()),
    }
}

/// The session's status line: what the screen is doing.
fn status(app: &App) -> String {
    let what = app.step.name;
    match &app.job {
        Some(job) => {
            let log = job.log();
            match (&log.end, &log.bar) {
                (None, Some(bar)) => format!("{what}: {}", bar.now.what),
                (None, None) => format!("{what}: running"),
                (Some(Ok(())), _) => format!("{what}: done"),
                (Some(Err(Ended::Stopped)), _) => format!("{what}: stopped"),
                (Some(Err(Ended::Failed(_))), _) => format!("{what}: failed"),
            }
        }
        None => format!("{what}: setting up"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use ratatui::Terminal;

    fn screen(app: &App) -> String {
        let mut t = Terminal::new(TestBackend::new(120, 30)).unwrap();
        t.draw(|f| super::super::draw::draw(f, app)).unwrap();
        let b = t.backend().buffer().clone();
        (0..b.area.height)
            .map(|y| {
                (0..b.area.width)
                    .map(|x| b[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn key(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::from(code), &legume_tui::ChildEnv::default());
    }

    #[test]
    fn files_are_added_from_the_browser_and_paired() {
        let dir = tempfile::tempdir().unwrap();
        for f in [
            "s1.rna.zarr.zip",
            "s1.atac.zarr.zip",
            "s2.rna.zarr.zip",
            "s2.atac.zarr.zip",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(f), "x").unwrap();
        }
        let mut app = App::new(&STEPS[0], &[]);
        app.browse_dir = dir.path().to_path_buf();
        assert_eq!(app.pane, Pane::Data);
        key(&mut app, KeyCode::Char('a'));
        let shown = screen(&app);
        assert!(shown.contains("s1.atac.zarr.zip"), "{shown}");
        assert!(!shown.contains("notes.txt"), "{shown}");
        // Mark all four with space; enter takes them.
        for _ in 0..4 {
            key(&mut app, KeyCode::Char(' '));
        }
        key(&mut app, KeyCode::Enter);
        assert!(app.pick.is_none());
        assert_eq!(app.data.as_ref().unwrap().pairs.len(), 2);
        let shown = screen(&app);
        println!("{shown}");
        assert!(shown.contains("s2"), "{shown}");
        let argv = app.argv().join(" ");
        assert!(argv.contains("--rna"), "{argv}");
        // A click on a tab opens its pane.
        let x = app.drawn.borrow().tabs[1].1.start;
        app.mouse(
            MouseEvent {
                kind: ratatui::crossterm::event::MouseEventKind::Down(
                    ratatui::crossterm::event::MouseButton::Left,
                ),
                column: x + 1,
                row: 0,
                modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
            },
            &legume_tui::ChildEnv::default(),
        );
        assert_eq!(app.pane, Pane::Form);
        println!("{}", screen(&app));
        key(&mut app, KeyCode::Enter);
        assert!(
            matches!(&app.pick, Some(p) if p.want == Want::Flag { long: "gene-coords".into(), many: false }),
            "enter on --gene-coords browses"
        );
        println!("{}", screen(&app));
    }

    #[test]
    fn a_step_without_count_files_opens_on_its_flags() {
        let app = App::new(super::super::step("regroup").unwrap(), &[]);
        assert_eq!(app.pane, Pane::Form);
        assert_eq!(app.panes(), [Pane::Form]);
        assert!(app.form.fields.iter().any(|f| &*f.long == "atac"));
        let shown = screen(&app);
        assert!(!shown.contains(" data ("), "{shown}");

        // Its --atac, given, is its own flag once, not a hidden sample's.
        let argv: Vec<String> = ["--run", "r", "--atac", "f", "--groups", "g", "-o", "o"]
            .map(String::from)
            .into();
        let app = App::new(super::super::step("regroup").unwrap(), &argv);
        assert!(app.data.is_none());
        let argv = app.argv();
        assert_eq!(
            argv.iter().filter(|a| *a == "--atac").count(),
            1,
            "{argv:?}"
        );
    }

    fn words(ws: &[&str]) -> Vec<String> {
        ws.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn a_step_given_after_dashes_runs_as_given() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, "x").unwrap();
            p.to_string_lossy().into_owned()
        };
        let (a, b, g) = (file("a.h5"), file("b.h5"), file("g.tsv"));
        let out = dir.path().join("out").to_string_lossy().into_owned();
        // --data (not the data pane's --rna and --atac) and a hidden flag,
        // which the form has no row for.
        let argv = words(&[
            "--data",
            &a,
            &b,
            "--gene-coords",
            &g,
            "--qc-min-counts",
            "7",
            "-o",
            &out,
        ]);
        let app = App::given(&STEPS[0], &argv);
        assert!(app.check.is_ok(), "{:?}", app.check);
        assert_eq!(app.command()[0], "peak-to-gene");
        assert_eq!(app.command()[1..], argv[..]);

        // Its inputs must be there.
        let mut gone = argv.clone();
        gone[2] = dir.path().join("no.h5").to_string_lossy().into_owned();
        let mut app = App::given(&STEPS[0], &gone);
        let why = app.check.clone().unwrap_err();
        assert!(
            why.contains("no.h5") && why.contains("does not exist"),
            "{why}"
        );

        // Its panes only move: an edit there would not reach the run.
        let env = legume_tui::ChildEnv::default();
        for pane in [Pane::Data, Pane::Form] {
            app.pane = pane;
            app.note = None;
            app.key(KeyEvent::from(KeyCode::Enter), &env);
            assert!(app.editing.is_none() && app.pick.is_none());
            assert!(app.note.as_deref().is_some_and(|n| n.contains("as given")));
        }
        app.key(KeyEvent::from(KeyCode::Down), &env);
        assert_eq!(app.command()[1..], gone[..]);
    }

    #[test]
    fn the_form_runs_with_data_set_as_a_flag() {
        let mut app = App::new(&STEPS[0], &[]);
        assert!(app.check.is_err());
        for (long, value) in [
            ("data", "a.h5 b.h5"),
            ("gene-coords", "g.tsv"),
            ("out", "o"),
        ] {
            app.form.get_mut(long).unwrap().set_text(value);
        }
        app.recheck();
        assert!(app.check.is_ok(), "{:?}", app.check);
        assert!(app.command().contains(&"--data".to_string()));
    }

    #[test]
    fn keys_held_with_ctrl_or_alt_are_not_plain_keys() {
        let mut app = App::new(super::super::step("regroup").unwrap(), &[]);
        let env = legume_tui::ChildEnv::default();
        app.key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
            &env,
        );
        app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT), &env);
        assert!(!app.quit);

        // Typed text takes plain and shifted characters only.
        let at = app
            .form
            .fields
            .iter()
            .position(|f| &*f.long == "block-size")
            .unwrap();
        app.at = at;
        key(&mut app, KeyCode::Char('e'));
        app.editing = Some(String::new());
        app.key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            &env,
        );
        app.key(KeyEvent::new(KeyCode::Char('B'), KeyModifiers::SHIFT), &env);
        key(&mut app, KeyCode::Char('c'));
        assert_eq!(app.editing.as_deref(), Some("Bc"));

        app.editing = None;
        key(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn the_log_scrolls_no_further_back_than_it_goes() {
        let mut app = App::new(&STEPS[0], &[]);
        app.run_key(KeyEvent::from(KeyCode::PageUp));
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn files_whose_names_do_not_say_are_read_off_the_screen() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = ["x.h5", "y.h5"].map(|f| dir.path().join(f)).into();
        for f in &files {
            std::fs::write(f, "x").unwrap();
        }
        let mut app = App::new(&STEPS[0], &[]);
        app.chosen(Want::Data, files);
        assert_eq!(app.note.as_deref(), Some("reading…"));
        assert!(app.check.is_err());
        for _ in 0..500 {
            if app.reading.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
            app.tick();
        }
        assert!(app.reading.is_none());
        // Unreadable, both are taken as RNA.
        assert_eq!(app.data.as_ref().unwrap().pairs.len(), 2);
        assert!(
            app.note.as_deref().unwrap().contains("2 RNA"),
            "{:?}",
            app.note
        );
    }
}
