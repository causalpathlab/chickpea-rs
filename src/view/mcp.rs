//! The viewer's tools for `chickpea mcp`: open a run, then look at it
//! genome-wide or at a gene's locus. What is looked at stays set between
//! calls, as on `chickpea view`, so a call passes only what changes. The
//! argument names mean what they mean in pinto's and senna's view tools.

use super::data::{Measure, Run, Summary};
use super::State;
use legume_mcp::{shown_path, Call, Content, ToolSpec};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

/// Links a locus lists, and genes the genome-wide view lists, by default.
const DEFAULT_N: usize = 20;

/// The viewer, as the server holds it between calls.
#[derive(Default)]
pub struct Viewer {
    state: Option<State>,
}

impl Viewer {
    /// Answer `name`, one of [`specs`]'s tools.
    pub fn call(&mut self, name: &str, args: Value, cx: &Call) -> anyhow::Result<Vec<Content>> {
        let json = |v: anyhow::Result<Value>| Ok(vec![Content::Json(v?)]);
        match name {
            "view_open" => json(self.open(legume_mcp::parse_args(args)?)),
            "view_genome" => self.genome(legume_mcp::parse_args(args)?),
            "view_locus" => self.locus(legume_mcp::parse_args(args)?),
            "view_export" => json(self.export(legume_mcp::parse_args(args)?, cx)),
            _ => unreachable!("only the view tools reach the viewer"),
        }
    }

    /// Whether `name` is one of the viewer's tools.
    #[must_use]
    pub fn answers(name: &str) -> bool {
        name.starts_with("view_")
    }

