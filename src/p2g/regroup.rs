//! `regroup`: a finished run's per-group tables again, for other groups
//! (a lupin round's cell types, say), without training it again.
//!
//! A group's shares need only the trained gates and the ABC prior of each
//! pair (`links`) and the peaks' accessibility in the group's cells (the
//! ATAC counts), so the new run reads those and writes `groups`,
//! `cell_clusters`, `peak_rates` and `links_by_cluster` under its own out,
//! and `{out}.regroup.json`, which names the run whose `links`, `peaks`,
//! `genes` and `gene_embedding` it uses (and the groups and samples it read),
//! so it opens as a run of its own (`chickpea view`) without copies. The
//! ATAC files are those the run's record (`{run}.run.json`) names, or for a
//! regrouped run, those its regroup record names.

use crate::common::*;
use crate::p2g::cis::CisPairs;
use crate::p2g::groups::Groups;
use crate::p2g::record::{
    ensure_atac, folder, read_samples, recorded, relative_to, run_record, samples_json, with_atac,
    write_json,
};
use crate::p2g::samples::{tags_of, Sample};
use crate::p2g::workflow::{
    prefix_of, regroup_record, stream_cluster_rates, table, trained_run, write_cell_tables,
    write_links_by_cluster, write_peak_rates, Labels, CELL_CLUSTERS, GENE_EMBEDDING, GROUPS, LINKS,
    LINKS_BY_CLUSTER, PEAKS, PEAK_RATES,
};
use legume_numeric::matrix::parquet::read_table_columns;
use rustc_hash::FxHashMap;
use std::path::PathBuf;

/// What regroup computes anew.
pub const MADE: [&str; 4] = [GROUPS, CELL_CLUSTERS, PEAK_RATES, LINKS_BY_CLUSTER];

#[derive(Args, Debug)]
pub struct RegroupArgs {
    #[arg(
        long,
        value_hint = clap::ValueHint::FilePath,
        help = "The finished run: its out prefix, or one of its tables"
    )]
    run: Box<str>,

    #[arg(
        long,
        num_args = 1..,
        value_hint = clap::ValueHint::FilePath,
        help = "ATAC peak counts in place of the files the run's record names",
        long_help = "ATAC peak counts (zarr/h5) in place of the files the run's record\n\
                     ({run}.run.json, or a regrouped run's {run}.regroup.json) names,\n\
                     for a sample whose file has moved: each\n\
                     stands in for the sample whose name its file name has (or whose\n\
                     recorded file has its name). A run without a record needs every\n\
                     sample's, in the order peak-to-gene was given them."
    )]
    atac: Vec<Box<str>>,

    #[arg(
        long,
        value_hint = clap::ValueHint::FilePath,
        help = "The new groups: a lupin round, its argmax table, or a cell, group table",
        long_help = "The new groups: a lupin round's manifest (.json, read for its\n\
                     annotate.argmax), its argmax table, or any table of cell and\n\
                     cell_type (or group) columns. Unassigned cells and cells it\n\
                     does not name are in no group; a cell is matched by name or\n\
                     by its barcode before '@'."
    )]
    groups: Box<str>,

    #[arg(long, default_value_t = 1000, help = "ATAC cells read at a time")]
    block_size: usize,

    #[arg(long, short, help = "Output prefix for the regrouped {out}.*.parquet")]
    out: Box<str>,
}

impl RegroupArgs {
    /// Every file the regroup writes.
    #[must_use]
    pub fn outputs(&self) -> Vec<PathBuf> {
        Self::outputs_of(&self.out)
    }

    /// Every file a regroup with output prefix `out` writes.
    #[must_use]
    pub fn outputs_of(out: &str) -> Vec<PathBuf> {
        let mut made: Vec<PathBuf> = MADE.iter().map(|stem| table(out, stem).into()).collect();
        made.push(regroup_record(out).into());
        made
    }
}

