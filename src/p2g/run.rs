//! `peak-to-gene` CLI arguments and the run.

use crate::common::*;
use crate::p2g::cis::AbcKernel;
use crate::p2g::input::load_gene_coords_tsv;
use crate::p2g::record::{run_record, write_run_record};
use crate::p2g::samples::{peak_rows, same_rows, Sample, Side};
use crate::p2g::two_track::{TwoTrackConfig, TwoTrackInput};
use crate::p2g::workflow::{run_links, table, tables, LinkConfig, LINKS};
use data_beans::qc_lib::QcReport;
use data_beans::sparse_io::open_sparse_matrix_by_path;
use genomic_data::coordinates::load_gene_tss;
use rustc_hash::FxHashSet;

#[derive(Args, Debug, serde::Serialize)]
#[command(group = clap::ArgGroup::new("positions")
    .required(true)
    .args(["gene_coords", "gff_file"]))]
#[command(group = clap::ArgGroup::new("counts")
    .required(true)
    .multiple(true)
    .args(["data", "rna"]))]
pub struct PeakToGeneArgs {
    /* Input */
    #[arg(
        long,
        num_args = 1..,
        conflicts_with_all = ["rna", "atac"],
        help = "Count files in any order: RNA, ATAC, or both in one file, paired by name",
        long_help = "Count files (zarr/h5) in any order, in place of --rna and --atac.\n\
                     Each is told RNA, ATAC, or both (genes and peaks as rows of one\n\
                     file, a 10x multiome matrix say) by its rows, a peak being named\n\
                     chr:start-end. A file of both is a sample by itself; RNA and ATAC\n\
                     files are paired by the words of their names (s1_gex.h5 with\n\
                     s1_atac.h5), else in the order given."
    )]
    #[serde(skip)]
    data: Vec<Box<str>>,

    #[arg(
        long,
        num_args = 1..,
        requires = "atac",
        help = "RNA gene counts (zarr/h5), one file per sample",
        long_help = "RNA gene counts (zarr/h5), one file per sample. With several\n\
                     samples, each sample's cells are {barcode}@{sample}, the sample\n\
                     named after its files, and each sample is a batch. A file of\n\
                     genes and peaks together can be given as both --rna and --atac."
    )]
    #[serde(skip)]
    rna: Vec<Box<str>>,

    #[arg(
        long,
        num_args = 1..,
        requires = "rna",
        help = "ATAC peak counts (zarr/h5) for the same barcodes, one file per sample in --rna's order"
    )]
    #[serde(skip)]
    atac: Vec<Box<str>>,

    #[arg(
        long,
        value_hint = clap::ValueHint::FilePath,
        help = "Batch labels, one per cell (default: each sample a batch)"
    )]
    batch: Option<Box<str>>,

    /// Shared cell QC, driven by the RNA counts (on by default; `--no-qc` to
    /// disable). Failed cells still inform the embedding, but are left out of
    /// clustering, accessibility rates and every cell table.
    #[command(flatten)]
    qc: data_beans::qc_lib::QcArgs,

    /* Gene positions (one required) */
    #[arg(
        long,
        value_hint = clap::ValueHint::FilePath,
        help = "Gene TSS table: gene, chr, tss, with a header"
    )]
    gene_coords: Option<Box<str>>,

    #[arg(
        long,
        value_hint = clap::ValueHint::FilePath,
        help = "GFF/GTF annotation for gene TSS"
    )]
    gff_file: Option<Box<str>>,

    /* Cis candidates and the ABC contact prior */
    #[arg(
        long,
        default_value_t = 500_000,
        help = "Max peak-midpoint distance (bp) to a gene's TSS"
    )]
    cis_window: i64,

    #[arg(
        long,
        default_value_t = 200,
        help = "Max candidate peaks per gene, nearest first"
    )]
    max_cis: usize,

    #[arg(long, default_value_t = 1.0, help = "Contact exponent γ in (d + c)^-γ")]
    contact_gamma: f32,

    #[arg(long, default_value_t = 5000.0, help = "Contact pseudocount c (bp)")]
    contact_pseudocount: f32,

    #[arg(
        long,
        default_value_t = 0.1,
        help = "Weight of the gene-to-cis-peak alignment, per unit",
        long_help = "Weight of the gene-to-cis-peak alignment, per unit.\n\
                     Pulls each gene's profile across pseudobulks toward the\n\
                     predicted accessibility of its cis peaks, so genes and\n\
                     peaks share one feature space. 0 turns it off."
    )]
    align_weight: f32,

    #[arg(
        long,
        default_value_t = 0.5,
        help = "Share of each gene's score taken from its cis peaks, in [0, 1)",
        long_help = "Share of each gene's score taken from its cis peaks, in [0, 1).\n\
                     A gene with cis pairs scores (1 − mix) of its own row plus\n\
                     mix of its cis peaks' predicted accessibility, and the\n\
                     written gene rows carry the same mixture. 0 keeps the gene\n\
                     likelihood exact."
    )]
    mix: f32,

    /* Multiome embedding (bge multiome recipe) */
    #[arg(long, default_value_t = 128, help = "Embedding dimension")]
    embedding_dim: usize,

    #[arg(long, default_value_t = 1000, help = "Embedding epochs")]
    epochs: usize,

    #[arg(long, default_value_t = 3, help = "Pseudobulk levels")]
    num_levels: usize,

    #[arg(
        long,
        default_value_t = 10,
        help = "Binary sort dimension; about 2^sort_dim finest pseudobulks"
    )]
    sort_dim: usize,

    #[arg(long, default_value_t = 50, help = "Random projection dimension")]
    proj_dim: usize,

    #[arg(
        long,
        default_value_t = 1e-4,
        help = "L2 weight decay on the gene and peak embeddings",
        long_help = "L2 weight decay on the gene and peak embeddings.\n\
                     Every feature row a phase-1 step touches (each modality's\n\
                     module and gene rows) shrinks by 1 − lr·wd before its\n\
                     update, so directions the data never pins down decay.\n\
                     The pseudobulk rows and biases never decay. 0 turns it off."
    )]
    weight_decay: f64,

    #[arg(long, default_value_t = 1024, help = "RNA gene modules")]
    feature_modules: usize,

    #[arg(
        long,
        default_value_t = 10_000,
        help = "ATAC peak modules",
        long_help = "ATAC peak modules. Each peak's row is its module's row, so\n\
                     this sets how finely peaks resolve; on the order of the\n\
                     genes with their own rows keeps the two sides comparable."
    )]
    peak_modules: usize,

    #[arg(
        long,
        default_value_t = 16,
        help = "Cells per finest pseudobulk drawn into training"
    )]
    phase1_cells_per_pb: usize,

    #[arg(
        long,
        default_value_t = 100_000,
        help = "ATAC modality is module-only when it has at least this many peaks (0 = off)"
    )]
    module_only_min_rows: usize,

    #[arg(long, default_value_t = ComputeDevice::Cpu, value_enum, help = "Compute device")]
    #[serde(skip)]
    device: ComputeDevice,

    #[arg(long, default_value_t = 0, help = "Device index (CUDA/Metal)")]
    #[serde(skip)]
    device_no: usize,

    /* Clusters, seed, output */
    #[arg(
        long,
        help = "Target number of cell clusters (default: Leiden decides)"
    )]
    n_clusters: Option<usize>,

    #[arg(
        long,
        value_hint = clap::ValueHint::FilePath,
        conflicts_with = "n_clusters",
        help = "Cell groups in place of the Leiden clusters: a lupin round, or a cell, group table",
        long_help = "Cell groups in place of the Leiden clusters, for the per-group\n\
                     tables: a lupin round's manifest (.json, read for its\n\
                     annotate.argmax), its argmax table, or any table of cell and\n\
                     cell_type (or group) columns. Unassigned cells and cells it\n\
                     does not name are in no group; a cell is matched by name or\n\
                     by its barcode before '@'. `chickpea regroup` redoes the\n\
                     groups of a finished run without training it again."
    )]
    #[serde(skip)]
    groups: Option<Box<str>>,

    #[arg(long, default_value_t = 42, help = "Random seed")]
    seed: u64,

    #[arg(long, short, help = "Output prefix for {out}.*.parquet")]
    #[serde(skip)]
    out: Box<str>,
}