    fn state(&mut self) -> anyhow::Result<&mut State> {
        self.state
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("no run open: view_open one first"))
    }

    fn open(&mut self, a: OpenArgs) -> anyhow::Result<Value> {
        let path = shown_path(Path::new(&a.prefix));
        let run = Run::open(&path)?;
        let mut reply = run.describe();
        self.state = Some(State::new(run));
        reply["next"] =
            "view_genome ranks the genes by their links; view_locus {gene} shows one gene's".into();
        Ok(reply)
    }

    fn genome(&mut self, a: GenomeArgs) -> anyhow::Result<Vec<Content>> {
        let state = self.state()?;
        set(
            state,
            a.measure.as_deref(),
            a.summary.as_deref(),
            a.count_cutoff,
        )?;
        if let Some(c) = &a.chr {
            state.show_chr(c)?;
        }
        if let Some(g) = &a.gene {
            state.set_gene(g)?;
        }
        state.genome_view.focus = state.gene;
        let genome = state.genome();
        let run = &state.run;
        let (from, span) = state.genome_view.window();
        let len = genome.len().max(1) as f32;
        let in_sight: rustc_hash::FxHashSet<usize> = genome
            .points
            .iter()
            .filter(|p| {
                let t = p.1 as f32 / len;
                t >= from && t <= from + span
            })
            .map(|p| p.0)
            .collect();
        let ranked = run.ranked(state.measure, state.summary);
        let genes: Vec<Value> = ranked
            .iter()
            .filter(|&&(g, v)| in_sight.contains(&g) && v > 0.0)
            .take(a.n.unwrap_or(DEFAULT_N))
            .map(|&(g, v)| {
                let mut row = json!({
                    "gene": run.genes[g],
                    "value": finite(v),
                    "candidates": run.n_candidates(g),
                });
                if let Some(t) = &run.tss[g] {
                    row["chr"] = t.chr.to_string().into();
                    row["tss"] = t.tss.into();
                }
                row
            })
            .collect();
        let reply = json!({
            "run": run.prefix,
            "measure": state.measure.name(),
            "summary": summary_name(state.summary),
            "shown": shown_part(&genome, from, span),
            "genes": genes,
            "settings": settings(state),
            "next": "view_locus {gene} draws one gene's links; view_export {what: genome, pdf} writes this figure into the records",
        });
        let figure = match a.format.as_deref() {
            Some("text") => {
                let cols = a.width.unwrap_or(120).clamp(40, 400) as u16;
                Content::Text(super::text::genome_to_string(
                    &genome,
                    &run.genes,
                    &state.genome_view,
                    cols,
                    24,
                ))
            }
            None | Some("png") => {
                let w = a.width.unwrap_or(1200).clamp(200, MAX_WIDTH);
                Content::Png(state.png(&super::Figure::Genome(genome), w)?)
            }
            Some(other) => anyhow::bail!("format {other:?}: png or text"),
        };
        Ok(vec![Content::Json(reply), figure])
    }

    fn locus(&mut self, a: LocusArgs) -> anyhow::Result<Vec<Content>> {
        let state = self.state()?;
        set(state, a.measure.as_deref(), None, None)?;
        if let Some(g) = &a.gene {
            state.set_gene(g)?;
        }
        if let Some(h) = a.half_window {
            state.half_window = (h > 0).then_some(h);
        }
        if let Some(c) = a.cutoff {
            state.style.cutoff = c;
        }
        if let Some(m) = a.marks {
            state.style.marks = m;
        }
        if let Some(r) = a.rows {
            state.style.rows = r
                .iter()
                .map(|v| match v {
                    Value::String(s) => state.run.row_of(s),
                    other => state.run.row_of(&other.to_string()),
                })
                .collect::<anyhow::Result<_>>()?;
        }
        let g = state
            .gene
            .ok_or_else(|| anyhow::anyhow!("give a gene: view_locus {{\"gene\": \"...\"}}"))?;
        let run = &state.run;
        let m = state.measure;
        let mut links = run.links(g);
        links.sort_by(|a, b| b.get(m).total_cmp(&a.get(m)));
        let n = links.len();
        let rows: Vec<Value> = links
            .into_iter()
            .take(a.n.unwrap_or(DEFAULT_N))
            .map(|l| {
                json!({
                    "peak": l.peak,
                    "distance": l.distance,
                    "score": finite(l.score),
                    "gate": finite(l.gate),
                    "corr": finite(l.corr),
                    "abc": finite(l.abc),
                })
            })
            .collect();
        let mut reply = json!({
            "gene": run.genes[g],
            "measure": m.name(),
            "candidates": n,
            "links": rows,
            "settings": settings(state),
        });
        let mut out = Vec::new();
        match state.locus() {
            Some(locus) => {
                reply["chr"] = locus.tss.chr.to_string().into();
                reply["tss"] = locus.tss.tss.into();
                reply["window"] = json!([locus.lo, locus.hi]);
                reply["rows"] = state
                    .style
                    .rows_of(&locus)
                    .into_iter()
                    .map(|i| {
                        let r = &locus.rows[i];
                        json!({"row": i, "name": run.row_name(i), "label": r.label, "cells": r.cells})
                    })
                    .collect();
                if locus.tss.inferred {
                    reply["tss_inferred"] = true.into();
                }
                if !run.has_rates() {
                    reply["note"] =
                        "this run kept no peak_rates: one row, the run's own values".into();
                }
                if a.format.as_deref() == Some("text") {
                    let cols = a.width.unwrap_or(120).clamp(40, 400) as u16;
                    reply["next"] = "view_locus {format: png} draws it as a picture".into();
                    out.push(Content::Json(reply));
                    out.push(Content::Text(super::text::to_string(
                        &locus,
                        &state.style,
                        cols,
                    )));
                    return Ok(out);
                }
                anyhow::ensure!(
                    a.format.as_deref().is_none_or(|f| f == "png"),
                    "format: png or text"
                );
                let w = a.width.unwrap_or(1200).clamp(200, MAX_WIDTH);
                let png = state.png(&super::Figure::Locus(locus), w)?;
                reply["next"] =
                    "view_export {pdf} writes this figure into the records; view_locus changes it"
                        .into();
                out.push(Content::Json(reply));
                out.push(Content::Png(png));
            }
            None => {
                reply["note"] = "the gene has no place on the genome: no figure".into();
                out.push(Content::Json(reply));
            }
        }
        Ok(out)
    }

    /// The locus figure as it is set, written by `chickpea view` as a step
    /// of the records, so the driver script draws it again.
    fn export(&mut self, a: ExportArgs, cx: &Call) -> anyhow::Result<Value> {
        let state = self.state()?;
        anyhow::ensure!(a.pdf.is_some() || a.png.is_some(), "give pdf, png or both");
        let genome = match a.what.as_deref() {
            None | Some("locus") => false,
            Some("genome") => true,
            Some(other) => anyhow::bail!("what {other:?}: locus or genome"),
        };
        let mut argv: Vec<String> = vec![
            "view".into(),
            state.run.prefix.clone(),
            "--measure".into(),
            state.measure.name().into(),
        ];
        if let Some(g) = state.gene {
            argv.extend(["--gene".into(), state.run.genes[g].to_string()]);
        }
        if genome {
            argv.push("--genome".into());
            if let Summary::Count(c) = state.summary {
                argv.extend([
                    "--summary".into(),
                    "count".into(),
                    "--count-cutoff".into(),
                    c.to_string(),
                ]);
            }
            let (from, span) = state.genome_view.window();
            if span < 0.999 {
                // Only a whole chromosome can be named on the line.
                let g = state.genome();
                let name = g.whole_chrom(from, span).ok_or_else(|| {
                    anyhow::anyhow!("the genome-wide view is zoomed to a part that is not one chromosome: view_genome {{chr}} or {{chr: \"\"}} first")
                })?;
                argv.extend(["--chr".into(), name.to_string()]);
            }
        } else {
            anyhow::ensure!(state.gene.is_some(), "no gene shown: view_locus one first");
        }
        argv.extend(["--cutoff".into(), state.style.cutoff.to_string()]);
        if let Some(h) = state.half_window {
            argv.extend(["--half-window".into(), h.to_string()]);
        }
        for m in &state.style.marks {
            argv.extend(["--mark".into(), m.to_string()]);
        }
        if !state.style.rows.is_empty() {
            // By name, unless a name would not survive the comma list.
            let names: Vec<String> = state
                .style
                .rows
                .iter()
                .map(|&r| state.run.row_name(r))
                .collect();
            let rows: Vec<String> = if names.iter().any(|n| n.contains(',')) {
                state.style.rows.iter().map(usize::to_string).collect()
            } else {
                names
            };
            argv.extend(["--rows".into(), rows.join(",")]);
        }
        let mut made = Vec::new();
        for (flag, path) in [("--pdf", &a.pdf), ("--png", &a.png)] {
            if let Some(p) = path {
                let p = shown_path(Path::new(p));
                argv.extend([flag.into(), p.clone()]);
                made.push(std::path::PathBuf::from(p));
            }
        }
        if let Some(w) = a.width_in {
            argv.extend(["--width-in".into(), w.to_string()]);
        }
        if let Some(d) = a.dpi {
            argv.extend(["--dpi".into(), d.to_string()]);
        }
        let summary = match (genome, state.gene) {
            (true, _) => format!("genome-wide figure of {}", state.run.prefix),
            (false, Some(g)) => format!(
                "locus figure of {} in {}",
                state.run.genes[g], state.run.prefix
            ),
            (false, None) => unreachable!("checked above"),
        };
        cx.run_making(legume_mcp::own_command(&argv)?, &made, &summary)?;
        Ok(json!({
            "written": made.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
            "command": format!("chickpea {}", legume_mcp::command_line(&argv)),
            "settings": settings(state),
        }))
    }
}

