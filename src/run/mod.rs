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

use crate::p2g::{PeakToGeneArgs, RegroupArgs};
use clap::{Args, FromArgMatches};
use data::Side;
use std::path::PathBuf;

/// A step `chickpea run` sets up: its subcommand and what it writes.
pub struct Step {
    /// The subcommand, as typed after `chickpea`.
    pub name: &'static str,
    /// What it does, in a line.
    pub about: &'static str,
    /// Its command line, built, named after the subcommand.
    pub command: fn() -> clap::Command,
    /// The files a run of it with these arguments writes.
    pub outputs: fn(&clap::ArgMatches) -> anyhow::Result<Vec<PathBuf>>,
    /// The flags (long names) listed first: what the step reads and writes.
    pub inputs: &'static [&'static str],
    /// The count files each sample has, set on the data pane rather than
    /// as flags.
    pub data: &'static [Side],
}

/// The steps, in the order the screen offers them.
pub const STEPS: &[Step] = &[
    Step {
        name: "peak-to-gene",
        about: "Link cis peaks to genes over a multiome embedding",
        command: || {
            let mut cmd = PeakToGeneArgs::augment_args(clap::Command::new("peak-to-gene"));
            cmd.build();
            cmd
        },
        outputs: |m| {
            let args = PeakToGeneArgs::from_arg_matches(m)?;
            args.check_pairs()?;
            Ok(args.outputs())
        },
        inputs: &["gene-coords", "gff-file", "batch", "groups", "out"],
        data: &[Side::Rna, Side::Atac],
    },
    Step {
        name: "regroup",
        about: "A finished run's per-group tables again, for other cell groups",
        command: || {
            let mut cmd = RegroupArgs::augment_args(clap::Command::new("regroup"));
            cmd.build();
            cmd
        },
        outputs: |m| Ok(RegroupArgs::from_arg_matches(m)?.outputs()),
        inputs: &["run", "groups", "atac", "out"],
        data: &[],
    },
];

/// The step named `name` (a subcommand or one of its aliases).
pub fn step(name: &str) -> anyhow::Result<&'static Step> {
    let alias = match name {
        "p2g" | "peak2gene" => "peak-to-gene",
        n => n,
    };
    STEPS.iter().find(|s| s.name == alias).ok_or_else(|| {
        let names: Vec<&str> = STEPS.iter().map(|s| s.name).collect();
        anyhow::anyhow!("no step {name:?}: one of {}", names.join(", "))
    })
}

/// What `step` with `argv` (its flags, after the subcommand) would write,
/// checked as chickpea parses it; clap's complaint, on one line, if it
/// refuses the line.
pub fn checked(step: &Step, argv: &[String]) -> Result<Vec<PathBuf>, String> {
    let line: Vec<String> = std::iter::once(step.name.to_string())
        .chain(argv.iter().cloned())
        .collect();
    let m = legume_tui::form::parse(&(step.command)(), &line)?;
    (step.outputs)(&m).map_err(|e| format!("{e:#}"))
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
    fn a_step_is_found_by_its_alias() {
        assert_eq!(step("p2g").unwrap().name, "peak-to-gene");
        assert!(step("nope").is_err());
    }

    #[test]
    fn a_line_is_checked_as_chickpea_parses_it() {
        let s = step("peak-to-gene").unwrap();
        let ok: Vec<String> = [
            "--rna",
            "r.zarr",
            "--atac",
            "a.zarr",
            "--gene-coords",
            "g.tsv",
            "-o",
            "out/x",
        ]
        .map(String::from)
        .into();
        let made = checked(s, &ok).unwrap();
        assert!(made.iter().any(|p| p.ends_with("x.links.parquet")));
        let missing: Vec<String> = ["--rna", "r.zarr"].map(String::from).into();
        assert!(checked(s, &missing).is_err());
    }
}