pub fn run_regroup(args: &RegroupArgs) -> anyhow::Result<()> {
    let from = prefix_of(&args.run);
    let trained = trained_run(&from)?;
    let links = table(&trained, LINKS);
    anyhow::ensure!(
        from != *args.out,
        "--out is the run itself: give the regrouped run an out of its own"
    );
    if let Some(there) = args.outputs().iter().find(|p| p.exists()) {
        anyhow::bail!("{} exists: a run is not written over", there.display());
    }
    let groups = Groups::read(&args.groups)?;
    mkdir_parent(&args.out)?;

    let strings = |at: &str, stem: &str, col: &str| -> anyhow::Result<Vec<Box<str>>> {
        let path = table(at, stem);
        let (mut s, _) = read_table_columns(&path, &[col], &[])
            .map_err(|e| anyhow::anyhow!("reading {path}: {e}"))?;
        Ok(s.pop().expect("the column asked for"))
    };
    let cells = strings(&from, CELL_CLUSTERS, "cell")?;
    let peaks = strings(&trained, PEAKS, "peak")?;
    let genes = strings(&trained, GENE_EMBEDDING, "gene")?;
    let (pairs, gate) = read_pairs(&links, &genes, &peaks)?;
    info!(
        "Run {from}: {} cells, {} genes, {} peaks, {} pairs",
        cells.len(),
        genes.len(),
        peaks.len(),
        pairs.n_pairs()
    );

    let labels = Labels::given(&groups, &cells)?;
    let samples = atac_of(&from, &trained, &cells, &args.atac)?;
    let atac: Vec<(&str, Option<&str>)> = samples
        .iter()
        .map(|s| (&*s.atac, s.name.as_deref()))
        .collect();
    let (lambda, pooled) = stream_cluster_rates(&atac, &peaks, &cells, &labels, args.block_size)?;
    write_cell_tables(&args.out, &cells, &labels)?;
    write_peak_rates(&table(&args.out, PEAK_RATES), &peaks, &pooled, &lambda)?;
    write_links_by_cluster(&table(&args.out, LINKS_BY_CLUSTER), &pairs, &gate, &lambda)?;
    write_record(&args.out, &trained, &args.groups, &samples)?;
    info!(
        "Done: {} groups ({}) → {}",
        labels.names.len(),
        labels.names.join(", "),
        table(&args.out, LINKS_BY_CLUSTER)
    );
    Ok(())
}

/// Each sample of run `from` (trained at `trained`), whose cells are
/// `cells`, with the ATAC to read: its record's ([`read_samples`]),
/// `given` standing in for a sample's; for a run without a record, `given`
/// in the order of the samples' tags.
fn atac_of(
    from: &str,
    trained: &str,
    cells: &[Box<str>],
    given: &[Box<str>],
) -> anyhow::Result<Vec<Sample>> {
    let recorded = match read_samples(from, trained)? {
        Some(recorded) => recorded,
        None => {
            anyhow::ensure!(
                !given.is_empty(),
                "{} does not exist (a run from before records): give each sample's ATAC \
                 with --atac, in the order peak-to-gene was given them",
                run_record(trained)
            );
            // The samples by their tags alone, their files all given.
            tags_of(cells, given.len())?
                .into_iter()
                .map(|name| Sample {
                    name,
                    rna: "".into(),
                    atac: "".into(),
                })
                .collect()
        }
    };
    let samples = with_atac(recorded, given)?;
    ensure_atac(&samples)?;
    Ok(samples)
}

/// `{out}.regroup.json`: the trained run whose tables `out` uses (relative
/// to the record's folder, and in full), the groups it read, and the
/// samples with the ATAC it read.
fn write_record(out: &str, trained: &str, groups: &str, samples: &[Sample]) -> anyhow::Result<()> {
    let record = regroup_record(out);
    let dir = folder(&record);
    let (run, run_abs) = recorded(trained, dir);
    let v = serde_json::json!({
        "run": run,
        "run_abs": run_abs,
        "groups": relative_to(groups, dir),
        "samples": samples_json(samples, dir),
    });
    write_json(&record, &v)
}