/// The part of the genome a view shows, in words: the whole genome, a
/// chromosome, or a stretch from one place to another.
fn shown_part(genome: &super::data::Genome, from: f32, span: f32) -> String {
    if span >= 0.999 || genome.chroms.is_empty() {
        return "the whole genome".into();
    }
    if let Some(name) = genome.whole_chrom(from, span) {
        return format!("chromosome {name} ({:.1}% of the genome)", span * 100.0);
    }
    let len = genome.len().max(1) as f32;
    let at = |t: f32| {
        let pos = (t.clamp(0.0, 1.0) * len) as u64;
        let (name, off, _) = &genome.chroms[genome.chrom_at(pos)];
        format!("{name}:{}", super::figure::bp_text((pos - off) as f32))
    };
    format!(
        "{} to {} ({:.1}% of the genome)",
        at(from),
        at(from + span),
        span * 100.0
    )
}

/// The settings in effect, as every view_* reply echoes them: they carry
/// over from call to call, into view_export too.
fn settings(state: &State) -> Value {
    let rows: Value = if state.style.rows.is_empty() {
        "all of them".into()
    } else {
        state
            .style
            .rows
            .iter()
            .map(|&r| state.run.row_name(r))
            .collect()
    };
    json!({
        "measure": state.measure.name(),
        "summary": summary_name(state.summary),
        "gene": state.gene.map(|g| state.run.genes[g].to_string()),
        "half_window": state.half_window.map_or_else(|| Value::from("the farthest link"), Value::from),
        "cutoff": state.style.cutoff,
        "marks": state.style.marks,
        "rows": rows,
        "genome_view": shown_part(&state.genome(), state.genome_view.from, state.genome_view.span),
    })
}

