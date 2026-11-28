//! `chickpea mcp`: chickpea as a Model Context Protocol server. Its tools
//! list peak-to-gene's flags, run it as a child `chickpea` (a background
//! job unless asked otherwise, since a run takes minutes to hours) in the
//! project's records, and read the links a run wrote.

use crate::p2g::workflow::{read_links, table, LinkColumns, LINKS};
use crate::p2g::{PeakToGeneArgs, RegroupArgs};
use anyhow::Context;
use clap::{Arg, ArgMatches, Args, FromArgMatches, ValueHint};
use legume_mcp::cli::{check, describe, find, settable, to_argv};
use legume_mcp::{command_line, parse_args, shown_path, Call, Content, Handler, ToolSpec};
use rustc_hash::FxHashSet;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// The view tools: the `tui` feature's viewer, or none without it.
#[cfg(feature = "tui")]
use crate::view::mcp as viewer;

#[cfg(not(feature = "tui"))]
mod viewer {
    use legume_mcp::{Call, Content, ToolSpec};
    use serde_json::Value;

    /// No viewer: it lists no tools and answers none.
    #[derive(Default)]
    pub struct Viewer;

    impl Viewer {
        pub fn call(&mut self, name: &str, _: Value, _: &Call) -> anyhow::Result<Vec<Content>> {
            unreachable!("{name}: no view tool is listed without the tui feature")
        }

        #[must_use]
        pub fn answers(_: &str) -> bool {
            false
        }
    }

    #[must_use]
    pub fn specs() -> Vec<ToolSpec> {
        Vec::new()
    }
}

/// The server's name, as `initialize` reports it.
pub const SERVER: &str = "chickpea";

/// What the agent is told when it connects: what the tools are for, and in
/// which order to call them.
pub const INSTRUCTIONS: &str = "\
Use chickpea's MCP tools to link ATAC peaks to RNA genes in paired single-cell \
RNA + ATAC data. `p2g_flags` lists peak-to-gene's flags (help, default, \
choices). `p2g_run` runs it on each sample's RNA and ATAC counts for the \
same barcodes (rna and atac, one file each per sample; or data, the files in \
any order, paired by name, a file of genes and peaks together a sample by \
itself), with gene positions from gene_coords (a gene, chr, tss TSV) or \
gff_file: checked as chickpea parses it, in the background (follow its job \
with `job_status` / `job_wait`), and never over a run. A background run \
goes on `chickpea run`'s screen in a session of its own: tell the user the \
`chickpea attach` command its status names, which shows it. When it is done, \
`p2g_links` reads the run's links (gene, peak, distance, abc, gate, corr, \
score), the best first, for all genes or the ones named. Per-group links come \
from chickpea's own Leiden clusters, or from cell types: give p2g_run `groups` \
(a lupin round's manifest, its argmax table, or a cell, group table), or redo \
a finished run's groups in minutes with `p2g_regroup`. To look at a run as \
`chickpea view` does: `view_open` it, then `view_genome` ranks its genes by \
their links and `view_locus` shows one gene's cis peaks; `view_export` writes \
either as PDF or PNG.";

/// The run's own arguments, by clap id, which is also how `p2g_run` names
/// them: given by name, never as flags.
const OWN: [&str; 8] = [
    "data",
    "rna",
    "atac",
    "gene_coords",
    "gff_file",
    "batch",
    "groups",
    "out",
];

/// The tools, and peak-to-gene's command line to read the flags from and
/// check a run against.
pub struct Server {
    cli: clap::Command,
    regroup: clap::Command,
    viewer: viewer::Viewer,
}

impl Default for Server {
    fn default() -> Self {
        Server {
            cli: p2g_command("chickpea peak-to-gene"),
            regroup: regroup_command("chickpea regroup"),
            viewer: Default::default(),
        }
    }
}

/* The steps' command lines, as `p2g_run`, `p2g_regroup` and `chickpea run`
 * check them. */

/// peak-to-gene's other names on the command line.
pub const P2G_ALIASES: [&str; 2] = ["p2g", "peak2gene"];

/// The flags (clap ids) whose values name files a run reads, though their
/// value hint does not say so: the count files.
const COUNTS: [&str; 3] = ["data", "rna", "atac"];

/// The flags (clap ids) whose value names a file a run writes, though
/// their declaration does not say it is a path.
const WRITES: [&str; 1] = ["qc_report"];