impl PeakToGeneArgs {
    /// The flags the run's record keeps: those that shape what it trains,
    /// its gene positions and batches as given (not its files, device or
    /// out).
    fn flags(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).expect("plain fields");
        if let Some(qc) = v["qc"].as_object_mut() {
            // Where its reports go, not which cells are kept.
            qc.remove("qc_report");
            qc.remove("qc_histogram");
        }
        v
    }

    /// The samples: `--data` told apart and paired by reading the files'
    /// rows, or `--rna` and `--atac` paired by position.
    pub fn samples(&self) -> anyhow::Result<Vec<Sample>> {
        if self.data.is_empty() {
            crate::p2g::samples::pair(&self.rna, &self.atac)
        } else {
            crate::p2g::samples::infer(&self.data)
        }
    }

    /// Refuse `--rna` and `--atac` that do not pair, reading no file (the
    /// check before a run; `--data` is sorted out when it starts).
    pub fn check_pairs(&self) -> anyhow::Result<()> {
        if self.data.is_empty() {
            crate::p2g::samples::pair(&self.rna, &self.atac)?;
        }
        Ok(())
    }

    /// Every file a run with output prefix `out` writes, whatever its
    /// flags: its tables and its record.
    #[must_use]
    pub fn outputs_of(out: &str) -> Vec<std::path::PathBuf> {
        let mut made = tables(out);
        made.push(run_record(out).into());
        made
    }

    /// Every file the run writes: its tables, its record, and the QC report
    /// when QC runs and one is asked for.
    pub fn outputs(&self) -> Vec<std::path::PathBuf> {
        let mut made = Self::outputs_of(&self.out);
        if self.qc.to_config().is_some() {
            made.extend(self.qc.qc_report.as_deref().map(Into::into));
        }
        made
    }
}