/// Change what the call names, and keep the rest.
fn set(
    state: &mut State,
    measure: Option<&str>,
    summary: Option<&str>,
    cutoff: Option<f32>,
) -> anyhow::Result<()> {
    if let Some(m) = measure {
        state.measure = Measure::parse(m)?;
    }
    state.summary = match (summary, cutoff, state.summary) {
        (Some("best"), _, _) => Summary::Best,
        (Some("count"), c, Summary::Count(old)) => Summary::Count(c.unwrap_or(old)),
        (Some("count"), c, Summary::Best) => Summary::Count(c.unwrap_or(super::data::COUNT_CUTOFF)),
        (Some(other), _, _) => anyhow::bail!("summary {other:?}: best or count"),
        (None, Some(c), Summary::Count(_)) => Summary::Count(c),
        (None, _, now) => now,
    };
    Ok(())
}

fn summary_name(s: Summary) -> String {
    match s {
        Summary::Best => "best".into(),
        Summary::Count(c) => format!("count ≥ {c}"),
    }
}

/// A number JSON can carry: NaN as null.
fn finite(x: f32) -> Value {
    if x.is_finite() {
        json!(x)
    } else {
        Value::Null
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenArgs {
    prefix: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenomeArgs {
    measure: Option<String>,
    summary: Option<String>,
    count_cutoff: Option<f32>,
    chr: Option<String>,
    gene: Option<String>,
    n: Option<usize>,
    format: Option<String>,
    width: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocusArgs {
    gene: Option<String>,
    measure: Option<String>,
    half_window: Option<i64>,
    cutoff: Option<f32>,
    marks: Option<Vec<i64>>,
    rows: Option<Vec<Value>>,
    n: Option<usize>,
    width: Option<usize>,
    format: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportArgs {
    what: Option<String>,
    pdf: Option<String>,
    png: Option<String>,
    width_in: Option<f32>,
    dpi: Option<u32>,
}

/// The widest figure a call draws, in pixels.
const MAX_WIDTH: usize = 4096;

pub fn specs() -> Vec<ToolSpec> {
    let measure = json!({"enum": Measure::ALL.map(Measure::name), "description": "What a link is measured by: score (gate × corr), gate, corr or abc (the fixed contact prior) [default: score; stays set]"});
    vec![
        ToolSpec::new(
            "view_open",
            "Open a finished peak-to-gene run to look at, as chickpea view does: its genes, \
             pairs, placed peaks and genes, and cell groups (Leiden clusters C0, C1, …, or the \
             cell types it was given). The view_* tools then look at it.",
            json!({
                "type": "object",
                "properties": {
                    "prefix": {"type": "string", "description": "The run's out prefix, or one of its tables ({out}.links.parquet)"},
                },
                "required": ["prefix"],
                "additionalProperties": false,
            }),
        ),
        ToolSpec::new(
            "view_genome",
            "The open run genome-wide, as a figure and a table: chromosomes end to end, each \
             linked gene at its TSS by its links in one number (summary best: its best link; \
             count: its links at or above count_cutoff; a sum is not offered, as it grows with \
             a gene's number of candidates), the best named and the gene in focus ringed. The \
             table lists the genes in sight, highest first. Every setting stays set.",
            json!({
                "type": "object",
                "properties": {
                    "measure": measure,
                    "summary": {"enum": ["best", "count"], "description": "How a gene's links become one number [default: best]"},
                    "count_cutoff": {"type": "number", "description": "With summary count: the least value a link counts at [default: 0.1]"},
                    "chr": {"type": "string", "description": "Show only this chromosome; \"\" the whole genome again"},
                    "gene": {"type": "string", "description": "The gene to ring (also the one view_locus shows)"},
                    "n": {"type": "integer", "minimum": 1, "description": "How many genes to list [default: 20]"},
                    "format": {"enum": ["png", "text"], "description": "png: the figure as a picture [default]; text: in characters (braille dots), for a client that shows no images"},
                    "width": {"type": "integer", "minimum": 40, "maximum": 4096, "description": "This call only: image width in pixels [default: 1200], or text width in columns [default: 120]"},
                },
                "additionalProperties": false,
            }),
        ),
        ToolSpec::new(
            "view_locus",
            "One gene's locus in the open run, as a figure (PNG) and its numbers. The figure has \
             a row for all the cells and one per cell group: arcs from the TSS to the linked peaks, \
             darker where stronger, over each peak's accessibility in that row; dashed lines at \
             the TSS and at the marks; the window's genes and a scale at the foot. In a row a \
             link is its gate (or ABC prior) times its peak's accessibility there, over the \
             gene's candidates, times its corr for score. The numbers: the candidates, the best \
             by the measure first. Every setting stays set.",
            json!({
                "type": "object",
                "properties": {
                    "gene": {"type": "string", "description": "The gene, as the run names it"},
                    "measure": measure,
                    "half_window": {"type": "integer", "minimum": 0, "description": "bp each side of the TSS; 0: the gene's farthest link [default]"},
                    "cutoff": {"type": "number", "minimum": 0, "maximum": 1, "description": "Leave out links weaker than this share of the strongest shown [default: 0.05]"},
                    "marks": {"type": "array", "items": {"type": "integer"}, "description": "Positions (bp) marked with a dashed line; [] for none"},
                    "rows": {"type": "array", "items": {"type": ["string", "integer"]}, "description": "Rows to show: \"all\" (all the cells) and groups by name (as view_open lists them: C0, … or the run's cell types), or by number as view_locus lists them (0 all, 1 the first group); [] for all"},
                    "n": {"type": "integer", "minimum": 1, "description": "How many links to list [default: 20]"},
                    "format": {"enum": ["png", "text"], "description": "png: the figure as a picture [default]; text: the same figure in characters (braille arcs, block tracks), for a client that shows no images"},
                    "width": {"type": "integer", "minimum": 40, "maximum": 4096, "description": "This call only: image width in pixels [default: 1200], or text width in columns [default: 120]"},
                },
                "additionalProperties": false,
            }),
        ),
        ToolSpec::new(
            "view_export",
            "Write the figure as it is set, the locus (view_locus) or the genome-wide view \
             (view_genome), as a PDF (text kept as text) and/or a PNG, by chickpea view as a \
             step of the project's records. A file is never written over.",
            json!({
                "type": "object",
                "properties": {
                    "what": {"enum": ["locus", "genome"], "description": "Which figure [default: locus]"},
                    "pdf": {"type": "string", "description": "PDF path, ending .pdf"},
                    "png": {"type": "string", "description": "PNG path, ending .png"},
                    "width_in": {"type": "number", "exclusiveMinimum": 0, "description": "Page width in inches [default: 7]"},
                    "dpi": {"type": "integer", "minimum": 72, "maximum": 1200, "description": "Resolution [default: 300]"},
                },
                "additionalProperties": false,
            }),
        ),
    ]
}
