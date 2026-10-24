//! The run screen's state and keys: a step's flags as a form, then its run.

use super::worker::{Ended, Job};
use super::{checked, Step, STEPS};
use crate::tui::{is_run_key, TOOL};
use legume_mcp::job::{End, Reporter};
use legume_tui::form::{Form, Kind};
use legume_tui::Round;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use std::path::PathBuf;
use std::time::Duration;

/// How long a round waits for a key.
const TICK: Duration = Duration::from_millis(100);

/// What the screen shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// The step's flags.
    Form,
    /// The run and its log.
    Run,
}

/// The screen.
pub struct App {
    pub step: &'static Step,
    pub form: Form,
    /// The row under the cursor.
    pub at: usize,
    /// The text being typed into the row under the cursor.
    pub editing: Option<String>,
    pub pane: Pane,
    pub job: Option<Job>,
    /// What the form's line would write, or clap's complaint.
    pub check: Result<Vec<PathBuf>, String>,
    /// A line for the user: why a run did not start, say.
    pub note: Option<String>,
    /// Log lines scrolled back from the newest.
    pub scroll: usize,
    pub quit: bool,
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
        if let Ok(m) = legume_tui::form::parse(&cmd, &line) {
            form.prefill(&m);
        }
        let mut app = Self {
            step,
            form,
            at: 0,
            editing: None,
            pane: Pane::Form,
            job: None,
            check: Err(String::new()),
            note: None,
            scroll: 0,
            quit: false,
        };
        app.recheck();
        app
    }

    /// The step's flags as the form has them.
    #[must_use]
    pub fn argv(&self) -> Vec<String> {
        self.form.argv()
    }

    /// `chickpea` and the whole line the form stands for.
    #[must_use]
    pub fn command(&self) -> Vec<String> {
        std::iter::once(self.step.name.to_string())
            .chain(self.argv())
            .collect()
    }

    fn recheck(&mut self) {
        self.check = checked(self.step, &self.argv());
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

    /// Take a key.
    pub fn key(&mut self, k: KeyEvent, env: &legume_tui::ChildEnv) {
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
        match self.pane {
            Pane::Form => self.form_key(k),
            Pane::Run => self.run_key(k),
        }
    }

    fn form_key(&mut self, k: KeyEvent) {
        let n = self.form.fields.len();
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc if !self.running() => self.quit = true,
            KeyCode::Char('q') | KeyCode::Esc => {
                self.note = Some("a run is going on: Tab, then x stops it, then q quits".into());
            }
            KeyCode::Down | KeyCode::Char('j') => self.at = (self.at + 1).min(n.saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => self.at = self.at.saturating_sub(1),
            KeyCode::PageDown => self.at = (self.at + 10).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.at = self.at.saturating_sub(10),
            KeyCode::Tab if self.job.is_some() => self.pane = Pane::Run,
            KeyCode::Char('d') => {
                self.form.fields[self.at].reset();
                self.recheck();
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let f = &mut self.form.fields[self.at];
                match f.kind {
                    Kind::Flag { .. } => f.toggle(),
                    Kind::Choice(_) => f.cycle(1),
                    Kind::Text => self.editing = Some(f.value.clone()),
                }
                self.recheck();
            }
            KeyCode::Left | KeyCode::Right => {
                let d = if k.code == KeyCode::Left { -1 } else { 1 };
                self.form.fields[self.at].cycle(d);
                self.recheck();
            }
            _ => {}
        }
    }

    fn run_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('x') => {
                if let Some(job) = &self.job {
                    job.stop();
                }
            }
            KeyCode::Char('q') if !self.running() => self.quit = true,
            KeyCode::Char('q') => {
                self.note = Some("a run is going on: x stops it, then q quits".into());
            }
            KeyCode::Tab | KeyCode::Char('e') => self.pane = Pane::Form,
            KeyCode::Up | KeyCode::Char('k') => self.scroll += 1,
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageUp => self.scroll += 20,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(20),
            KeyCode::End => self.scroll = 0,
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
