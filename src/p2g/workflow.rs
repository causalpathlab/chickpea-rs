//! The link workflow: multiome embed, peak module readout, gates, per-cluster tables.
//!
//! Outputs, all `{out}.*.parquet`:
//!
//! - `gene_embedding`: RNA gene rows;
//! - `cell_embedding`;
//! - `cell_clusters`: `cell`, `cluster` (its group's number, `-1` for a cell
//!   in no group) and `group` (its name, `""` for none) of every cell that
//!   passed QC;
//! - `groups`: `cluster`, `group`, `cells`: the groups, chickpea's own Leiden
//!   clusters (`C0`, `C1`, …) or those given with `--groups`;
//! - `peak_embedding`: each peak's ATAC-module row `μ_{m(p)}`;
//! - `peaks`: `peak`, `chromosome`, `start`, `end`;
//! - `genes`: `gene`, `chromosome`, `tss` of every gene with a position;
//! - `peak_rates`: `peak`, then each peak's accessibility (its reads over
//!   the cells' reads) in `all` the cells that passed QC and in each
//!   cluster, a column named by the cluster's number;
//! - `links`: `gene`, `peak`, `distance`, `abc`, `gate` (the gene's share of
//!   each pair under the trained distance prior; each gene's sum to 1, `0`
//!   for a closed pair), `corr` (the
//!   data's evidence: correlation across pseudobulks of the peak module's
//!   score and the gene's, so the peaks of one module share it) and `score = gate × corr`;
//! - `links_by_cluster`: `gene_idx`, `peak_idx`, `cluster`, `gate`, `abc`
//!   (indices into the RNA / peak name axes; shares re-weighted by cluster
//!   accessibility). Dense string copies of every pair×cluster row OOMs at
//!   genome scale.

use super::cis::CisPairs;
use super::context::for_each_context_share;
use super::groups::Groups;
use super::two_track::{embed_two_track, TwoTrackConfig, TwoTrackEmbedding, TwoTrackInput};
use crate::common::*;
use data_beans::sparse_io::open_sparse_matrix_by_path;
use genomic_data::coordinates::parse_peak_coordinates;
use graph_embedding_util as ge;
use legume_numeric::candle::candle_core::Device;
use legume_numeric::matrix::parquet::{write_table, Column};
use legume_numeric::matrix::traits::ConvertMatOps;
use nalgebra::DMatrix;
use rustc_hash::{FxHashMap, FxHashSet};

