//! `chickpea run`: set up a step in the terminal and run it.
//!
//! Every step chickpea runs is listed in [`STEPS`] with its command line,
//! so the screen's form, the check before a run and the outputs a run must
//! not write over all come from clap. A step given whole after `--` runs at
//! once: that is how `p2g_run` in the background puts a run on this screen
//! (a session job), and how a person watches one started elsewhere.

pub mod app;
pub mod data;
pub mod draw;
pub mod pick;
pub mod worker;

pub use crate::p2g::steps::{step, Step, STEPS};
use clap::Args;
use std::path::PathBuf;

/// What `step` with `argv` (its flags, after the subcommand) would write,
/// checked as chickpea parses it and its files to read there; clap's
/// complaint, on one line, if it refuses the line.
pub fn checked(step: &Step, argv: &[String]) -> Result<Vec<PathBuf>, String> {
    let line: Vec<String> = std::iter::once(step.name.to_string())
        .chain(argv.iter().cloned())
        .collect();
    let cmd = step.command();
    let m = legume_tui::form::parse(&cmd, &line)?;
    crate::p2g::steps::checked(step, &cmd, &m)
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// A step to run at once, as typed after `chickpea`:
    /// `-- peak-to-gene --rna r.zarr --atac a.zarr ...`; its name alone
    /// (`-- regroup`) opens its form
    #[arg(last = true, value_name = "STEP")]
    pub step: Vec<String>,

    #[command(flatten)]
    pub session: legume_tui::cli::SessionArgs,
}

impl RunArgs {
    /// The session to run in. A step given starts a new one; started with
    /// none, live sessions are offered first.
    pub fn session(&self) -> legume_tui::Options {
        self.session.options(self.step.len() > 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_checked_as_chickpea_parses_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, "x").unwrap();
            p.to_string_lossy().into_owned()
        };
        let (r, a, g) = (file("r.zarr"), file("a.zarr"), file("g.tsv"));
        let out = dir.path().join("x").to_string_lossy().into_owned();
        let s = step("peak-to-gene").unwrap();
        let ok: Vec<String> = ["--rna", &r, "--atac", &a, "--gene-coords", &g, "-o", &out]
            .map(String::from)
            .into();
        let made = checked(s, &ok).unwrap();
        assert!(made.iter().any(|p| p.ends_with("x.links.parquet")));
        let missing: Vec<String> = ["--rna", &r].map(String::from).into();
        assert!(checked(s, &missing).is_err());

        // A file to read that is not there refuses the line.
        let mut gone = ok.clone();
        gone[5] = dir.path().join("no.tsv").to_string_lossy().into_owned();
        let why = checked(s, &gone).unwrap_err();
        assert!(
            why.contains("--gene-coords") && why.contains("does not exist"),
            "{why}"
        );
    }
}
