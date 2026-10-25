//! `chickpea mcp`: chickpea as a Model Context Protocol server. Its tools
//! list peak-to-gene's flags, run it as a child `chickpea` (a background
//! job unless asked otherwise, since a run takes minutes to hours) in the
//! project's records, and read the links a run wrote.

use crate::p2g::workflow::{table, LINKS};
use crate::p2g::{PeakToGeneArgs, RegroupArgs};
use anyhow::Context;
use clap::{Args, FromArgMatches};
use legume_mcp::cli::{check, describe, settable, to_argv};
use legume_mcp::{command_line, parse_args, shown_path, Call, Content, Handler, ToolSpec};
use legume_numeric::matrix::parquet::read_table_columns;
use rustc_hash::FxHashSet;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

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
    #[cfg(feature = "tui")]
    viewer: crate::view::mcp::Viewer,
}

impl Default for Server {
    fn default() -> Self {
        let mut cli = PeakToGeneArgs::augment_args(clap::Command::new("chickpea peak-to-gene"));
        cli.build();
        let mut regroup = RegroupArgs::augment_args(clap::Command::new("chickpea regroup"));
        regroup.build();
        Server {
            cli,
            regroup,
            #[cfg(feature = "tui")]
            viewer: Default::default(),
        }
    }
}

impl Handler for Server {
    fn tools(&self) -> Vec<ToolSpec> {
        #[cfg(feature = "tui")]
        return [specs(), crate::view::mcp::specs()].concat();
        #[cfg(not(feature = "tui"))]
        specs()
    }

    fn call(&mut self, name: &str, args: Value, cx: &Call) -> anyhow::Result<Vec<Content>> {
        #[cfg(feature = "tui")]
        if crate::view::mcp::Viewer::answers(name) {
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
        let flags = to_argv(&self.cli, &a.flags, &OWN.map(|id| (id, id)), "p2g_flags")?;
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
        let out = match a.out.as_deref().map(str::trim) {
            Some(out) if !out.is_empty() => shown_path(Path::new(out)),
            _ => {
                cx.free_prefix("p2g", |out| Ok(PeakToGeneArgs::outputs_of(out)), |_| false)
                    .context("give out, the output prefix")?
                    .0
            }
        };
        argv.extend(["--out".into(), out.clone()]);
        argv.extend(flags);
        let parsed = PeakToGeneArgs::from_arg_matches(&check(&self.cli, &argv)?)?;
        parsed.check_pairs()?;
        let made: Vec<PathBuf> = parsed
            .outputs()
            .iter()
            .map(|p| shown_path(p).into())
            .collect();
        if let Some(there) = made.iter().find(|p| p.exists()) {
            anyhow::bail!(
                "{} exists: a run is not written over; give another out or path",
                there.display()
            );
        }

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
        let out = match a.out.as_deref().map(str::trim) {
            Some(out) if !out.is_empty() => shown_path(Path::new(out)),
            _ => {
                let made = |out: &str| Ok(RegroupArgs::outputs_of(out));
                let name = format!("regroup.{}", groups_name(&a.groups));
                cx.free_prefix(&name, made, |_| false)
                    .context("give out, the output prefix")?
                    .0
            }
        };
        argv.extend(["--out".into(), out.clone()]);
        let made: Vec<PathBuf> = RegroupArgs::from_arg_matches(&check(&self.regroup, &argv)?)?
            .outputs()
            .iter()
            .map(|p| shown_path(p).into())
            .collect();
        if let Some(there) = made.iter().find(|p| p.exists()) {
            anyhow::bail!(
                "{} exists: a run is not written over; give another out",
                there.display()
            );
        }
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
    let (s, n) = read_table_columns(
        &file,
        &["gene", "peak"],
        &["distance", "abc", "gate", "corr", "score"],
    )?;
    let ([gene, peak], [distance, abc, gate, corr, score]) = (&s[..], &n[..]) else {
        unreachable!("the columns asked for")
    };
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
    best_first(score, &mut rows, a.top);
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
