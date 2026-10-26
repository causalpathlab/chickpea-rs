//! The file browser the run screen opens: for a step's count files,
//! several marked with space, or one file for a flag (`--gene-coords`,
//! `--groups`, a run's links). legume-tui's [`Browser`] moves, narrows and
//! lists; this keeps the marks and says what is wanted.

use crate::tui::ListHit;
use legume_tui::browse::{first_visible, tilde, Browser, Nav};
use legume_tui::look::Look;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What a chosen file fills.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Want {
    /// Count files for the data pane, several at once.
    Data,
    /// One count file for the data pane's cell under the cursor.
    Cell,
    /// The flag with this long name, and whether it takes several files.
    Flag { long: String, many: bool },
}

/// What a key or click did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Still browsing.
    Open,
    Cancelled,
    Chosen(Vec<PathBuf>),
}

pub struct Pick {
    pub browser: Browser,
    pub want: Want,
    /// Files marked to take together (data, or a flag of several).
    pub marked: BTreeSet<PathBuf>,
    title: String,
    /// The listing as last drawn.
    pub drawn: std::cell::Cell<ListHit>,
}

/// Count files chickpea reads: a name data-beans takes as a backend's.
fn is_data(name: &str) -> bool {
    data_beans::hdf5_io::strip_backend_suffix(name) != name
}

/// The flags of `cmd` that name a file to read (clap's `FilePath` hint),
/// so enter browses for one.
#[must_use]
pub fn file_flags(cmd: &clap::Command) -> Vec<String> {
    cmd.get_arguments()
        .filter(|a| a.get_value_hint() == clap::ValueHint::FilePath)
        .filter_map(|a| a.get_long().map(str::to_string))
        .collect()
}

/// What files flag `long` takes, and what the popup calls them: any file
/// when it says nothing of its kind.
fn wanted(long: &str) -> (&'static str, fn(&str) -> bool) {
    fn ends(name: &str, endings: &[&str]) -> bool {
        let name = name.strip_suffix(".gz").unwrap_or(name);
        endings.iter().any(|e| name.ends_with(e))
    }
    match long {
        "gene-coords" => ("gene TSS tables (.tsv, .txt, .csv)", |n| {
            ends(n, &[".tsv", ".txt", ".csv"])
        }),
        "gff-file" => ("annotations (.gff, .gff3, .gtf)", |n| {
            ends(n, &[".gff", ".gff3", ".gtf"])
        }),
        "batch" => ("batch labels (.txt, .tsv, .csv)", |n| {
            ends(n, &[".txt", ".tsv", ".csv"])
        }),
        "groups" => (
            "lupin rounds (.json) and cell, group tables (.parquet, .tsv, .csv)",
            |n| ends(n, &[".json", ".parquet", ".tsv", ".csv", ".txt"]),
        ),
        "atac" => ("count files (.zarr, .zarr.zip, .h5)", is_data),
        "run" => ("finished runs (.links.parquet, .regroup.json)", |n| {
            n.ends_with(".links.parquet") || n.ends_with(".regroup.json")
        }),
        _ => ("files", |_| true),
    }
}

/// A file's size, as listed beside it.
fn size(path: &Path) -> Option<String> {
    let n = std::fs::metadata(path).ok()?.len() as f64;
    let (v, unit) = [(1e9, "GB"), (1e6, "MB"), (1e3, "kB")]
        .into_iter()
        .find(|&(d, _)| n >= d)
        .map_or((n, "B"), |(d, u)| (n / d, u));
    Some(if unit == "B" {
        format!("{n} B")
    } else {
        format!("{v:.1} {unit}")
    })
}

