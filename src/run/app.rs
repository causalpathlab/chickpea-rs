//! The run screen's state and keys: a step's count files (the data pane)
//! and its other flags as a form, then its run.

use super::data::{self, Data, SIDES};
use super::pick::{Outcome, Pick, Want};
use super::worker::Job;
use super::{checked, Step};
use crate::p2g::steps::{is_path, not_over};
use crate::tui::ListHit;
use legume_tui::browse::shown;
use legume_tui::form::{given_words, Field, Form, Kind, Many};
use legume_tui::is_run_key;
use legume_tui::keys::{is_stray, list_move};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

mod given;
mod mouse;
mod screen;

use given::{spawn_read, Given, Reading};
pub use screen::run;

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
    reading: Option<Reading>,
    /// The line given after `--`, run word for word while the screen is
    /// as that line filled it: the form's rows and the data pane cannot
    /// always say it (a path with a space, in a row split on spaces), nor
    /// `--data` as given. The first edit drops it; [`App::command`] is the
    /// screen's own line from then on.
    given: Option<Given>,
    /// Every flag but the count files, the hidden ones as advanced rows
    /// (last), so a line given after `--` is all here.
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
    /// The flags (long names) clap takes only as `--flag=value`
    /// (data-beans' `--qc-mad-on-genes=false`), which the form's rows
    /// would give as two words.
    equals: Vec<String>,
}

