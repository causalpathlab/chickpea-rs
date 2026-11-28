//! A line given after `--`, run word for word until the screen is edited,
//! and the files read off the screen's thread for the data pane.

use super::App;
use crate::run::data::{Pair, Told};
use legume_tui::work::{Failed, Pending};

/// Files being told apart off the screen's thread.
pub(super) struct Reading {
    pub(super) pending: Pending<Told>,
    /// Whether they are the given line's `--data`, whose rows on the data
    /// pane are part of the line as given, not an edit.
    pub(super) given: bool,
}

/// A line given after `--`, and the screen as it filled it.
pub(super) struct Given {
    pub(super) argv: Vec<String>,
    /// Each row's value, and the data pane's samples, once filled.
    pub(super) shot: Shot,
}

/// What an edit changes: each row's value, and the data pane's samples.
pub(super) type Shot = (Vec<String>, Vec<Pair>);

/// `work` on a thread of its own, its panic its answer, as
/// [`Pending::spawn`] does; but a thread that cannot be made is said, not
/// a panic.
pub(super) fn spawn_read(
    work: impl FnOnce() -> Told + Send + 'static,
) -> Result<Pending<Told>, String> {
    let (tx, pending) = Pending::pair();
    std::thread::Builder::new()
        .name("chickpea-read".into())
        .spawn(move || {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .map_err(|panic| Failed::panicked("reading the files", &*panic).to_string());
            let _ = tx.send(out);
        })
        .map_err(|e| format!("cannot read the files: {e}"))?;
    Ok(pending)
}

impl App {
    /// The line given after `--`, while nothing on the screen has been
    /// changed since it filled it.
    #[must_use]
    pub fn as_given(&self) -> Option<&[String]> {
        let g = self.given.as_ref()?;
        let (values, pairs) = &g.shot;
        let rows = values.iter().eq(self.form.fields.iter().map(|f| &f.value));
        let data = self.data.as_ref().map_or(&[][..], |d| &d.pairs[..]);
        (rows && pairs[..] == *data).then_some(g.argv.as_slice())
    }

    /// What an edit changes, as the screen has it now.
    pub(super) fn shot(&self) -> Shot {
        let values = self.form.fields.iter().map(|f| f.value.clone()).collect();
        let pairs = self
            .data
            .as_ref()
            .map(|d| d.pairs.clone())
            .unwrap_or_default();
        (values, pairs)
    }

    /// Something was changed: the screen's line runs from now on.
    pub(super) fn edited(&mut self) {
        self.given = None;
    }

    /// Drop the given line once the screen differs from it, whatever
    /// changed it: once edited, it stays so.
    pub(super) fn sync(&mut self) {
        if self.given.is_some() && self.as_given().is_none() {
            self.edited();
        }
    }
}
