//! `chickpea mcp`: chickpea as a Model Context Protocol server. Its tools
//! list peak-to-gene's flags, run it as a child `chickpea` (a background
//! job unless asked otherwise, since a run takes minutes to hours) in the
//! project's records, and read the links a run wrote.

use crate::p2g::workflow::{table, tables, LINKS};
use crate::p2g::PeakToGeneArgs;
use clap::{ArgAction, Args, FromArgMatches};
use legume_mcp::{Call, Content, Handler, ToolSpec};
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

/// The run's own arguments: given by name to `p2g_run`, never as flags.
const NAMED: [&str; 6] = ["rna", "atac", "gene-coords", "gff-file", "batch", "out"];

/// The tools, and peak-to-gene's command line to read the flags from and
/// check a run against.
pub struct Server {
    cli: clap::Command,
}

impl Default for Server {
    fn default() -> Self {
        let mut cli = PeakToGeneArgs::augment_args(clap::Command::new("peak-to-gene"));
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
            "p2g_run" => self.run(parse(args)?, cx)?,
            "p2g_links" => links(parse(args)?)?,
            _ => unreachable!("the server answers unknown tools itself"),
        };
        Ok(vec![Content::Json(reply)])
    }
}

fn is_switch(arg: &clap::Arg) -> bool {
    matches!(arg.get_action(), ArgAction::SetTrue | ArgAction::SetFalse)
}

impl Server {
    /// The flags a run takes, without the run's own arguments; hidden ones
    /// are taken but not listed.
    fn flag_args(&self) -> impl Iterator<Item = (&str, &clap::Arg)> {
        self.cli.get_arguments().filter_map(|a| {
            let long = a.get_long()?;
            (long != "help" && !NAMED.contains(&long)).then_some((long, a))
        })
    }

    fn flags(&self) -> Value {
        let flags: Vec<Value> = self
            .flag_args()
            .filter(|(_, a)| !a.is_hide_set())
            .map(|(long, a)| {
                let mut flag = json!({
                    "flag": format!("--{long}"),
                    "help": a.get_help().map(|h| h.to_string()),
                });
                if is_switch(a) {
                    flag["switch"] = true.into();
                } else {
                    let default: Vec<_> = a
                        .get_default_values()
                        .iter()
                        .map(|v| v.to_string_lossy())
                        .collect();
                    if !default.is_empty() {
                        flag["default"] = default.join(",").into();
                    }
                    let choices: Vec<String> = a
                        .get_possible_values()
                        .iter()
                        .filter(|v| !v.is_hide_set())
                        .map(|v| v.get_name().to_string())
                        .collect();
                    if !choices.is_empty() {
                        flag["choices"] = choices.into();
                    }
                }
                flag
            })
            .collect();
        json!({ "command": "chickpea peak-to-gene", "flags": flags })
    }

    /// `flags`, as p2g_run takes them, on peak-to-gene's command line.
    fn flag_argv(&self, flags: &Map<String, Value>) -> anyhow::Result<Vec<String>> {
        let mut argv: Vec<String> = Vec::new();
        for (flag, value) in flags {
            let long = flag.trim_start_matches('-').replace('_', "-");
            anyhow::ensure!(
                !NAMED.contains(&long.as_str()),
                "--{long} is p2g_run's own argument {}, not a flag",
                long.replace('-', "_")
            );
            let (_, arg) = self.flag_args().find(|(l, _)| *l == long).ok_or_else(|| {
                anyhow::anyhow!("peak-to-gene has no flag --{long}: p2g_flags lists them")
            })?;
            if value.is_null() {
                continue; // Null leaves the flag at its default.
            }
            if is_switch(arg) {
                match value {
                    Value::Bool(true) => argv.push(format!("--{long}")),
                    Value::Bool(false) => {}
                    v => anyhow::bail!("--{long} is a switch: true or false, not {v}"),
                }
            } else {
                let text = match value {
                    Value::Array(vs) => vs.iter().map(text).collect::<Vec<_>>().join(","),
                    v => text(v),
                };
                // `=`: a value may start with `-`, and some flags take no other form.
                argv.push(format!("--{long}={text}"));
            }
        }
        Ok(argv)
    }

    fn run(&self, a: RunArgs, cx: &Call) -> anyhow::Result<Value> {
        // The flags first, so a misplaced input is named as such.
        let flags = self.flag_argv(&a.flags)?;
        let positions = match (&a.gene_coords, &a.gff_file) {
            (Some(tsv), None) => ("--gene-coords", tsv),
            (None, Some(gff)) => ("--gff-file", gff),
            _ => anyhow::bail!("give one of gene_coords (a gene, chr, tss TSV) or gff_file"),
        };
        // Paths from the root: the records' replay of the run reads the
        // same files wherever it starts.
        let mut argv: Vec<String> = vec!["peak-to-gene".into()];
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
            _ => free_default(cx)?,
        };
        argv.extend(["--out".into(), out.clone()]);
        argv.extend(flags);
        let matches = self.cli.clone().try_get_matches_from(&argv).map_err(|e| {
            let msg = e.to_string();
            let first = msg.lines().next().unwrap_or_default();
            anyhow::anyhow!("chickpea {}", first.trim_start_matches("error: "))
        })?;
        let made: Vec<PathBuf> = PeakToGeneArgs::from_arg_matches(&matches)?
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
        let mut cmd = legume_mcp::own_command(&argv)?;
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
            "command": format!("chickpea {}", argv.join(" ")),
            "note": note,
        });
        reply[key] = made.iter().map(|p| p.to_string_lossy()).collect();
        if let Some(job) = job {
            reply["job"] = job.into();
        }
        Ok(reply)
    }
}

/// The first of `p2g`, `p2g-2`, ... in today's results folder that no run
/// has written and no job is writing.
fn free_default(cx: &Call) -> anyhow::Result<String> {
    let dir = cx
        .results_dir()
        .ok_or_else(|| anyhow::anyhow!("give out, the output prefix"))?;
    let free = (1..)
        .map(|k| match k {
            1 => "p2g".to_string(),
            k => format!("p2g-{k}"),
        })
        .map(|name| shown_path(&dir.join(name)))
        .find(|out| {
            let made = tables(out);
            made.iter().all(|p| !p.exists()) && cx.ensure_free(&made).is_ok()
        });
    Ok(free.expect("an unused name"))
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

/// The arguments of a call, or what is wrong with them.
fn parse<T: serde::de::DeserializeOwned>(args: Value) -> anyhow::Result<T> {
    serde_json::from_value(args).map_err(|e| anyhow::anyhow!("arguments: {e}"))
}

/// `p` as results give paths: from the root.
fn shown_path(p: &Path) -> String {
    std::path::absolute(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// A flag's value as typed on the command line.
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
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