pub fn run_peak_to_gene(args: &PeakToGeneArgs) -> anyhow::Result<()> {
    mkdir_parent(&args.out)?;
    // Read before the hours of training, so a wrong file fails at once.
    let groups = args
        .groups
        .as_deref()
        .map(crate::p2g::groups::Groups::read)
        .transpose()?;

    let samples = args.samples()?;
    let gene_names = same_rows(&samples, Side::Rna)?;
    same_rows(&samples, Side::Atac)?;
    let positions = match (&args.gene_coords, &args.gff_file) {
        (Some(tsv), _) => load_gene_coords_tsv(tsv, &gene_names)?,
        (None, Some(gff)) => load_gene_tss(gff, &gene_names)?,
        (None, None) => unreachable!("clap requires --gene-coords or --gff-file"),
    };
    info!(
        "Gene positions: {} of {} RNA genes placed",
        positions.iter().filter(|p| p.is_some()).count(),
        gene_names.len()
    );

    let keep: Option<FxHashSet<Box<str>>> = match args.qc.to_config() {
        Some(cfg) => {
            // Each sample by itself, its cells named as the run names them.
            let mut keep = FxHashSet::default();
            let mut all: Option<(Vec<Box<str>>, QcReport)> = None;
            for s in &samples {
                let rna = open_sparse_matrix_by_path(&s.rna)?;
                let barcodes: Vec<Box<str>> = rna
                    .column_names()?
                    .iter()
                    .map(|b| s.cell(b).into())
                    .collect();
                // A mixed file's genes alone: its peaks are not genes.
                let genes = s.mixed().then(|| rna.row_names()).transpose()?;
                let mut v = SparseIoVec::new();
                v.push(Arc::from(rna), None)?;
                if let Some(names) = genes {
                    let keep: Vec<bool> = peak_rows(&names).iter().map(|p| !p).collect();
                    v.mask_rows(&keep)?;
                }
                let report = data_beans::qc_lib::compute_qc(&v, &cfg, None)?;
                keep.extend(
                    report
                        .emit_idx_unmasked()
                        .into_iter()
                        .map(|c| barcodes[c].clone()),
                );
                all = Some(match all {
                    None => (barcodes, report),
                    Some((mut cells, mut both)) => {
                        cells.extend(barcodes);
                        append(&mut both, report);
                        (cells, both)
                    }
                });
            }
            if let (Some(path), Some((cells, report))) = (&args.qc.qc_report, &all) {
                data_beans::qc_lib::write_qc_report(path, cells, report)?;
                info!("Wrote the per-cell QC report: {path}");
            }
            Some(keep)
        }
        None => None,
    };

    let kernel = AbcKernel {
        window: args.cis_window,
        max_per_gene: args.max_cis,
        gamma: args.contact_gamma,
        pseudocount: args.contact_pseudocount,
    };
    let input = TwoTrackInput {
        samples: &samples,
        batch_file: args.batch.as_deref(),
        gene_positions: &positions,
        kernel: &kernel,
        work_prefix: &args.out,
    };
    let cfg = LinkConfig {
        embed: TwoTrackConfig {
            embedding_dim: args.embedding_dim,
            epochs: args.epochs,
            num_levels: args.num_levels,
            sort_dim: args.sort_dim,
            proj_dim: args.proj_dim,
            feature_modules: args.feature_modules,
            peak_modules: args.peak_modules,
            phase1_cells_per_pb: args.phase1_cells_per_pb,
            module_only_min_rows: args.module_only_min_rows,
            align_weight: args.align_weight,
            mix: args.mix,
            weight_decay: args.weight_decay,
            seed: args.seed,
            device: args.device.clone(),
            device_no: args.device_no,
        },
        n_clusters: args.n_clusters,
        groups,
        ..LinkConfig::default()
    };
    let summary = run_links(&input, &cfg, keep.as_ref())?;
    info!(
        "Done: {} genes, {} peaks, {} cis pairs, {} groups → {}",
        summary.n_genes,
        summary.n_peaks,
        summary.n_pairs,
        summary.n_clusters,
        table(&args.out, LINKS)
    );
    write_run_record(&args.out, &samples, args.groups.as_deref(), args.flags())
}