/// peak-to-gene's command line, built, named `name`.
#[must_use]
pub fn p2g_command(name: &'static str) -> clap::Command {
    let mut cmd = PeakToGeneArgs::augment_args(clap::Command::new(name));
    cmd.build();
    cmd
}

/// regroup's command line, built, named `name`.
#[must_use]
pub fn regroup_command(name: &'static str) -> clap::Command {
    let mut cmd = RegroupArgs::augment_args(clap::Command::new(name));
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

/// Whether `arg` takes a path, as its declaration says: by its value hint
/// (a file, a folder or either) or its value name (`FILE`, `PATH`, `DIR`).
#[must_use]
pub fn takes_path(arg: &Arg) -> bool {
    matches!(
        arg.get_value_hint(),
        ValueHint::FilePath | ValueHint::AnyPath | ValueHint::DirPath
    ) || arg.get_value_names().is_some_and(|ns| {
        ns.iter()
            .any(|n| matches!(n.as_str(), "FILE" | "PATH" | "DIR"))
    })
}

/// Whether `arg` names files a run reads: one that takes a path, or a
/// count file, unless it is one the run writes.
fn reads_path(arg: &Arg) -> bool {
    let id = arg.get_id().as_str();
    (takes_path(arg) || COUNTS.contains(&id)) && !WRITES.contains(&id)
}

/// Whether `path` is there to read: a file or folder, or a run's prefix.
fn is_there(path: &str) -> bool {
    use crate::p2g::workflow::{is_run, prefix_of};
    Path::new(path).exists() || is_run(&prefix_of(path))
}

/// Why a line parsed as `m` by `cmd` cannot start: the first file given on
/// it to read ([`reads_path`]) that is not there.
#[must_use]
pub fn missing_input(cmd: &clap::Command, m: &ArgMatches) -> Option<String> {
    settable(cmd).filter(|a| reads_path(a)).find_map(|a| {
        let id = a.get_id().as_str();
        if m.value_source(id) != Some(clap::parser::ValueSource::CommandLine) {
            return None;
        }
        let path = m
            .get_raw(id)?
            .map(|v| v.to_string_lossy())
            .find(|p| !is_there(p))?;
        Some(format!(
            "--{} {path} does not exist",
            a.get_long().unwrap_or(id)
        ))
    })
}

/// `flags` (an agent's map of flag to value) with each path from the
/// project's root, as [`push_paths`] gives the run's own: the values of
/// every flag that takes one, refused unless there when the run reads it.
/// Unknown flags and those of `own` (the tool's own arguments) are left
/// for [`to_argv`] to refuse.
fn absolute_flags(
    cmd: &clap::Command,
    flags: &Map<String, Value>,
    own: &[&str],
) -> anyhow::Result<Map<String, Value>> {
    let mut out = flags.clone();
    for (key, value) in &mut out {
        let Some(arg) = find(cmd, key) else { continue };
        let id = arg.get_id().as_str();
        if own.contains(&id) || (!takes_path(arg) && !WRITES.contains(&id)) {
            continue;
        }
        let reads = reads_path(arg);
        let flag = format!("--{}", arg.get_long().unwrap_or(id));
        let absolute = |v: &mut Value| -> anyhow::Result<()> {
            if let Value::String(p) = v {
                let path = shown_path(Path::new(p.as_str()));
                anyhow::ensure!(!reads || is_there(&path), "{flag} {path} does not exist");
                *p = path;
            }
            Ok(())
        };
        match value {
            Value::Array(vs) => vs.iter_mut().try_for_each(absolute)?,
            v => absolute(v)?,
        }
    }
    Ok(out)
}

/// `out` as given, from the project's root; else the first prefix named
/// `name` under the project's results at which `outputs_of` finds nothing.
fn out_prefix(
    cx: &Call,
    out: Option<&str>,
    name: &str,
    outputs_of: fn(&str) -> Vec<PathBuf>,
) -> anyhow::Result<String> {
    Ok(match out.map(str::trim) {
        Some(out) if !out.is_empty() => shown_path(Path::new(out)),
        _ => {
            cx.free_prefix(name, |out| Ok(outputs_of(out)), |_| false)
                .context("give out, the output prefix")?
                .0
        }
    })
}

/// What `argv` writes, checked by `outputs` as `cmd` parses it, each from
/// the project's root; refused when it would write over a run, with what
/// to do `instead`.
fn checked_outputs(
    cmd: &clap::Command,
    argv: &[String],
    outputs: fn(&ArgMatches) -> anyhow::Result<Vec<PathBuf>>,
    instead: &str,
) -> anyhow::Result<Vec<PathBuf>> {
    let made: Vec<PathBuf> = outputs(&check(cmd, argv)?)?
        .iter()
        .map(|p| shown_path(p).into())
        .collect();
    not_over(&made, instead)?;
    Ok(made)
}

impl Handler for Server {
    fn tools(&self) -> Vec<ToolSpec> {
        [specs(), viewer::specs()].concat()
    }

    fn call(&mut self, name: &str, args: Value, cx: &Call) -> anyhow::Result<Vec<Content>> {
        if viewer::Viewer::answers(name) {
            return self.viewer.call(name, args, cx);
        }
        let reply = match name {
            "p2g_flags" => self.flags(),
            "p2g_run" => self.run(parse_args(args)?, cx)?,
            "p2g_regroup" => self.regroup(parse_args(args)?, cx)?,
            "p2g_links" => links(parse_args(args)?)?,
            _ => unreachable!("the server answers unknown tools itself"),
        };
        Ok(vec![Content::Json(reply)])
    }
}

impl Server {
    /// The flags an agent sees: the visible ones, without the run's own.
    fn flags(&self) -> Value {
        let flags: Vec<Value> = settable(&self.cli)
            .filter(|a| !a.is_hide_set() && !OWN.contains(&a.get_id().as_str()))
            .map(describe)
            .collect();
        json!({ "command": self.cli.get_name(), "flags": flags })
    }

    fn run(&self, a: RunArgs, cx: &Call) -> anyhow::Result<Value> {
        // The flags first, so a misplaced input is named as such.
        let flags = absolute_flags(&self.cli, &a.flags, &OWN)?;
        let flags = to_argv(&self.cli, &flags, &OWN.map(|id| (id, id)), "p2g_flags")?;
        let positions = match (&a.gene_coords, &a.gff_file) {
            (Some(tsv), None) => ("--gene-coords", tsv),
            (None, Some(gff)) => ("--gff-file", gff),
            _ => anyhow::bail!("give one of gene_coords (a gene, chr, tss TSV) or gff_file"),
        };
        let mut argv: Vec<String> = Vec::new();
        for (flag, paths) in [("--data", &a.data), ("--rna", &a.rna), ("--atac", &a.atac)] {
            if let Some(p) = paths {
                push_paths(&mut argv, flag, &p.0)?;
            }
        }
        for (flag, path) in [positions]
            .into_iter()
            .chain(a.batch.as_ref().map(|b| ("--batch", b)))
            .chain(a.groups.as_ref().map(|g| ("--groups", g)))
        {
            push_paths(&mut argv, flag, std::slice::from_ref(path))?;
        }
        let out = out_prefix(cx, a.out.as_deref(), "p2g", PeakToGeneArgs::outputs_of)?;
        argv.extend(["--out".into(), out.clone()]);
        argv.extend(flags);
        let made = checked_outputs(&self.cli, &argv, p2g_outputs, "give another out or path")?;

        let files: Vec<&str> = [&a.data, &a.rna, &a.atac]
            .into_iter()
            .flatten()
            .flat_map(|p| p.0.iter().map(String::as_str))
            .collect();
        let summary = format!("peak-to-gene links of {}", files.join(", "));
        let command = [String::from("peak-to-gene")]
            .into_iter()
            .chain(argv)
            .collect::<Vec<_>>();
        launch(cx, &out, &command, &made, &summary, a.background)
    }

    fn regroup(&self, a: RegroupCall, cx: &Call) -> anyhow::Result<Value> {
        let mut argv: Vec<String> = Vec::new();
        let run = crate::p2g::workflow::prefix_of(&shown_path(Path::new(&a.run)));
        crate::p2g::workflow::trained_run(&run)?;
        argv.extend(["--run".into(), run.clone()]);
        if !a.atac.0.is_empty() {
            push_paths(&mut argv, "--atac", &a.atac.0)?;
        }
        push_paths(&mut argv, "--groups", std::slice::from_ref(&a.groups))?;
        let name = format!("regroup.{}", groups_name(&a.groups));
        let out = out_prefix(cx, a.out.as_deref(), &name, RegroupArgs::outputs_of)?;
        argv.extend(["--out".into(), out.clone()]);
        let made = checked_outputs(&self.regroup, &argv, regroup_outputs, "give another out")?;
        let summary = format!("{run}'s links regrouped by {}", a.groups);
        let command = [String::from("regroup")]
            .into_iter()
            .chain(argv)
            .collect::<Vec<_>>();
        launch(cx, &out, &command, &made, &summary, a.background)
    }
}

/// This binary run with `argv`, at info logs unless `RUST_LOG` says
/// otherwise, so a run's progress reaches whoever follows it.
pub fn own_command<S: AsRef<std::ffi::OsStr>>(
    argv: impl IntoIterator<Item = S>,
) -> anyhow::Result<std::process::Command> {
    let mut cmd = legume_mcp::own_command(argv)?;
    if std::env::var_os("RUST_LOG").is_none() {
        cmd.env("RUST_LOG", "info");
    }
    Ok(cmd)
}

/// Run `chickpea {command}`, which writes `made`, in the project's records:
/// in the background (see [`background`]) or waiting for it.
fn launch(
    cx: &Call,
    out: &str,
    command: &[String],
    made: &[PathBuf],
    summary: &str,
    in_background: bool,
) -> anyhow::Result<Value> {
    let cmd = own_command(command)?;
    let mut session: Option<String> = None;
    let (key, job, note) = if in_background {
        let job;
        (job, session) = background(cx, out, command, cmd, made, summary)?;
        let note = match &session {
            Some(s) => format!(
                "it runs on chickpea run's screen in session {s} (`chickpea attach {s}` shows it to the user) and goes on if this server ends; job_wait (id {job}) or job_status until it ends, then p2g_links {out} or view_open {out}"
            ),
            None => format!("job_wait (id {job}) or job_status until it ends, then p2g_links {out}"),
        };
        ("writes", Some(job), note)
    } else {
        cx.run_making(cmd, made, summary)?;
        (
            "written",
            None,
            format!("p2g_links {out} or view_open {out} to read it"),
        )
    };
    let mut reply = json!({
        "command": format!("chickpea {}", command_line(command)),
        "note": note,
    });
    reply[key] = made.iter().map(|p| p.to_string_lossy()).collect();
    if let Some(job) = job {
        reply["job"] = job.into();
    }
    if let Some(s) = session {
        reply["session"] = s.into();
    }
    Ok(reply)
}

/// A short name for a groups file, for a default out: its file name
/// without the extensions a labels table or a round has (`X.L1.json` is
/// `X.L1`, `X.L1.argmax.tsv.gz` too).
fn groups_name(path: &str) -> String {
    let mut name = Path::new(path)
        .file_name()
        .map_or_else(|| "groups".into(), |n| n.to_string_lossy().into_owned());
    loop {
        let before = name.len();
        for ext in [
            ".gz", ".json", ".tsv", ".csv", ".txt", ".parquet", ".argmax",
        ] {
            if let Some(n) = name.strip_suffix(ext) {
                name = n.to_string();
            }
        }
        if name.len() == before {
            break;
        }
    }
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() {
        "groups".into()
    } else {
        name
    }
}

/// Run `chickpea {command}` in the background: on `chickpea run`'s screen
/// in a detached session, which a person can attach to and which outlives
/// this server; where there are no sessions, as a job of this server's
/// (`record`, the step itself). Its job, and its session if it has one.
fn background(
    cx: &Call,
    out: &str,
    command: &[String],
    record: std::process::Command,
    made: &[PathBuf],
    summary: &str,
) -> anyhow::Result<(String, Option<String>)> {
    #[cfg(all(unix, feature = "tui"))]
    {
        let mut watch = legume_mcp::own_command(["run", "--"])?;
        watch.args(command);
        let label = Path::new(out)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let started = cx.start_session_job(legume_mcp::SessionJob {
            tool: crate::tui::TOOL,
            screen: "run".into(),
            label,
            watch,
            record,
            outputs: made.to_vec(),
            summary: summary.into(),
        })?;
        Ok((started.job, Some(started.session)))
    }
    #[cfg(not(all(unix, feature = "tui")))]
    {
        let _ = (out, command);
        Ok((cx.spawn_job(record, made, summary)?, None))
    }
}

/// Keep the `top` best of `rows` by `scores`, highest first and undefined
/// (NaN) last.
pub fn best_first(scores: &[f64], rows: &mut Vec<usize>, top: usize) {
    let key = |i: usize| match scores[i] {
        s if s.is_nan() => f64::NEG_INFINITY,
        s => s,
    };
    let order = |&i: &usize, &j: &usize| key(j).total_cmp(&key(i));
    if rows.len() > top {
        rows.select_nth_unstable_by(top, order);
        rows.truncate(top);
    }
    rows.sort_by(order);
}

/// A run's links, the best first: all genes', or those of `genes`.
fn links(a: LinksArgs) -> anyhow::Result<Value> {
    // A regrouped run's links are those of the run it regrouped.
    let prefix = crate::p2g::workflow::prefix_of(&shown_path(Path::new(&a.out)));
    let file = table(&crate::p2g::workflow::trained_run(&prefix)?, LINKS);
    let file = shown_path(&crate::p2g::workflow::tidy(Path::new(&file)));
    let LinkColumns {
        gene,
        peak,
        distance,
        abc,
        gate,
        corr,
        score,
    } = read_links(&file)?;
    let wanted: FxHashSet<&str> = a.genes.iter().map(String::as_str).collect();
    let mut rows: Vec<usize> = (0..gene.len())
        .filter(|&i| wanted.is_empty() || wanted.contains(&*gene[i]))
        .collect();
    let matched = rows.len();
    let linked: FxHashSet<&str> = if wanted.is_empty() {
        FxHashSet::default()
    } else {
        rows.iter().map(|&i| &*gene[i]).collect()
    };
    let missing: Vec<&String> = a
        .genes
        .iter()
        .filter(|g| !linked.contains(g.as_str()))
        .collect();
    best_first(&score, &mut rows, a.top);
    let links: Vec<Value> = rows
        .into_iter()
        .map(|i| {
            json!({
                "gene": gene[i],
                "peak": peak[i],
                "distance": distance[i] as i64,
                "abc": abc[i],
                "gate": gate[i],
                "corr": corr[i],
                "score": score[i],
            })
        })
        .collect();
    let mut reply = json!({
        "file": file,
        "pairs": gene.len(),
        "matched": matched,
        "links": links,
    });
    if !missing.is_empty() {
        reply["no_links"] = json!(missing);
    }
    Ok(reply)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArgs {
    data: Option<Paths>,
    rna: Option<Paths>,
    atac: Option<Paths>,
    gene_coords: Option<String>,
    gff_file: Option<String>,
    batch: Option<String>,
    groups: Option<String>,
    out: Option<String>,
    #[serde(default)]
    flags: Map<String, Value>,
    #[serde(default = "in_background")]
    background: bool,
}

/// `flag` and `paths`, each from the project's root and refused unless it
/// exists: the records' replay of a run reads the same files wherever it
/// starts.
fn push_paths(argv: &mut Vec<String>, flag: &str, paths: &[String]) -> anyhow::Result<()> {
    argv.push(flag.into());
    for path in paths {
        let path = shown_path(Path::new(path));
        anyhow::ensure!(Path::new(&path).exists(), "{flag} {path} does not exist");
        argv.push(path);
    }
    Ok(())
}

/// Paths, one per sample: a list, or one path as a string.
#[derive(Deserialize, Default)]
#[serde(from = "OneOrMany")]
struct Paths(Vec<String>);

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl From<OneOrMany> for Paths {
    fn from(v: OneOrMany) -> Self {
        match v {
            OneOrMany::One(p) => Self(vec![p]),
            OneOrMany::Many(ps) => Self(ps),
        }
    }
}

/// A schema for [`Paths`].
fn paths(about: &str) -> Value {
    json!({
        "type": ["array", "string"],
        "items": {"type": "string"},
        "description": about,
    })
}

fn in_background() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegroupCall {
    run: String,
    #[serde(default)]
    atac: Paths,
    groups: String,
    out: Option<String>,
    #[serde(default)]
    background: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinksArgs {
    out: String,
    #[serde(default)]
    genes: Vec<String>,
    #[serde(default = "top_links")]
    top: usize,
}

fn top_links() -> usize {
    20
}

fn specs() -> Vec<ToolSpec> {
    let path = |what: &str| json!({ "type": "string", "description": what });
    vec![
        ToolSpec::new(
            "p2g_flags",
            "peak-to-gene's flags, read from chickpea's command line: name, help, default, \
             choices, and whether it is a switch. The inputs and out are p2g_run's own arguments.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        ToolSpec::new(
            "p2g_run",
            "Link cis peaks to genes: chickpea peak-to-gene on paired RNA and ATAC counts, \
             checked as chickpea parses it, then run as a child chickpea and kept in the \
             project's records. A run takes minutes to hours, so it runs as a background job: \
             the call returns the job at once, to follow with job_status / job_wait / job_cancel \
             (background: false waits for it, for a small data set). A run is never written \
             over. When it ends, p2g_links reads its links.",
            json!({
                "type": "object",
                "properties": {
                    "data": paths("Count files in any order, in place of rna and atac: each told RNA, ATAC or both (genes and peaks as rows of one file, a 10x multiome matrix) by its rows; RNA and ATAC files paired by the words of their names (s1_gex.h5 with s1_atac.h5), else in the order given"),
                    "rna": paths("RNA gene counts (.zarr, .zarr.zip, .h5), one file per sample (a list, or a path for one sample); with several, cells are {barcode}@{sample}, the sample named after its files"),
                    "atac": paths("ATAC peak counts for the same barcodes, one file per sample in rna's order; peaks named chr:start-end"),
                    "gene_coords": path("Gene TSS table: gene, chr, tss, with a header (this or gff_file)"),
                    "gff_file": path("GFF/GTF annotation for gene TSS (this or gene_coords)"),
                    "batch": path("Batch labels, one per cell [default: each sample a batch]"),
                    "groups": path("Cell groups for the per-group tables, in place of the Leiden clusters: a lupin round's manifest (.json), its argmax table, or a cell, group table; unassigned cells are in no group"),
                    "out": path("Output prefix: {out}.links.parquet and the rest [default: results/<today>/p2g]"),
                    "flags": {"type": "object", "description": "peak-to-gene's flags by name, as p2g_flags lists them (a switch takes true or false; null leaves a flag at its default): {\"cis-window\": 250000}"},
                    "background": {"type": "boolean", "description": "Run as a background job and return at once [default: true]; false waits for the run"},
                },
                "required": [],
                "additionalProperties": false,
            }),
        ),
        ToolSpec::new(
            "p2g_regroup",
            "Redo a finished run's per-group tables for other cell groups (a lupin round's \
             cell types, say) without training it again: a group's link shares need only the \
             trained gates and the peaks' accessibility in its cells. Writes {out}.groups, \
             cell_clusters, peak_rates and links_by_cluster, and {out}.regroup.json naming the run \
             whose links it uses, so out opens as a run of its own (view_open, p2g_links). \
             It reads each sample's ATAC counts once, from the files the run's record names: \
             minutes; waits unless background.",
            json!({
                "type": "object",
                "properties": {
                    "run": path("The finished run's out prefix, or one of its tables"),
                    "atac": paths("ATAC peak counts in place of the files the run's record ({run}.run.json, or a regrouped run's {run}.regroup.json) names, for a sample whose file has moved: each stands in for the sample its file name names (a list, or a path). A run without a record needs every sample's, in peak-to-gene's order"),
                    "groups": path("The new groups: a lupin round's manifest (.json, read for annotate.argmax), its argmax table, or a cell, group table"),
                    "out": path("Output prefix for the regrouped run [default: results/<today>/regroup.{groups' name}]"),
                    "background": {"type": "boolean", "description": "Run as a background job and return at once [default: false]"},
                },
                "required": ["run", "groups"],
                "additionalProperties": false,
            }),
        ),
        ToolSpec::new(
            "p2g_links",
            "A finished run's peak-to-gene links, highest score first: gene, peak, distance \
             (bp, peak midpoint to TSS), abc (contact prior share), gate (the gene's share of \
             the pair), corr (the correlation across pseudobulks of the gene's score and its peak's \
             ATAC module's, so peaks of one module share it) and score (gate × corr). With genes, \
             only theirs. Also the number of pairs in the run and of those matched.",
            json!({
                "type": "object",
                "properties": {
                    "out": path("The run's out prefix, or its .links.parquet"),
                    "genes": {"type": "array", "items": {"type": "string"}, "description": "Only these genes' links"},
                    "top": {"type": "integer", "minimum": 1, "description": "Links to return [default: 20]"},
                },
                "required": ["out"],
                "additionalProperties": false,
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_groups_file_names_its_regroup() {
        assert_eq!(groups_name("lupin/X.L1.json"), "X.L1");
        assert_eq!(groups_name("X.L1.argmax.tsv.gz"), "X.L1");
        assert_eq!(groups_name("my groups.csv"), "my_groups");
    }
}
