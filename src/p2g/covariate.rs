//! `--covariate`: per-cell covariates (cell cycle, mito and ribo shares,
//! sex, stress, hypoxia, interferon) as a second axis of batch, by the
//! shared [`genomic_data::covariate`] (the same flags and model as senna
//! and pinto). Each cell is scored on its RNA rows alone, so its peaks
//! neither dilute a share nor join the control pool, and its batch becomes
//! `batch|cvK`, so the fit's per-gene δ covers the bins with no change to
//! the model. Every loaded cell is binned, QC-failed ones too: they are in
//! the fit, and so in its δ.

use super::two_track::RNA_TAG;
use crate::common::*;
use data_beans::alg::random_projection::RandProjOps;
use data_beans::bipartite_graph_embedding_input::BipartiteEmbeddingCells;
use genomic_data::covariate::{
    files, plain_batch, score_covariates, ColumnBlocks, CovariateArgs, Modelled,
};
use nalgebra_sparse::CscMatrix;
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};

/// Columns read at a time when scoring and projecting.
const BLOCK: usize = 1024;

/// The unified counts as the covariate module reads them.
struct Blocks<'a>(&'a SparseIoVec);

impl ColumnBlocks for Blocks<'_> {
    fn n_cols(&self) -> usize {
        self.0.num_columns()
    }
    fn row_names(&self) -> anyhow::Result<Vec<Box<str>>> {
        self.0.row_names()
    }
    fn block(&self, lb: usize, ub: usize) -> anyhow::Result<CscMatrix<f32>> {
        self.0.read_columns_csc(lb..ub)
    }
}

/// Every covariate file a run at `out` may write, whether or not it bins
/// (the names the MCP prefix guard checks).
#[must_use]
pub fn outputs_of(out: &str) -> [PathBuf; 3] {
    [files::model(out), files::cells(out), files::effect(out)].map(Into::into)
}

/// The rows of `names` on the RNA side (`{gene}/rna`): the rows scored.
#[must_use]
pub fn rna_rows(names: &[Box<str>]) -> Vec<usize> {
    names
        .iter()
        .enumerate()
        .filter(|(_, n)| n.rsplit_once('/').is_some_and(|(_, tag)| tag == RNA_TAG))
        .map(|(r, _)| r)
        .collect()
}

/// Model `args`' covariates on `unified`'s cells and give each its
/// `batch|cvK` label in place of its batch (the condition axis too, when it
/// copied the batch axis). The cells are scored first; only when some
/// covariate scores is the projection the like-cell guard reads made,
/// seeded by `seed`. Returns the model, to be written by [`write`] once the
/// fit has succeeded; `None`, the batches as they were, when no cell is
/// binned (the reason logged).
///
/// # Errors
/// A malformed `--covariate`, a covariate named that cannot be modelled,
/// or counts that cannot be read.
pub fn relabel_batches(
    unified: &mut BipartiteEmbeddingCells,
    args: &CovariateArgs,
    proj_dim: usize,
    seed: u64,
) -> anyhow::Result<Option<Modelled>> {
    if !args.is_on()? {
        return Ok(None);
    }
    // Labels with parts of their own are checked first, before any counts
    // are read (each distinct label once).
    if let Err(e) = unified
        .batch_names
        .iter()
        .try_for_each(|b| plain_batch(b).map(drop))
    {
        args.unsupported(&e.to_string())?;
        return Ok(None);
    }
    let plain = unified.batch_labels();
    let data = unified.count_backend();
    let rna = rna_rows(&data.row_names()?);
    if rna.is_empty() {
        args.unsupported("no RNA rows to score")?;
        return Ok(None);
    }
    // Scored on a view of the RNA rows alone, so no block carries the peaks.
    let mut genes = data.clone_for_collapse();
    let mut keep = vec![false; data.num_rows()];
    rna.iter().for_each(|&r| keep[r] = true);
    genes.mask_rows(&keep)?;
    let Some(scored) = score_covariates(&Blocks(&genes), args, None, BLOCK)? else {
        return Ok(None);
    };
    drop(genes);
    // Over every row, peaks too: the like-cell groups are the cells' states.
    let proj_kn = data
        .project_columns_with_batch_correction_seeded(proj_dim, Some(BLOCK), Some(&plain), seed)?
        .proj;
    let Some(modelled) = scored.bin(&plain, &proj_kn)? else {
        return Ok(None);
    };

    // A condition axis that copied the batch axis keeps copying it.
    let condition_was_batch = unified.condition_names == unified.batch_names;
    (unified.batch_membership, unified.batch_names) = intern(&modelled.labels);
    if condition_was_batch {
        unified
            .condition_membership
            .clone_from(&unified.batch_membership);
        unified.condition_names.clone_from(&unified.batch_names);
    }
    Ok(Some(modelled))
}

