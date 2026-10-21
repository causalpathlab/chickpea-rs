//! `chickpea mcp`: chickpea as a Model Context Protocol server. Its tools
//! list peak-to-gene's flags, run it as a child `chickpea` (a background
//! job unless asked otherwise, since a run takes minutes to hours) in the
//! project's records, and read the links a run wrote.

use crate::p2g::workflow::{table, tables, LINKS};
use crate::p2g::PeakToGeneArgs;
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
choices). `p2g_run` runs it on an RNA and an ATAC count file for the same \
barcodes, with gene positions from gene_coords (a gene, chr, tss TSV) or \
gff_file: checked as chickpea parses it, in the background (follow its job \
with `job_status` / `job_wait`), and never over a run. When it is done, \
`p2g_links` reads the run's links (gene, peak, distance, abc, gate, corr, \
score), the best first, for all genes or the ones named.";

/// The run's own arguments, by clap id, which is also how `p2g_run` names
/// them: given by name, never as flags.
const OWN: [&str; 6] = ["rna", "atac", "gene_coords", "gff_file", "batch", "out"];

/// The tools, and peak-to-gene's command line to read the flags from and
/// check a run against.
pub struct Server {
    cli: clap::Command,
}

impl Default for Server {
    fn default() -> Self {
        let mut cli = PeakToGeneArgs::augment_args(clap::Command::new("chickpea peak-to-gene"));
        cli.build();
        Server { cli }
    }
}

impl Handler for Server {
    fn tools(&self) -> Vec<ToolSpec> {
        specs()
    }

    fn call(&mut self, name: &str, args: Value, cx: &Call) -> anyhow::Result<Vec<Content>> {
        let reply = match name {
            "p2g_flags" => self.flags(),
            "p2g_run" => self.run(parse_args(args)?, cx)?,
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
        // Paths from the root: the records' replay of the run reads the
        // same files wherever it starts.
        let mut argv: Vec<String> = Vec::new();
        let inputs = [("--rna", &a.rna), ("--atac", &a.atac), positions];
        for (flag, path) in inputs
            .into_iter()
            .chain(a.batch.as_ref().map(|b| ("--batch", b)))
        {
            let path = shown_path(Path::new(path));
            anyhow::ensure!(Path::new(&path).exists(), "{flag} {path} does not exist");
            argv.extend([flag.into(), path]);
        }
        let out = match a.out.as_deref().map(str::trim) {
            Some(out) if !out.is_empty() => shown_path(Path::new(out)),
            _ => {
                cx.free_prefix("p2g", |out| Ok(tables(out)), |_| false)
                    .context("give out, the output prefix")?
                    .0
            }
        };
        argv.extend(["--out".into(), out.clone()]);
        argv.extend(flags);
        let made: Vec<PathBuf> = PeakToGeneArgs::from_arg_matches(&check(&self.cli, &argv)?)?
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

        let summary = format!("peak-to-gene links of {} and {}", a.rna, a.atac);
        let command = [String::from("peak-to-gene")]
            .into_iter()
            .chain(argv)
            .collect::<Vec<_>>();
        let mut cmd = legume_mcp::own_command(&command)?;
        // Info logs, so a run's progress reaches job_status.
        if std::env::var_os("RUST_LOG").is_none() {
            cmd.env("RUST_LOG", "info");
        }
        let (key, job, note) = if a.background {
            let job = cx.spawn_job(cmd, &made, &summary)?;
            let note =
                format!("job_wait (id {job}) or job_status until it ends, then p2g_links {out}");
            ("writes", Some(job), note)
        } else {
            cx.run_making(cmd, &made, &summary)?;
            (
                "written",
                None,
                format!("p2g_links {out} to read the links"),
            )
        };
        let mut reply = json!({
            "command": format!("chickpea {}", command_line(&command)),
            "note": note,
        });
        reply[key] = made.iter().map(|p| p.to_string_lossy()).collect();
        if let Some(job) = job {
            reply["job"] = job.into();
        }
        Ok(reply)
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
    let file = if a.out.ends_with(&format!(".{LINKS}.parquet")) {
        a.out.clone()
    } else {
        table(&a.out, LINKS)
    };
    let file = shown_path(Path::new(&file));
    anyhow::ensure!(
        Path::new(&file).exists(),
        "{file} does not exist: give the out of a finished p2g_run"
    );
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
    rna: String,
    atac: String,
    gene_coords: Option<String>,
    gff_file: Option<String>,
    batch: Option<String>,
    out: Option<String>,
    #[serde(default)]
    flags: Map<String, Value>,
    #[serde(default = "in_background")]
    background: bool,
}

fn in_background() -> bool {
    true
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
                    "rna": path("RNA gene counts (.zarr, .zarr.zip, .h5)"),
                    "atac": path("ATAC peak counts for the same barcodes; peaks named chr:start-end"),
                    "gene_coords": path("Gene TSS table: gene, chr, tss, with a header (this or gff_file)"),
                    "gff_file": path("GFF/GTF annotation for gene TSS (this or gene_coords)"),
                    "batch": path("Batch labels, one per barcode"),
                    "out": path("Output prefix: {out}.links.parquet and the rest [default: results/<today>/p2g]"),
                    "flags": {"type": "object", "description": "peak-to-gene's flags by name, as p2g_flags lists them (a switch takes true or false; null leaves a flag at its default): {\"cis-window\": 250000}"},
                    "background": {"type": "boolean", "description": "Run as a background job and return at once [default: true]; false waits for the run"},
                },
                "required": ["rna", "atac"],
                "additionalProperties": false,
            }),
        ),
        ToolSpec::new(
            "p2g_links",
            "A finished run's peak-to-gene links, highest score first: gene, peak, distance \
             (bp, peak midpoint to TSS), abc (contact prior share), gate (the gene's share of \
             the pair), corr (evidence across pseudobulks) and score (gate × corr). With genes, \
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