/// The run's pairs, gene by gene in `genes`' order, and their gates, from
/// its `links` table: by its `gene_idx` and `peak_idx`, or in a run from
/// before those, by name (refused where two of the run's genes share one).
fn read_pairs(
    links: &str,
    genes: &[Box<str>],
    peaks: &[Box<str>],
) -> anyhow::Result<(CisPairs, Vec<f32>)> {
    let (_, n) = read_table_columns(links, &[], &["distance", "abc", "gate"])
        .map_err(|e| anyhow::anyhow!("reading {links}: {e}"))?;
    let [distance, abc, gate] = &n[..] else {
        unreachable!("the columns asked for")
    };
    let at = match read_table_columns(links, &[], &["gene_idx", "peak_idx"]) {
        Ok((_, idx)) => {
            let at: Vec<(usize, usize)> = idx[0]
                .iter()
                .zip(&idx[1])
                .map(|(&g, &p)| (g as usize, p as usize))
                .collect();
            if let Some((g, p)) = at
                .iter()
                .find(|&&(g, p)| g >= genes.len() || p >= peaks.len())
            {
                anyhow::bail!("{links}: gene {g}, peak {p} are not the run's");
            }
            at
        }
        Err(_) => by_name(links, genes, peaks)?,
    };
    let mut rows: Vec<(usize, usize)> = at.iter().enumerate().map(|(i, &(g, _))| (g, i)).collect();
    rows.sort_unstable();
    let mut gene_ptr = vec![0usize; genes.len() + 1];
    for &(g, _) in &rows {
        gene_ptr[g + 1] += 1;
    }
    for g in 0..genes.len() {
        gene_ptr[g + 1] += gene_ptr[g];
    }
    let mut pairs = CisPairs {
        gene_ptr,
        peak: Vec::with_capacity(rows.len()),
        dist: Vec::with_capacity(rows.len()),
        weight: Vec::with_capacity(rows.len()),
        n_genes_placed: 0,
        n_unparsed_peaks: 0,
        n_unreached_peaks: 0,
    };
    let mut gates = Vec::with_capacity(rows.len());
    for &(_, i) in &rows {
        pairs.peak.push(at[i].1 as u32);
        pairs.dist.push(distance[i] as i64);
        pairs.weight.push(abc[i] as f32);
        gates.push(gate[i] as f32);
    }
    pairs.n_genes_placed = (0..genes.len())
        .filter(|&g| !pairs.gene(g).is_empty())
        .count();
    Ok((pairs, gates))
}

/// Each `links` row's gene and peak by its names, in a run written before
/// `links` had their indices.
fn by_name(
    links: &str,
    genes: &[Box<str>],
    peaks: &[Box<str>],
) -> anyhow::Result<Vec<(usize, usize)>> {
    let (s, _) = read_table_columns(links, &["gene", "peak"], &[])
        .map_err(|e| anyhow::anyhow!("reading {links}: {e}"))?;
    let index = |names: &[Box<str>]| -> FxHashMap<Box<str>, Option<usize>> {
        let mut m: FxHashMap<Box<str>, Option<usize>> = FxHashMap::default();
        for (i, n) in names.iter().enumerate() {
            m.entry(n.clone())
                .and_modify(|e| *e = None)
                .or_insert(Some(i));
        }
        m
    };
    let (gene_of, peak_of) = (index(genes), index(peaks));
    let find = |m: &FxHashMap<Box<str>, Option<usize>>, name: &str, what: &str| match m.get(name) {
        Some(Some(i)) => Ok(*i),
        Some(None) => Err(anyhow::anyhow!(
            "{links}: two of the run's {what}s are named {name}, and this run's links \
             predate gene_idx: run peak-to-gene again to regroup it"
        )),
        None => Err(anyhow::anyhow!(
            "{links}: {what} {name} is not one of the run's"
        )),
    };
    s[0].iter()
        .zip(&s[1])
        .map(|(g, p)| Ok((find(&gene_of, g, "gene")?, find(&peak_of, p, "peak")?)))
        .collect()
}