/// Each label's index among the distinct labels, in first-seen order, and
/// those labels.
fn intern(labels: &[Box<str>]) -> (Vec<u32>, Vec<Box<str>>) {
    let mut index: FxHashMap<&str, u32> = FxHashMap::default();
    let mut names: Vec<Box<str>> = Vec::new();
    let membership = labels
        .iter()
        .map(|l| {
            *index.entry(l).or_insert_with(|| {
                names.push(l.clone());
                (names.len() - 1) as u32
            })
        })
        .collect();
    (membership, names)
}

/// After a fit succeeds: the covariate files at `out` replaced by this
/// run's. With `modelled`, the model and every cell's scores and label
/// (`cells` its cell names), and each RNA gene's covariate effect from the
/// fit's per-batch gene fold (`genes` and their rows on its feature axis);
/// without, an earlier run's removed, so none reads as this one's.
///
/// # Errors
/// A file that cannot be written or removed.
pub fn write(
    out: &str,
    modelled: Option<&Modelled>,
    cells: &[Box<str>],
    fold: Option<&graph_embedding_util::fit::BatchGeneFold>,
    genes: &[Box<str>],
    rows: &[usize],
) -> anyhow::Result<()> {
    for old in files::clear_stale(out)? {
        info!("--covariate: removed an earlier run's {old}");
    }
    let Some(m) = modelled else {
        return Ok(());
    };
    m.model.write(Path::new(&files::model(out)))?;
    genomic_data::covariate::write_cells(
        Path::new(&files::cells(out)),
        &m.model,
        cells,
        &m.scores,
        &m.labels,
    )?;
    info!(
        "--covariate: {} batch levels; wrote {} and {}",
        m.model.levels.len(),
        files::model(out),
        files::cells(out)
    );
    let Some(fold) = fold else {
        info!("--covariate: the fit kept no per-batch gene fold, so no covariate effect table");
        return Ok(());
    };
    let log_delta = nalgebra::DMatrix::<f32>::from_fn(genes.len(), fold.n_batches(), |g, b| {
        fold.row(b)[rows[g]].ln()
    });
    let path = files::effect(out);
    if genomic_data::covariate::write_covariate_effect(
        Path::new(&path),
        genes,
        &fold.batch_names,
        &log_delta,
    )? {
        info!("--covariate: wrote each gene's covariate effect to {path}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_rna_rows_are_scored() {
        let names: Vec<Box<str>> = ["MT-ND1/rna", "chr1:10-20/atac", "XIST/rna", "odd"]
            .into_iter()
            .map(Into::into)
            .collect();
        assert_eq!(rna_rows(&names), [0, 2]);
    }

    #[test]
    fn labels_are_numbered_as_first_seen() {
        let labels: Vec<Box<str>> = ["b|cv1", "a", "b|cv1", "a|cv0"].map(Into::into).into();
        let (membership, names) = intern(&labels);
        assert_eq!(membership, [0, 1, 0, 2]);
        assert_eq!(
            names,
            labels[..2]
                .iter()
                .chain(&labels[3..])
                .cloned()
                .collect::<Vec<_>>()
        );
    }
}