/// `more`'s cells after `report`'s: the samples' reports as one, of the
/// same genes.
fn append(report: &mut QcReport, more: QcReport) {
    report.train_keep.extend(more.train_keep);
    report.near_empty.extend(more.near_empty);
    report.n_genes.extend(more.n_genes);
    report.total_counts.extend(more.total_counts);
    for (to, from) in [
        (&mut report.mito_frac, more.mito_frac),
        (&mut report.ribo_frac, more.ribo_frac),
    ] {
        if let (Some(to), Some(from)) = (to.as_mut(), from) {
            to.extend(from);
        }
    }
    report.n_cells_dropped += more.n_cells_dropped;
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::FromArgMatches;

    #[test]
    fn the_record_keeps_every_flag_but_the_files_device_and_reports() {
        let cmd = PeakToGeneArgs::augment_args(clap::Command::new("p2g"));
        let m = cmd
            .clone()
            .try_get_matches_from([
                "p2g",
                "--rna",
                "r",
                "--atac",
                "a",
                "--gene-coords",
                "g",
                "-o",
                "o",
            ])
            .unwrap();
        let flags = PeakToGeneArgs::from_arg_matches(&m).unwrap().flags();
        let not = [
            "data",
            "rna",
            "atac",
            "groups",
            "out",
            "device",
            "device-no",
            "qc-report",
            "qc-histogram",
            "help",
        ];
        // Each kept as its own value, the default clap gave it.
        let same = |raw: Option<&str>, v: &serde_json::Value| match (raw, v) {
            (None, serde_json::Value::Null) => true,
            (Some(r), serde_json::Value::String(s)) => r == s,
            (Some(r), serde_json::Value::Bool(b)) => r.parse() == Ok(*b),
            (Some(r), serde_json::Value::Number(n)) => r
                .parse::<f64>()
                .is_ok_and(|x| (x - n.as_f64().unwrap()).abs() <= 1e-6 * x.abs().max(1.0)),
            _ => false,
        };
        for arg in cmd.get_arguments() {
            let Some(long) = arg.get_long() else { continue };
            let key = long.replace('-', "_");
            let kept = flags.get(&key).or_else(|| flags["qc"].get(&key));
            assert_eq!(kept.is_some(), !not.contains(&long), "--{long}");
            if let Some(v) = kept {
                let raw = m
                    .get_raw(arg.get_id().as_str())
                    .and_then(|mut r| r.next())
                    .and_then(|r| r.to_str());
                assert!(same(raw, v), "--{long}: {raw:?} kept as {v}");
            }
        }
    }
}