impl Pick {
    /// A browser at `dir` for `want`; for a flag, at its file when it has
    /// one.
    #[must_use]
    pub fn new(dir: &Path, want: Want, at: Option<&Path>) -> Self {
        let (title, keep): (String, fn(&str) -> bool) = match &want {
            Want::Data => (
                "count files (.zarr, .zarr.zip, .h5): space marks several".into(),
                is_data,
            ),
            Want::Cell => ("a count file (.zarr, .zarr.zip, .h5)".into(), is_data),
            Want::Flag { long, many } => {
                let (what, keep) = wanted(long);
                let several = if *many { ": space marks several" } else { "" };
                (format!("--{long}: {what}{several}"), keep)
            }
        };
        let start = at
            .and_then(Path::parent)
            .filter(|d| d.is_dir())
            .unwrap_or(dir);
        let mut browser = Browser::new(start.to_path_buf(), keep, size).opened();
        match at.and_then(Path::file_name) {
            Some(name) => {
                browser.list.select(&name.to_string_lossy());
            }
            // On the first file, past `..` and the folders.
            None => {
                let file = browser.list.shown().position(|e| !e.dir);
                if let Some(i) = file {
                    browser.list.at = i;
                }
            }
        }
        Self {
            browser,
            want,
            marked: BTreeSet::new(),
            title,
            drawn: std::cell::Cell::new(ListHit::default()),
        }
    }

    fn many(&self) -> bool {
        matches!(self.want, Want::Data | Want::Flag { many: true, .. })
    }

    /// Mark or unmark the file under the cursor, and move on.
    fn mark(&mut self) {
        if let Some(e) = self.browser.highlighted().filter(|e| !e.dir) {
            let p = e.path.clone();
            if !self.marked.remove(&p) {
                self.marked.insert(p);
            }
        }
        self.browser.list.step(1);
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        match k.code {
            KeyCode::Esc if self.browser.list.find.text.is_empty() => return Outcome::Cancelled,
            KeyCode::Char(' ') if self.many() => {
                self.mark();
                return Outcome::Open;
            }
            KeyCode::Enter
                if !self.marked.is_empty() && self.browser.highlighted().is_none_or(|e| !e.dir) =>
            {
                return Outcome::Chosen(std::mem::take(&mut self.marked).into_iter().collect());
            }
            _ => {}
        }
        match self.browser.key(k) {
            Nav::Picked(p) => Outcome::Chosen(vec![p]),
            Nav::Moved | Nav::Ignored => Outcome::Open,
        }
    }

    /// A click at `(x, y)`: on a row, the cursor goes there; on the row it
    /// is on, as enter (or space, on a data file's mark).
    pub fn click(&mut self, x: u16, y: u16) -> Outcome {
        let hit = self.drawn.get();
        let Some(i) = hit.row_at(x, y) else {
            return Outcome::Open;
        };
        if i >= self.browser.list.len() {
            return Outcome::Open;
        }
        if i != self.browser.list.at {
            self.browser.list.at = i;
            return Outcome::Open;
        }
        if self.many() && x < hit.rows.x + 6 {
            self.mark();
            return Outcome::Open;
        }
        self.key(KeyEvent::from(KeyCode::Enter))
    }

    pub fn scroll(&mut self, d: isize) {
        self.browser.list.step(d);
    }

    /// The popup, over most of `area`.
    pub fn draw(&self, f: &mut Frame, area: Rect, look: &Look) {
        let pop = area.inner(Margin::new(area.width / 12, area.height / 10));
        f.render_widget(Clear, pop);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(look.border)
            .title(format!(" {} {}", self.title, self.browser.list.find.tag()));
        let inner = block.inner(pop);
        f.render_widget(block, pop);
        let [here, rows, foot] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(inner);
        f.render_widget(
            Paragraph::new(Span::styled(tilde(&self.browser.cwd), look.bold)),
            here,
        );
        let n = rows.height as usize;
        let first = first_visible(self.browser.list.at, self.browser.list.len(), n);
        self.drawn.set(ListHit { rows, first });
        let mark_w = if self.many() { 3 } else { 0 };
        let lines = self.browser.lines(
            n,
            rows.width as usize,
            look,
            mark_w,
            |e| {
                Some(
                    if self.marked.contains(&e.path) {
                        "[x]"
                    } else {
                        "[ ]"
                    }
                    .into(),
                )
            },
            "no such files here: ← goes up, / or ~ jumps",
        );
        f.render_widget(Paragraph::new(lines), rows);
        let keys = if self.many() {
            format!(
                "space marks ({} marked) · enter takes · → opens · ← up · type to find · / ~ jump · esc",
                self.marked.len()
            )
        } else {
            "enter takes · → opens · ← up · type to find · / ~ jump · esc".into()
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(keys, look.hint))),
            foot,
        );
    }
}
