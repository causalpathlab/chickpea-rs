//! The run screen's state and keys: a step's count files (the data pane)
//! and its other flags as a form, then its run.

use super::data::Data;
use super::pick::{file_flags, Outcome, Pick, Want};
use super::worker::{Ended, Job};
use super::{checked, Step, STEPS};
use crate::tui::{click, wheel, ListHit};
use crate::tui::{is_run_key, TOOL};
use legume_mcp::job::{End, Reporter};
use legume_tui::form::{Form, Kind};
use legume_tui::Round;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, MouseEvent};
use std::cell::RefCell;
use std::path::PathBuf;
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
    pub data: Data,
    pub form: Form,
    /// The form's flags that name a file to read: enter browses for one.
    pub files: Vec<String>,
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
        let mut data = Data::new(step.data);
        data.prefill(|side| {
            m.as_ref()
                .and_then(|m| legume_tui::form::given_words(m, side.long()))
                .unwrap_or_default()
        });
        form.fields
            .retain(|f| !step.data.iter().any(|s| s.long() == f.long));
        let pane = if data.pairs.is_empty() {
            Pane::Data
        } else {
            Pane::Form
        };
        let mut app = Self {
            step,
            data,
            form,
            files: file_flags(&cmd),
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

    /// The step's flags: the count files, then the form's.
    #[must_use]
    pub fn argv(&self) -> Vec<String> {
        self.data
            .argv()
            .into_iter()
            .chain(self.form.argv())
            .collect()
    }

    /// `chickpea` and the whole line the form stands for.
    #[must_use]
    pub fn command(&self) -> Vec<String> {
        std::iter::once(self.step.name.to_string())
            .chain(self.argv())
            .collect()
    }

    fn recheck(&mut self) {
        self.check = match self.data.problem() {
            Some(why) => Err(why),
            None => checked(self.step, &self.argv()),
        };
    }

    /// Start the run the form stands for, unless the line is refused or it
    /// would write over a run.
    pub fn start(&mut self, env: legume_tui::ChildEnv) {
        self.recheck();
        self.note = match &self.check {
            Err(why) => Some(why.clone()),
            Ok(made) => match made.iter().find(|p| p.exists()) {
                Some(there) => Some(format!(
                    "{} exists: a run is not written over; change --out",
                    crate::tui::shown(there)
                )),
                None => match Job::start(self.command(), env) {
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
    fn panes(&self) -> Vec<Pane> {
        let mut v = vec![Pane::Data, Pane::Form];
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
            Want::Data => self.note = Some(self.data.add(files)),
            Want::Cell => {
                if let Some(f) = files.into_iter().next() {
                    self.data.set(f);
                }
            }
            Want::Flag(long) => {
                if let (Some(f), Some(field)) = (files.first(), self.form.get_mut(&long)) {
                    field.set_text(&crate::tui::shown(f));
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
                KeyCode::Char(c) => text.push(c),
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
        match k.code {
            KeyCode::Tab => return self.next_pane(1),
            KeyCode::BackTab => return self.next_pane(-1),
            _ => {}
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
        let n = self.data.pairs.len();
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit_key(),
            KeyCode::Down | KeyCode::Char('j') => self.data.at = (self.data.at + 1).min(n),
            KeyCode::Up | KeyCode::Char('k') => self.data.at = self.data.at.saturating_sub(1),
            KeyCode::Left | KeyCode::Char('h') => self.data.col = self.data.col.saturating_sub(1),
            KeyCode::Right | KeyCode::Char('l') => {
                self.data.col = (self.data.col + 1).min(self.data.sides.len() - 1);
            }
            KeyCode::Char('a') | KeyCode::Char('+') => self.browse(Want::Data, None),
            KeyCode::Enter | KeyCode::Char(' ') if self.data.at >= n => {
                self.browse(Want::Data, None);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let at = self.data.pairs[self.data.at].get(self.data.side()).cloned();
                self.browse(Want::Cell, at);
            }
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
                self.data.remove();
                self.data.at = self.data.at.min(self.data.pairs.len());
                self.recheck();
            }
            _ => {}
        }
    }

    fn form_key(&mut self, k: KeyEvent) {
        let n = self.form.fields.len();
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit_key(),
            KeyCode::Down | KeyCode::Char('j') => self.at = (self.at + 1).min(n.saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => self.at = self.at.saturating_sub(1),
            KeyCode::PageDown => self.at = (self.at + 10).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.at = self.at.saturating_sub(10),
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
            Kind::Text if self.files.contains(&f.long) => {
                let at = (!f.value.trim().is_empty()).then(|| PathBuf::from(f.value.trim()));
                let want = Want::Flag(f.long.clone());
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
    }

    /// Take the mouse: the wheel scrolls what is under the cursor's pane,
    /// a click on a tab opens its pane, a click on a row puts the cursor
    /// there, and a click on the row it is on changes it, as enter does.
    pub fn mouse(&mut self, m: MouseEvent, env: &legume_tui::ChildEnv) {
        let wheel = wheel(&m);
        if wheel.is_none() && !click(&m) {
            return;
        }
        if let Some(pick) = self.pick.as_mut() {
            let out = match wheel {
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
        if let Some(d) = wheel {
            // As the arrow keys, but the log three lines a turn.
            let code = if d < 0 { KeyCode::Up } else { KeyCode::Down };
            let turns = if self.pane == Pane::Run { 3 } else { 1 };
            for _ in 0..turns {
                self.key(KeyEvent::from(code), env);
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
            Pane::Data if i <= self.data.pairs.len() => {
                let col = drawn.cols.iter().rposition(|&x| m.column >= x).unwrap_or(0);
                if i == self.data.at && (col == self.data.col || i == self.data.pairs.len()) {
                    self.data_key(KeyEvent::from(KeyCode::Enter));
                } else {
                    self.data.at = i;
                    self.data.col = col;
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
    crate::tui::need_terminal(
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
    let mut app = App::new(step, argv);
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
    let end = crate::tui::with_terminal(&session, |terminal| {
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
            "pbmc1.rna.zarr.zip",
            "pbmc1.atac.zarr.zip",
            "pbmc2.rna.zarr.zip",
            "pbmc2.atac.zarr.zip",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(f), "x").unwrap();
        }
        let mut app = App::new(&STEPS[0], &[]);
        app.browse_dir = dir.path().to_path_buf();
        assert_eq!(app.pane, Pane::Data);
        key(&mut app, KeyCode::Char('a'));
        let shown = screen(&app);
        assert!(shown.contains("pbmc1.atac.zarr.zip"), "{shown}");
        assert!(!shown.contains("notes.txt"), "{shown}");
        // Mark all four with space; enter takes them.
        for _ in 0..4 {
            key(&mut app, KeyCode::Char(' '));
        }
        key(&mut app, KeyCode::Enter);
        assert!(app.pick.is_none());
        assert_eq!(app.data.pairs.len(), 2);
        let shown = screen(&app);
        println!("{shown}");
        assert!(shown.contains("pbmc2"), "{shown}");
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
            matches!(&app.pick, Some(p) if p.want == Want::Flag("gene-coords".into())),
            "enter on --gene-coords browses"
        );
        println!("{}", screen(&app));
    }
}
