//! chickpea's steps as their command lines say: each step's name, its
//! command line built from clap, the files a line of it writes, and the
//! rules every front end checks a line by before it runs (`chickpea run`'s
//! screen, `chickpea mcp`'s tools, and the steps themselves): the files it
//! reads are there, and it writes over no run.

use crate::p2g::workflow::{is_run, prefix_of};
use crate::p2g::{PeakToGeneArgs, RegroupArgs};
use clap::{Arg, ArgMatches, Args, Command, FromArgMatches, ValueHint};
use legume_mcp::cli::settable;
use std::path::{Path, PathBuf};

/// A step chickpea runs: its subcommand and what it writes.
pub struct Step {
    /// The subcommand, as typed after `chickpea`.
    pub name: &'static str,
    /// Its other names on the command line.
    pub aliases: &'static [&'static str],
    /// What it does, in a line.
    pub about: &'static str,
    /// Its command line, built, under the name given.
    pub build: fn(&'static str) -> Command,
    /// The files a run of it with these arguments writes.
    pub outputs: fn(&ArgMatches) -> anyhow::Result<Vec<PathBuf>>,
    /// The flags (long names) listed first: what the step reads and writes.
    pub inputs: &'static [&'static str],
    /// Whether it reads each sample's RNA and ATAC, which `chickpea run`
    /// sets on its data pane rather than as flags.
    pub data: bool,
}

impl Step {
    /// Its command line, built, named after the subcommand.
    #[must_use]
    pub fn command(&self) -> Command {
        (self.build)(self.name)
    }
}

/// peak-to-gene's other names on the command line.
pub const P2G_ALIASES: [&str; 2] = ["p2g", "peak2gene"];

/// peak-to-gene.
pub const P2G: Step = Step {
    name: "peak-to-gene",
    aliases: &P2G_ALIASES,
    about: "Link cis peaks to genes over a multiome embedding",
    build: built::<PeakToGeneArgs>,
    outputs: p2g_outputs,
    inputs: &["gene-coords", "gff-file", "batch", "groups", "out"],
    data: true,
};

/// regroup.
pub const REGROUP: Step = Step {
    name: "regroup",
    aliases: &[],
    about: "A finished run's per-group tables again, for other cell groups",
    build: built::<RegroupArgs>,
    outputs: regroup_outputs,
    inputs: &["run", "groups", "atac", "out"],
    data: false,
};

/// The steps, in the order `chickpea run` offers them.
pub const STEPS: &[Step] = &[P2G, REGROUP];

/// The step named `name` (a subcommand or one of its aliases).
pub fn step(name: &str) -> anyhow::Result<&'static Step> {
    STEPS
        .iter()
        .find(|s| s.name == name || s.aliases.contains(&name))
        .ok_or_else(|| {
            let names: Vec<&str> = STEPS.iter().map(|s| s.name).collect();
            anyhow::anyhow!("no step {name:?}: one of {}", names.join(", "))
        })
}

/// The command line of `A`, built, named `name`.
#[must_use]
pub fn built<A: Args>(name: &'static str) -> Command {
    let mut cmd = A::augment_args(Command::new(name));
    cmd.build();
    cmd
}

/// The files a peak-to-gene line, parsed as `m`, writes: checked as
/// before a run, its `--rna` and `--atac` paired.
pub fn p2g_outputs(m: &ArgMatches) -> anyhow::Result<Vec<PathBuf>> {
    let args = PeakToGeneArgs::from_arg_matches(m)?;
    args.check_pairs()?;
    Ok(args.outputs())
}

/// The files a regroup line, parsed as `m`, writes.
pub fn regroup_outputs(m: &ArgMatches) -> anyhow::Result<Vec<PathBuf>> {
    Ok(RegroupArgs::from_arg_matches(m)?.outputs())
}

/// Refuse a run that would write over one: the first of `made` there,
/// and what to do `instead`.
pub fn not_over(made: &[PathBuf], instead: &str) -> anyhow::Result<()> {
    match made.iter().find(|p| p.exists()) {
        Some(there) => anyhow::bail!(
            "{} exists: a run is not written over; {instead}",
            there.display()
        ),
        None => Ok(()),
    }
}

/// Whether a value with clap's value `hint` and value name `name` is a
/// path: a file, a folder or either by its hint, or by its name (`FILE`,
/// `PATH`, `DIR`). The name counts because flags of crates chickpea does
/// not own name their files only so (legume-genomic-types'
/// `--covariate-genes FILE`). `chickpea run`'s form browses for such a
/// flag's file, and every front end checks it is there when the run reads it.
#[must_use]
pub fn is_path(hint: ValueHint, name: &str) -> bool {
    matches!(
        hint,
        ValueHint::FilePath | ValueHint::AnyPath | ValueHint::DirPath
    ) || matches!(name, "FILE" | "PATH" | "DIR")
}

/// Whether `arg` takes a path, as its declaration says ([`is_path`]).
#[must_use]
pub fn takes_path(arg: &Arg) -> bool {
    let name = arg
        .get_value_names()
        .and_then(|ns| ns.first())
        .map_or("", |n| n.as_str());
    is_path(arg.get_value_hint(), name)
}

/// Whether `path` is there to read: a file or folder, or a run's prefix.
fn is_there(path: &str) -> bool {
    Path::new(path).exists() || is_run(&prefix_of(path))
}

/// Why a line parsed as `m` by `cmd` cannot start: the first file given on
/// it to read that is not there. A file is read when its flag takes a path
/// ([`takes_path`]) and the line does not write it (one of `made`, the
/// files the line writes).
#[must_use]
pub fn missing_input(cmd: &Command, m: &ArgMatches, made: &[PathBuf]) -> Option<String> {
    settable(cmd).filter(|a| takes_path(a)).find_map(|a| {
        let id = a.get_id().as_str();
        // legume-tui's `given_words`, but this is built without the tui.
        if m.value_source(id) != Some(clap::parser::ValueSource::CommandLine) {
            return None;
        }
        let path = m
            .get_raw(id)?
            .map(|v| v.to_string_lossy())
            .filter(|p| !made.iter().any(|w| w == Path::new(p.as_ref())))
            .find(|p| !is_there(p))?;
        Some(format!(
            "--{} {path} does not exist",
            a.get_long().unwrap_or(id)
        ))
    })
}

/// What `step`'s line `m` (parsed by `cmd`) writes, once it may start: its
/// files to read there.
pub fn checked(step: &Step, cmd: &Command, m: &ArgMatches) -> Result<Vec<PathBuf>, String> {
    let made = (step.outputs)(m).map_err(|e| format!("{e:#}"))?;
    match missing_input(cmd, m, &made) {
        Some(why) => Err(why),
        None => Ok(made),
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
    fn the_count_files_take_paths() {
        let cmd = P2G.command();
        for long in ["data", "rna", "atac", "gene-coords", "covariate-genes"] {
            let a = cmd.get_arguments().find(|a| a.get_long() == Some(long));
            assert!(a.is_some_and(takes_path), "--{long}");
        }
        let out = cmd.get_arguments().find(|a| a.get_long() == Some("out"));
        assert!(!out.is_some_and(takes_path));
    }
}