#[derive(Debug, Clone)]
pub struct LinkConfig {
    pub embed: TwoTrackConfig,
    /// Target Leiden cluster count; `None` lets the resolution decide.
    pub n_clusters: Option<usize>,
    /// Groups given from outside, in place of the Leiden clusters.
    pub groups: Option<Groups>,
    /// ATAC cells per block when streaming the cluster accessibility rates.
    pub block_size: usize,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            embed: TwoTrackConfig::default(),
            n_clusters: None,
            groups: None,
            block_size: 1000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LinkSummary {
    pub n_genes: usize,
    pub n_peaks: usize,
    pub n_pairs: usize,
    pub n_clusters: usize,
    pub theta0: f64,
    pub theta1: f64,
    /// The gene-to-cis-peak gap after training.
    pub align_gap: f64,
}

/// The tables [`run_links`] writes, each at [`table`]`(out, stem)`.
pub const GENE_EMBEDDING: &str = "gene_embedding";
pub const CELL_EMBEDDING: &str = "cell_embedding";
pub const PEAK_EMBEDDING: &str = "peak_embedding";
pub const CELL_CLUSTERS: &str = "cell_clusters";
pub const GROUPS: &str = "groups";
pub const PEAKS: &str = "peaks";
pub const GENES: &str = "genes";
pub const PEAK_RATES: &str = "peak_rates";
pub const LINKS: &str = "links";
pub const LINKS_BY_CLUSTER: &str = "links_by_cluster";
pub const OUTPUT_STEMS: [&str; 10] = [
    GENE_EMBEDDING,
    CELL_EMBEDDING,
    PEAK_EMBEDDING,
    CELL_CLUSTERS,
    GROUPS,
    PEAKS,
    GENES,
    PEAK_RATES,
    LINKS,
    LINKS_BY_CLUSTER,
];

/// Where a run with output prefix `out` writes table `stem`.
pub fn table(out: &str, stem: &str) -> String {
    format!("{out}.{stem}.parquet")
}

/// Every table a run with output prefix `out` writes.
pub fn tables(out: &str) -> Vec<std::path::PathBuf> {
    OUTPUT_STEMS
        .iter()
        .map(|stem| table(out, stem).into())
        .collect()
}

/// What `chickpea regroup` writes beside its tables: the run it regrouped
/// (whose `links`, `peaks`, `genes` and `gene_embedding` it uses), and the
/// groups and ATAC counts it read.
pub const REGROUP_JSON: &str = "regroup.json";

/// Where `prefix`'s regroup record is.
#[must_use]
pub fn regroup_record(prefix: &str) -> String {
    format!("{prefix}.{REGROUP_JSON}")
}

/// [`trained_prefix`], which must have its `links`.
pub fn trained_run(prefix: &str) -> anyhow::Result<String> {
    let trained = trained_prefix(prefix)?;
    let links = table(&trained, LINKS);
    anyhow::ensure!(
        std::path::Path::new(&links).exists(),
        "{links} does not exist: give the out of a finished peak-to-gene run"
    );
    Ok(trained)
}

/// Whether `prefix` is a run on disk: a trained one, or a regrouped one.
#[must_use]
pub fn is_run(prefix: &str) -> bool {
    std::path::Path::new(&table(prefix, LINKS)).exists()
        || std::path::Path::new(&regroup_record(prefix)).exists()
}

/// The prefix whose trained tables (`links`, `peaks`, `genes`,
/// `gene_embedding`) run `prefix` uses: its own, or for a regrouped run,
/// those of the run its record names (regroup records the trained run, so
/// one step).
pub fn trained_prefix(prefix: &str) -> anyhow::Result<String> {
    let record = regroup_record(prefix);
    if std::path::Path::new(&table(prefix, LINKS)).exists()
        || !std::path::Path::new(&record).exists()
    {
        return Ok(prefix.to_string());
    }
    let text =
        std::fs::read_to_string(&record).map_err(|e| anyhow::anyhow!("reading {record}: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("{record} is not JSON: {e}"))?;
    let run = v["run"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("{record} names no run"))?;
    // Recorded relative to the record's folder, and in full for when the
    // record's folder has moved without the run.
    let dir = std::path::Path::new(&record)
        .parent()
        .unwrap_or(std::path::Path::new(""));
    let near = tidy(&dir.join(run)).to_string_lossy().into_owned();
    let trained = |p: &str| std::path::Path::new(&table(p, LINKS)).exists();
    Ok(match v["run_abs"].as_str() {
        Some(full) if !trained(&near) && trained(full) => full.to_string(),
        _ => near,
    })
}

/// `path` with its `.` and `..` worked out by name, not by the file system
/// (`a/b/../c` is `a/c`).
#[must_use]
pub fn tidy(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(out.components().next_back(), Some(Component::Normal(_))) =>
            {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// The out prefix `path` names: a prefix, or one of the run's tables (with
/// or without `.parquet`). A prefix that is a run on disk is kept whole, even
/// one that ends like a table (`regroup.cells.groups`).
#[must_use]
pub fn prefix_of(path: &str) -> String {
    if let Some(p) = path.strip_suffix(&format!(".{REGROUP_JSON}")) {
        return p.to_string();
    }
    let p = match path.strip_suffix(".parquet") {
        Some(p) => p,
        None if is_run(path) => return path.to_string(),
        None => path,
    };
    for stem in OUTPUT_STEMS {
        if let Some(prefix) = p.strip_suffix(&format!(".{stem}")) {
            return prefix.to_string();
        }
    }
    p.to_string()
}

pub fn run_links(
    input: &TwoTrackInput,
    cfg: &LinkConfig,
    keep: Option<&FxHashSet<Box<str>>>,
) -> anyhow::Result<LinkSummary> {
    let out = input.work_prefix;
    let emb = embed_two_track(input, &cfg.embed)?;

    let kept: Vec<usize> = match keep {
        Some(set) => (0..emb.barcodes.len())
            .filter(|&c| {
                let b = emb.barcodes[c].as_ref();
                set.contains(b) || b.split_once('@').is_some_and(|(raw, _)| set.contains(raw))
            })
            .collect(),
        None => (0..emb.barcodes.len()).collect(),
    };
    anyhow::ensure!(!kept.is_empty(), "no embedded cell passed QC");
    info!("Cells: {} of {} pass QC", kept.len(), emb.barcodes.len());
    let cell_rows = emb.cell_rows.select_rows(kept.iter());
    let barcodes: Vec<Box<str>> = kept.iter().map(|&c| emb.barcodes[c].clone()).collect();

    let dev = Device::Cpu;
    let labels = match &cfg.groups {
        Some(g) => Labels::given(g, &barcodes)?,
        None => {
            let (l, _) = ge::cell_clusters(&cell_rows.to_tensor(&dev)?, cfg.n_clusters)?;
            Labels::numbered(l)
        }
    };
    let n_clusters = labels.names.len();

    let (lambda, pooled) = stream_cluster_rates(
        input.atac_file,
        &emb.peak_names,
        &barcodes,
        &labels,
        cfg.block_size,
    )?;
    info!(
        "Gates: θ0={:.3} θ1={:.3}; gene-to-cis-peak gap {:.4}",
        emb.gate_theta0, emb.gate_theta1, emb.align_gap
    );

    let save = |stem: &str, m: &DMatrix<f32>, names: &[Box<str>], axis: &str| {
        ge::save_embedding(&table(out, stem), &m.to_tensor(&dev)?, names, axis)
    };
    save(GENE_EMBEDDING, &emb.rna_rows, &emb.genes, "gene")?;
    save(CELL_EMBEDDING, &cell_rows, &barcodes, "cell")?;
    save(PEAK_EMBEDDING, &emb.peak_rows, &emb.peak_names, "peak")?;

    write_cell_tables(out, &barcodes, &labels)?;
    write_peaks(&table(out, PEAKS), &emb.peak_names)?;
    write_genes(&table(out, GENES), &emb.genes, input.gene_positions)?;
    write_peak_rates(&table(out, PEAK_RATES), &emb.peak_names, &pooled, &lambda)?;
    write_links(out, &emb, &emb.gate_w)?;
    write_links_by_cluster(
        &table(out, LINKS_BY_CLUSTER),
        &emb.pairs,
        &emb.gate_w,
        &lambda,
    )?;

    Ok(LinkSummary {
        n_genes: emb.rna_genes.len(),
        n_peaks: emb.peak_names.len(),
        n_pairs: emb.pairs.n_pairs(),
        n_clusters,
        theta0: f64::from(emb.gate_theta0),
        theta1: f64::from(emb.gate_theta1),
        align_gap: f64::from(emb.align_gap),
    })
}

/// The cells' groups: each cell's number in `names`, or none.
pub struct Labels {
    pub names: Vec<Box<str>>,
    pub of: Vec<Option<usize>>,
}

impl Labels {
    /// Leiden clusters `0..k`, named `C0`, `C1`, ….
    #[must_use]
    pub fn numbered(clusters: Vec<usize>) -> Self {
        let k = clusters.iter().max().map_or(0, |m| m + 1);
        Self {
            names: (0..k).map(|k| format!("C{k}").into()).collect(),
            of: clusters.into_iter().map(Some).collect(),
        }
    }

    /// `groups`' groups of `cells`, keeping only the groups with a cell
    /// here (in their order).
    pub fn given(groups: &Groups, cells: &[Box<str>]) -> anyhow::Result<Self> {
        let of = groups.label(cells);
        let mut used = vec![false; groups.names.len()];
        for k in of.iter().flatten() {
            used[*k] = true;
        }
        anyhow::ensure!(
            used.iter().any(|&u| u),
            "no cell of the run has a group in {}",
            groups.table
        );
        let mut new_of = vec![None; used.len()];
        let mut names = Vec::new();
        for (k, _) in used.iter().enumerate().filter(|(_, &u)| u) {
            new_of[k] = Some(names.len());
            names.push(groups.names[k].clone());
        }
        let left: Vec<&str> = groups
            .names
            .iter()
            .zip(&used)
            .filter(|(_, &u)| !u)
            .map(|(n, _)| &**n)
            .collect();
        if !left.is_empty() {
            log::warn!("groups with no cell in the run: {}", left.join(", "));
        }
        Ok(Self {
            names,
            of: of.into_iter().map(|k| k.and_then(|k| new_of[k])).collect(),
        })
    }

    /// Cells in each group.
    #[must_use]
    pub fn sizes(&self) -> Vec<usize> {
        let mut n = vec![0; self.names.len()];
        for k in self.of.iter().flatten() {
            n[*k] += 1;
        }
        n
    }
}

/// Each peak's accessibility (its reads over the cells' reads) in each
/// group of `cells` (`labels.of`, in their order) and in all of `cells`,
/// reading the ATAC counts `block` cells at a time. `peaks` are the ATAC
/// rows, in order.
pub fn stream_cluster_rates(
    atac_file: &str,
    peaks: &[Box<str>],
    cells: &[Box<str>],
    labels: &Labels,
    block: usize,
) -> anyhow::Result<(DMatrix<f32>, Vec<f32>)> {
    let n_clusters = labels.names.len();
    let atac = open_sparse_matrix_by_path(atac_file)?;
    let n_peaks = peaks.len();
    let rows = atac.row_names()?;
    anyhow::ensure!(
        rows.len() == n_peaks,
        "{atac_file} has {} peaks, the run {n_peaks}: is it the run's ATAC?",
        rows.len()
    );
    // Rates are kept by ATAC row and read by the run's peak: same order.
    if let Some(i) = (0..n_peaks).find(|&i| rows[i] != peaks[i]) {
        anyhow::bail!(
            "{atac_file}: row {i} is {}, the run's peak {}: is it the run's ATAC, in its order?",
            rows[i],
            peaks[i]
        );
    }
    let cell_of: FxHashMap<&str, usize> = cells
        .iter()
        .enumerate()
        .map(|(i, b)| (b.as_ref(), i))
        .collect();
    let columns = atac.column_names()?;
    let cell_idx: Vec<Option<Option<usize>>> = columns
        .iter()
        .map(|b| Some(labels.of[*cell_of.get(b.as_ref())?]))
        .collect();
    let n_matched = cell_idx.iter().filter(|c| c.is_some()).count();
    anyhow::ensure!(
        n_matched > 0,
        "no ATAC barcode matches an embedded cell that passed QC"
    );
    let block = block.max(1);
    let blocks: Vec<std::ops::Range<usize>> = (0..columns.len())
        .step_by(block)
        .map(|lb| lb..(lb + block).min(columns.len()))
        .collect();

    let mut cl_size = vec![0f32; n_clusters];
    let mut counts = DMatrix::<f32>::zeros(n_peaks, n_clusters);
    let mut all = vec![0f32; n_peaks];
    let mut total = 0f32;
    for cols in &blocks {
        let csc = atac.read_columns_csc(cols.clone().collect())?;
        for (j, col) in csc.col_iter().enumerate() {
            let Some(k) = cell_idx[cols.start + j] else {
                continue;
            };
            let depth: f32 = col.values().iter().sum();
            total += depth;
            if let Some(k) = k {
                cl_size[k] += depth;
            }
            for (&p, &x) in col.row_indices().iter().zip(col.values()) {
                all[p] += x;
                if let Some(k) = k {
                    counts[(p, k)] += x;
                }
            }
        }
    }
    let pooled = all
        .iter()
        .map(|&x| if total > 0.0 { x / total } else { 0.0 })
        .collect();
    let rates = DMatrix::from_fn(n_peaks, n_clusters, |p, k| {
        if cl_size[k] > 0.0 {
            counts[(p, k)] / cl_size[k]
        } else {
            0.0
        }
    });
    Ok((rates, pooled))
}

/// `cell_clusters` (each cell's group) and `groups` (the groups and their
/// cells) under `out`.
pub fn write_cell_tables(out: &str, cells: &[Box<str>], labels: &Labels) -> anyhow::Result<()> {
    let number: Vec<i32> = labels
        .of
        .iter()
        .map(|k| k.map_or(-1, |k| k as i32))
        .collect();
    let name: Vec<Box<str>> = labels
        .of
        .iter()
        .map(|k| k.map_or_else(|| Box::from(""), |k| labels.names[k].clone()))
        .collect();
    write_table(
        &table(out, CELL_CLUSTERS),
        &[
            ("cell".into(), Column::Str(cells)),
            ("cluster".into(), Column::I32(&number)),
            ("group".into(), Column::Str(&name)),
        ],
    )?;
    let ks: Vec<i32> = (0..labels.names.len() as i32).collect();
    let sizes: Vec<i64> = labels.sizes().into_iter().map(|n| n as i64).collect();
    write_table(
        &table(out, GROUPS),
        &[
            ("cluster".into(), Column::I32(&ks)),
            ("group".into(), Column::Str(&labels.names)),
            ("cells".into(), Column::I64(&sizes)),
        ],
    )
}

/// Each peak's accessibility in all the cells (`pooled`) and in each
/// cluster (`lambda`'s columns), as the locus figure draws it.
pub fn write_peak_rates(
    path: &str,
    names: &[Box<str>],
    pooled: &[f32],
    lambda: &DMatrix<f32>,
) -> anyhow::Result<()> {
    let mut columns = vec![
        ("peak".into(), Column::Str(names)),
        ("all".into(), Column::F32(pooled)),
    ];
    // Column-major: each group's rates are one slice, written uncopied.
    let rates = lambda.as_slice().chunks(lambda.nrows().max(1));
    for (k, rates) in rates.take(lambda.ncols()).enumerate() {
        columns.push((k.to_string().into(), Column::F32(rates)));
    }
    write_table(path, &columns)
}

fn write_peaks(path: &str, names: &[Box<str>]) -> anyhow::Result<()> {
    let coords = parse_peak_coordinates(names);
    let chr: Vec<Box<str>> = coords
        .iter()
        .map(|c| c.as_ref().map_or_else(|| Box::from(""), |c| c.chr.clone()))
        .collect();
    let start: Vec<i64> = coords
        .iter()
        .map(|c| c.as_ref().map_or(-1, |c| c.start))
        .collect();
    let end: Vec<i64> = coords
        .iter()
        .map(|c| c.as_ref().map_or(-1, |c| c.end))
        .collect();
    write_table(
        path,
        &[
            ("peak".into(), Column::Str(names)),
            ("chromosome".into(), Column::Str(&chr)),
            ("start".into(), Column::I64(&start)),
            ("end".into(), Column::I64(&end)),
        ],
    )
}

fn write_genes(
    path: &str,
    names: &[Box<str>],
    positions: &[Option<genomic_data::coordinates::GeneTss>],
) -> anyhow::Result<()> {
    let placed: Vec<(&Box<str>, &genomic_data::coordinates::GeneTss)> = names
        .iter()
        .zip(positions)
        .filter_map(|(g, p)| Some((g, p.as_ref()?)))
        .collect();
    let gene: Vec<Box<str>> = placed.iter().map(|(g, _)| (*g).clone()).collect();
    let chr: Vec<Box<str>> = placed.iter().map(|(_, p)| p.chr.clone()).collect();
    let tss: Vec<i64> = placed.iter().map(|(_, p)| p.tss).collect();
    write_table(
        path,
        &[
            ("gene".into(), Column::Str(&gene)),
            ("chromosome".into(), Column::Str(&chr)),
            ("tss".into(), Column::I64(&tss)),
        ],
    )
}

fn write_links(out: &str, emb: &TwoTrackEmbedding, w: &[f32]) -> anyhow::Result<()> {
    let pairs = &emb.pairs;
    let mut gene: Vec<Box<str>> = Vec::with_capacity(pairs.n_pairs());
    for g in 0..pairs.n_genes() {
        gene.extend(pairs.gene(g).map(|_| emb.rna_genes[g].clone()));
    }
    let peak: Vec<Box<str>> = pairs
        .peak
        .iter()
        .map(|&p| emb.peak_names[p as usize].clone())
        .collect();
    let score: Vec<f32> = w.iter().zip(&emb.gate_corr).map(|(w, c)| w * c).collect();
    // Indices beside the names, as in `links_by_cluster`: a name may be
    // several genes'.
    let gene_idx = gene_of_pair(pairs);
    let peak_idx: Vec<i32> = pairs.peak.iter().map(|&p| p as i32).collect();
    write_table(
        &table(out, LINKS),
        &[
            ("gene".into(), Column::Str(&gene)),
            ("peak".into(), Column::Str(&peak)),
            ("gene_idx".into(), Column::I32(&gene_idx)),
            ("peak_idx".into(), Column::I32(&peak_idx)),
            ("distance".into(), Column::I64(&pairs.dist)),
            ("abc".into(), Column::F32(&pairs.weight)),
            ("gate".into(), Column::F32(w)),
            ("corr".into(), Column::F32(&emb.gate_corr)),
            ("score".into(), Column::F32(&score)),
        ],
    )
}

/// Each pair's gene.
fn gene_of_pair(pairs: &CisPairs) -> Vec<i32> {
    let mut of = vec![0i32; pairs.n_pairs()];
    for g in 0..pairs.n_genes() {
        for k in pairs.gene(g) {
            of[k] = g as i32;
        }
    }
    of
}

/// Each pair's share in each group ([`for_each_context_share`]) at `path`,
/// under the gates `w` and the peak × group accessibility `lambda`.
pub fn write_links_by_cluster(
    path: &str,
    pairs: &CisPairs,
    w: &[f32],
    lambda: &DMatrix<f32>,
) -> anyhow::Result<()> {
    // Integer indices — not gene/peak name strings — so a genome-scale
    // pair×cluster table stays in the hundreds of MB, not tens of GB.
    let (mut g_i, mut p_i, mut k_i) = (Vec::<i32>::new(), Vec::<i32>::new(), Vec::<i32>::new());
    let (mut a_c, mut b_c) = (Vec::<f32>::new(), Vec::<f32>::new());
    let gene_of_pair = gene_of_pair(pairs);
    for_each_context_share(pairs, w, lambda, |k, j, a, b| {
        g_i.push(gene_of_pair[k]);
        p_i.push(pairs.peak[k] as i32);
        k_i.push(j as i32);
        a_c.push(a);
        b_c.push(b);
    })?;
    write_table(
        path,
        &[
            ("gene_idx".into(), Column::I32(&g_i)),
            ("peak_idx".into(), Column::I32(&p_i)),
            ("cluster".into(), Column::I32(&k_i)),
            ("gate".into(), Column::F32(&a_c)),
            ("abc".into(), Column::F32(&b_c)),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_whose_prefix_ends_like_a_table_is_kept_whole() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("regroup.cells.groups");
        let out = out.to_str().unwrap();
        std::fs::write(regroup_record(out), "{}").unwrap();
        assert_eq!(prefix_of(out), out);
        assert_eq!(prefix_of(&table(out, LINKS_BY_CLUSTER)), out);
        assert_eq!(prefix_of(&regroup_record(out)), out);
    }

    #[test]
    fn tidy_works_out_dots_by_name() {
        let t = |p: &str| tidy(std::path::Path::new(p)).to_string_lossy().into_owned();
        assert_eq!(t("results/d/../../out/p2g"), "out/p2g");
        assert_eq!(t("../x/./y"), "../x/y");
        assert_eq!(t("/a/b/../c"), "/a/c");
    }

    #[test]
    fn a_regroup_record_leads_back_to_the_trained_run_after_a_move() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("train/p2g");
        std::fs::create_dir_all(run.parent().unwrap()).unwrap();
        std::fs::write(table(run.to_str().unwrap(), LINKS), "").unwrap();
        let moved = dir.path().join("elsewhere/deep/rg");
        std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
        // Its relative path no longer leads anywhere; the full one does.
        std::fs::write(
            regroup_record(moved.to_str().unwrap()),
            serde_json::json!({"run": "../train/p2g", "run_abs": run.to_str().unwrap()})
                .to_string(),
        )
        .unwrap();
        assert_eq!(
            trained_prefix(moved.to_str().unwrap()).unwrap(),
            run.to_str().unwrap()
        );
    }
}