impl App {
    /// The screen for `step`, its form filled from `argv` (its flags),
    /// which it runs as given until it is edited.
    pub fn new(step: &'static Step, argv: &[String]) -> Self {
        let cmd = step.command();
        let mut form = Form::new(&cmd, |_| true);
        let first = |long: &str| {
            step.inputs
                .iter()
                .position(|l| *l == long)
                .unwrap_or(step.inputs.len())
        };
        form.fields.sort_by_key(|f| (f.advanced, first(&f.long)));
        let line: Vec<String> = std::iter::once(step.name.to_string())
            .chain(argv.iter().cloned())
            .collect();
        let m = legume_tui::form::parse(&cmd, &line).ok();
        if let Some(m) = &m {
            form.prefill(m);
        }
        let given = |long: &str| {
            m.as_ref()
                .and_then(|m| given_words(m, long))
                .unwrap_or_default()
        };
        // The count files are the data pane's, not rows of the form: by
        // side (`--rna`, `--atac`), or in any order (`--data`), told apart
        // by their rows as the run does.
        let (mut reading, mut note) = (None, None);
        let data = step.data.then(|| {
            let mut data = Data::default();
            data.prefill(|side| given(side.long()));
            let files: Vec<PathBuf> = given(DATA).into_iter().map(PathBuf::from).collect();
            if !files.is_empty() {
                match spawn_read(move || data::told_as_run(files)) {
                    Ok(pending) => {
                        reading = Some(Reading {
                            pending,
                            given: true,
                        });
                    }
                    Err(why) => note = Some(why),
                }
            }
            form.fields
                .retain(|f| f.long != DATA && !SIDES.iter().any(|s| s.long() == f.long));
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
            reading,
            given: None,
            form,
            at: 0,
            editing: None,
            pick: None,
            browse_dir: std::env::current_dir().unwrap_or_default(),
            pane,
            job: None,
            check: Err(String::new()),
            note,
            scroll: 0,
            quit: false,
            drawn: RefCell::new(Drawn::default()),
            equals: cmd
                .get_arguments()
                .filter(|a| a.is_require_equals_set())
                .filter_map(|a| a.get_long().map(String::from))
                .collect(),
        };
        if !argv.is_empty() {
            app.given = Some(Given {
                argv: argv.to_vec(),
                shot: app.shot(),
            });
        }
        app.recheck();
        app
    }

    /// The step's flags the screen stands for: the count files, then the
    /// form's.
    #[must_use]
    pub fn argv(&self) -> Vec<String> {
        let rows = self.form.fields.iter().flat_map(|f| {
            if self.equals.contains(&f.long) {
                with_equals(f)
            } else {
                f.argv()
            }
        });
        self.data.iter().flat_map(Data::argv).chain(rows).collect()
    }

    /// The line a run starts, as typed after `chickpea`: the step's name,
    /// then the line given as it was, until something on the screen is
    /// changed; then its flags as the screen has them.
    #[must_use]
    pub fn command(&self) -> Vec<String> {
        let argv = match self.as_given() {
            Some(given) => given.to_vec(),
            None => self.argv(),
        };
        std::iter::once(self.step.name.to_string())
            .chain(argv)
            .collect()
    }

    fn recheck(&mut self) {
        self.sync();
        self.check = if let Some(given) = self.as_given() {
            checked(self.step, given)
        } else if self.reading.is_some() {
            Err("reading the files chosen…".into())
        } else {
            match self.data.as_ref().and_then(Data::problem) {
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
        self.sync();
        let Some(reading) = self.reading.as_mut() else {
            return;
        };
        let Some(told) = reading
            .pending
            .try_take("the files chosen could not be told apart")
        else {
            return;
        };
        let given = reading.given;
        self.reading = None;
        self.note = match told {
            Ok(told) => self.data.as_mut().map(|d| d.add_read(told)),
            Err(why) => Some(why),
        };
        // The given line's own files fill the pane as it was given.
        if given {
            let shot = self.shot();
            if let Some(g) = self.given.as_mut() {
                g.shot = shot;
            }
        }
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
        self.note = Some(match spawn_read(move || data::told(files)) {
            Ok(pending) => {
                self.reading = Some(Reading {
                    pending,
                    given: false,
                });
                "reading…".into()
            }
            Err(why) => why,
        });
    }

    /// Start the run the form stands for, unless the line is refused or it
    /// would write over a run.
    pub fn start(&mut self, env: legume_tui::ChildEnv) {
        self.recheck();
        self.note = match &self.check {
            Err(why) => Some(why.clone()),
            Ok(made) => match not_over(made, "change --out") {
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
                self.edited();
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
            Kind::Text if browses(f) => {
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
}

/// The count files given in any order, set on the data pane.
const DATA: &str = "data";

/// Whether enter on `f` opens the file browser: its value is a path, by
/// the test every front end uses ([`is_path`]).
#[must_use]
pub fn browses(f: &Field) -> bool {
    is_path(f.value_hint, &f.value_name)
}

/// The words of `f`, a flag clap takes only as `--flag=value`: each value
/// in its flag's word (`--flag=a --flag=b`), as the row would give them.
fn with_equals(f: &Field) -> Vec<String> {
    let words = f.argv();
    if matches!(f.kind, Kind::Flag { .. }) {
        return words;
    }
    let flag = format!("--{}", f.long);
    let eq = format!("{flag}=");
    words
        .into_iter()
        .filter(|w| *w != flag)
        .map(|w| {
            if w.starts_with(&eq) {
                w
            } else {
                format!("{eq}{w}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::data::Pair;
    use super::super::STEPS;
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyModifiers, MouseEvent};
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

    /// Each of `line`'s values (`step` after `chickpea`), by clap id.
    fn values(step: &Step, line: &[String]) -> Vec<(String, Option<Vec<String>>)> {
        let cmd = step.command();
        let m = legume_tui::form::parse(&cmd, line).unwrap();
        cmd.get_arguments()
            .map(|a| {
                let id = a.get_id().as_str();
                let words = m
                    .get_raw(id)
                    .map(|r| r.map(|v| v.to_string_lossy().into_owned()).collect());
                (id.to_string(), words)
            })
            .collect()
    }

    #[test]
    fn every_flag_but_the_count_files_is_a_row() {
        for step in STEPS {
            let app = App::new(step, &[]);
            for a in step.command().get_arguments() {
                let Some(long) = a.get_long().filter(|l| *l != "help") else {
                    continue;
                };
                let counts = step.data && (long == DATA || SIDES.iter().any(|s| s.long() == long));
                assert_eq!(app.form.get(long).is_some(), !counts, "--{long}");
                if let Some(f) = app.form.get(long) {
                    assert_eq!(f.advanced, a.is_hide_set(), "--{long}");
                }
            }
            // The advanced rows last.
            let first = app.form.fields.iter().position(|f| f.advanced);
            assert!(first.is_none_or(|i| app.form.fields[i..].iter().all(|f| f.advanced)));
        }
    }

    /// Wait for the files being told apart, as the run loop's ticks do.
    fn settle(app: &mut App) {
        while let Some(reading) = app.reading.as_mut() {
            reading.pending.wait_for(TICK);
            app.tick();
        }
    }

    /// A count file at `dir/name` whose rows are `rows`, over two cells.
    fn counts(dir: &std::path::Path, name: &str, rows: &[&str]) -> String {
        use crate::common::{create_sparse_from_triplets, SparseIoBackend};
        let path = dir.join(name).to_string_lossy().into_owned();
        let trip: Vec<(u64, u64, f32)> = (0..rows.len() as u64).map(|r| (r, r % 2, 1.0)).collect();
        let mut m = create_sparse_from_triplets(
            &trip,
            (rows.len(), 2, trip.len()),
            Some(&path),
            Some(&SparseIoBackend::Zarr),
        )
        .unwrap();
        let rows: Vec<Box<str>> = rows.iter().map(|&r| r.into()).collect();
        m.register_row_names_vec(&rows);
        m.register_column_names_vec(&["c1".into(), "c2".into()]);
        path
    }

    /// `step`'s name, then `argv`.
    fn line(step: &Step, argv: &[String]) -> Vec<String> {
        std::iter::once(step.name.to_string())
            .chain(argv.iter().cloned())
            .collect()
    }

    #[test]
    fn a_step_given_after_dashes_runs_as_given() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, "x").unwrap();
            p.to_string_lossy().into_owned()
        };
        let (a, b, g) = (file("s1_atac.h5"), file("s1_gex.h5"), file("g.tsv"));
        let out = dir.path().join("out").to_string_lossy().into_owned();
        // --data, and flags the form has as rows, a hidden one among them.
        let argv = words(&[
            "--data",
            &a,
            &b,
            "--gene-coords",
            &g,
            "--qc-min-counts",
            "7",
            "--no-qc",
            "--cis-window=1000",
            "--qc-mad-on-genes=false",
            "-o",
            &out,
        ]);
        let mut app = App::new(&STEPS[0], &argv);
        // Unedited, it is the line given, word for word, and checked as it.
        assert_eq!(app.command(), line(&STEPS[0], &argv));
        assert!(app.check.is_ok(), "{:?}", app.check);
        let shown = screen(&app);
        assert!(shown.contains("runs as given"), "{shown}");
        settle(&mut app);
        assert_eq!(app.command(), line(&STEPS[0], &argv));
        // The files cannot be read: said, and not guessed at.
        assert!(app.data.as_ref().unwrap().pairs.is_empty());
        let note = app.note.clone().unwrap_or_default();
        assert!(
            note.contains("s1_atac.h5") && note.contains("s1_gex.h5"),
            "{note}"
        );
        // The form holds every flag as given, the hidden one too.
        let mut rows = app.argv();
        rows.extend(words(&["--data", &a, &b]));
        let ran = values(&STEPS[0], &line(&STEPS[0], &rows));
        let given = values(&STEPS[0], &line(&STEPS[0], &argv));
        for ((id, ran), (_, given)) in ran.iter().zip(&given) {
            if !["data", "rna", "atac"].contains(&id.as_str()) {
                assert_eq!(ran, given, "{id}");
            }
        }

        // Its inputs must be there.
        let mut gone = argv.clone();
        gone[1] = dir.path().join("no_atac.h5").to_string_lossy().into_owned();
        let why = checked(&STEPS[0], &gone).unwrap_err();
        assert!(
            why.contains("--data") && why.contains("no_atac.h5"),
            "{why}"
        );
        let app = App::new(&STEPS[0], &gone);
        let why = app.check.clone().unwrap_err();
        assert!(
            why.contains("no_atac.h5") && why.contains("does not exist"),
            "{why}"
        );
    }

    #[test]
    fn a_given_path_with_a_space_runs_unchanged() {
        let step = super::super::step("regroup").unwrap();
        let argv = words(&[
            "--run",
            "/data/My Project/run",
            "--atac",
            "/data/My Project/s1_atac.h5",
            "--groups",
            "/data/My Project/groups.tsv",
            "-o",
            "/data/My Project/out",
        ]);
        let app = App::new(step, &argv);
        assert_eq!(app.as_given(), Some(argv.as_slice()));
        assert_eq!(app.command(), line(step, &argv));
        // The form's row splits it: the line given is what runs.
        assert_ne!(app.argv(), argv);
    }

    #[test]
    fn an_edit_runs_the_screens_line() {
        let step = super::super::step("regroup").unwrap();
        let argv = words(&["--run", "r", "--atac", "a b.h5", "--groups", "g", "-o", "o"]);
        let at = |app: &App, long: &str| app.form.fields.iter().position(|f| f.long == long);

        // A row typed into.
        let mut app = App::new(step, &argv);
        app.at = at(&app, "out").unwrap();
        key(&mut app, KeyCode::Char('e'));
        app.editing = Some("elsewhere".into());
        key(&mut app, KeyCode::Enter);
        assert!(app.as_given().is_none());
        assert_eq!(app.command(), line(step, &app.argv()));
        assert!(app.command().contains(&"elsewhere".to_string()));
        // Changed back, it stays edited.
        app.editing = Some("o".into());
        key(&mut app, KeyCode::Enter);
        assert!(app.as_given().is_none());

        // The reset key, even on a row at its default.
        let mut app = App::new(step, &argv);
        app.at = app.form.fields.iter().position(Field::is_default).unwrap();
        key(&mut app, KeyCode::Char('d'));
        assert_eq!(app.command(), line(step, &app.argv()));

        // A row changed by any way, the screen's own keys aside: the next
        // tick sees it.
        let mut app = App::new(step, &argv);
        let i = at(&app, "groups").unwrap();
        app.form.fields[i].set_text("h");
        assert_eq!(app.command(), line(step, &app.argv()));
        app.tick();
        assert!(app.given.is_none());

        // A sample dropped from the data pane.
        let p2g = words(&[
            "--rna",
            "r.h5",
            "--atac",
            "a.h5",
            "--gene-coords",
            "g",
            "-o",
            "o",
        ]);
        let mut app = App::new(&STEPS[0], &p2g);
        assert_eq!(app.command(), line(&STEPS[0], &p2g));
        app.pane = Pane::Data;
        key(&mut app, KeyCode::Char('x'));
        assert!(app.data.as_ref().unwrap().pairs.is_empty());
        assert_eq!(app.command(), line(&STEPS[0], &app.argv()));
        assert!(app.check.is_err());
        assert!(!app.command().contains(&"--rna".to_string()));
    }

    #[test]
    fn the_given_data_filling_its_pane_is_no_edit() {
        let dir = tempfile::tempdir().unwrap();
        // Names that say nothing: told apart by their rows, as the run does.
        let rna = counts(dir.path(), "x.zarr", &["GENE1", "GENE2"]);
        let atac = counts(dir.path(), "y.zarr", &["chr1:1-500", "chr1:900-1400"]);
        let argv = words(&["--data", &atac, &rna, "--gene-coords", "g", "-o", "o"]);
        let mut app = App::new(&STEPS[0], &argv);
        assert!(app.reading.is_some());
        settle(&mut app);
        let d = app.data.as_ref().unwrap();
        assert_eq!(
            d.pairs,
            [Pair([
                Some(PathBuf::from(&rna)),
                Some(PathBuf::from(&atac))
            ])]
        );
        assert_eq!(app.as_given(), Some(argv.as_slice()));
        assert_eq!(app.command(), line(&STEPS[0], &argv));

        // An edit made while they are read is one still once they are in.
        let mut app = App::new(&STEPS[0], &argv);
        let i = app
            .form
            .fields
            .iter()
            .position(|f| f.long == "out")
            .unwrap();
        app.form.fields[i].set_text("p");
        settle(&mut app);
        assert!(app.as_given().is_none());
        assert!(app.command().contains(&"--rna".to_string()));
    }

    #[test]
    fn a_require_equals_flag_has_each_value_in_its_word() {
        let arg = clap::Arg::new("f")
            .long("f")
            .require_equals(true)
            .num_args(1..);
        let mut f = Field::from_arg(&arg).unwrap();
        f.set_text("a b");
        assert_eq!(with_equals(&f), ["--f=a", "--f=b"]);
        f.set_text("-1");
        assert_eq!(with_equals(&f), ["--f=-1"]);
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
