//! The file browser the run screen opens: for a step's count files,
//! several marked with space, or one file for a flag (`--gene-coords`,
//! `--groups`, a run's links). legume-tui's [`Browser`] lists, moves,
//! narrows, marks and draws; this says what is wanted.

use legume_tui::browse::{self, size, Browser, Header, Simple};
use legume_tui::Look;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Margin, Rect};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::borrow::Cow;
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
    pub browser: Browser<Simple>,
    pub want: Want,
    /// Where the browser's lines were last drawn, for a click.
    drawn: std::cell::Cell<Rect>,
}

/// Count files chickpea reads: a name data-beans takes as a backend's.
fn is_data(name: &str) -> bool {
    data_beans::hdf5_io::strip_backend_suffix(name) != name
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

impl Pick {
    /// A browser at `dir` for `want`; for a flag, at its file when it has
    /// one.
    #[must_use]
    pub fn new(dir: &Path, want: Want, at: Option<&Path>) -> Self {
        let (header, keep): (Header, fn(&str) -> bool) = match &want {
            Want::Data => (
                Header::new("count files", "count files (.zarr, .zarr.zip, .h5)"),
                is_data,
            ),
            Want::Cell => (
                Header::new("a count file", "count files (.zarr, .zarr.zip, .h5)"),
                is_data,
            ),
            Want::Flag { long, .. } => {
                let (what, keep) = wanted(long);
                (Header::new(format!("--{long}"), what), keep)
            }
        };
        let many = matches!(want, Want::Data | Want::Flag { many: true, .. });
        let simple = Simple::new(header, keep, |p, dir| {
            if dir {
                None
            } else {
                size(p).map(Cow::Owned)
            }
        });
        let simple = if many { simple.marking() } else { simple };
        let start = at
            .and_then(Path::parent)
            .filter(|d| d.is_dir())
            .unwrap_or(dir);
        let select = at
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned());
        let browser = Browser::open_within(start, simple, select.as_deref(), GRACE);
        Self {
            browser,
            want,
            drawn: std::cell::Cell::default(),
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Outcome {
        match self.browser.key(k) {
            browse::Outcome::Cancelled => Outcome::Cancelled,
            browse::Outcome::Chosen(c) => Outcome::Chosen(c.files()),
            browse::Outcome::Moved | browse::Outcome::Ignored => Outcome::Open,
        }
    }

    /// A click at `(x, y)`: on a row, the cursor goes there; on the row it
    /// is on, as enter, or as space on its mark.
    pub fn click(&mut self, x: u16, y: u16) -> Outcome {
        let area = self.drawn.get();
        if !area.contains((x, y).into()) {
            return Outcome::Open;
        }
        let (h, w) = (area.height as usize, area.width as usize);
        let Some(i) = self.browser.row_at(h, w, usize::from(y - area.y)) else {
            return Outcome::Open;
        };
        if i != self.browser.row {
            let name = self.browser.shown()[i].name().to_string();
            self.browser.select(&name);
            return Outcome::Open;
        }
        let on_mark = self.browser.has_mark_column() && (2..4).contains(&(x - area.x));
        let key = if on_mark {
            KeyCode::Char(' ')
        } else {
            KeyCode::Enter
        };
        self.key(KeyEvent::from(key))
    }

    /// Take what the folder's listing found since the last tick.
    pub fn tick(&mut self) {
        self.browser.poll();
    }

    pub fn scroll(&mut self, d: isize) {
        let key = if d < 0 { KeyCode::Up } else { KeyCode::Down };
        for _ in 0..d.unsigned_abs() {
            self.browser.key(KeyEvent::from(key));
        }
    }

    /// The popup, over most of `area`.
    pub fn draw(&self, f: &mut Frame, area: Rect, look: &Look) {
        let pop = area.inner(Margin::new(area.width / 12, area.height / 10));
        f.render_widget(Clear, pop);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(look.border);
        let inner = block.inner(pop);
        f.render_widget(block, pop);
        self.drawn.set(inner);
        let lines = self
            .browser
            .lines(inner.height as usize, inner.width as usize, look);
        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// How long the browser waits for a folder's listing before it shows the
/// folder as still listing: long under test, so a busy machine still lists
/// a test's folder before the test looks.
#[cfg(not(test))]
const GRACE: std::time::Duration = browse::LIST_GRACE;
#[cfg(test)]
const GRACE: std::time::Duration = std::time::Duration::from_secs(60);

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// The screen line, and column, where `name` is drawn.
    fn spot(t: &Terminal<TestBackend>, name: &str) -> (u16, u16) {
        let buf = t.backend().buffer();
        (0..buf.area.height)
            .find_map(|y| {
                let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                line.find(name)
                    .map(|at| (line[..at].chars().count() as u16, y))
            })
            .unwrap_or_else(|| panic!("{name} is not drawn"))
    }

    #[test]
    fn a_click_moves_to_a_row_then_takes_it_and_a_mark_click_marks() {
        let dir = tempfile::tempdir().unwrap();
        for f in ["a.zarr.zip", "b.zarr.zip", "c.zarr.zip"] {
            std::fs::write(dir.path().join(f), "x").unwrap();
        }
        let mut pick = Pick::new(dir.path(), Want::Data, None);
        let mut t = Terminal::new(TestBackend::new(80, 30)).unwrap();
        let draw = |t: &mut Terminal<TestBackend>, pick: &Pick| {
            t.draw(|f| pick.draw(f, f.area(), &Look::terminal()))
                .unwrap();
        };
        draw(&mut t, &pick);
        let (bx, by) = spot(&t, "b.zarr.zip");
        assert_eq!(pick.click(bx, by), Outcome::Open);
        assert_eq!(pick.browser.current().unwrap().name(), "b.zarr.zip");

        // On its mark, a click marks it, as space does.
        let x0 = pick.drawn.get().x;
        assert!(pick.browser.has_mark_column());
        assert_eq!(pick.click(x0 + 2, by), Outcome::Open);
        assert_eq!(pick.browser.marks.len(), 1);

        // Back on b, a click on its name takes the marked files.
        draw(&mut t, &pick);
        let (bx, by) = spot(&t, "b.zarr.zip");
        pick.click(bx, by);
        let Outcome::Chosen(files) = pick.click(bx, by) else {
            panic!("a click on the row it is on takes")
        };
        assert_eq!(files, vec![dir.path().join("b.zarr.zip")]);
    }
}
