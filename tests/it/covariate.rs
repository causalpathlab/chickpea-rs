//! `--covariate` on the planted fixture: cells with a planted mitochondrial
//! share get bins of their own, scored on the RNA rows alone.

use crate::common::{gene_positions, kernel, write_fixture_with, N_CELLS};
use chickpea::p2g::two_track::{embed_two_track, TwoTrackConfig, TwoTrackInput};
use genomic_data::covariate::{files, CovariateArgs, CovariateModel};

/// The 13 mitochondrially encoded protein-coding genes (HGNC group 1974).
const MITO: [&str; 13] = [
    "MT-ND1", "MT-ND2", "MT-CO1", "MT-CO2", "MT-ATP8", "MT-ATP6", "MT-CO3", "MT-ND3", "MT-ND4L",
    "MT-ND4", "MT-ND5", "MT-ND6", "MT-CYB",
];

/// Every other run of three cells is high in mito, across both programs
/// (`c % 2`), so the share is no cell type of its own.
fn high_mito(c: usize) -> bool {
    (c / 3) % 2 == 1
}

/// [`try_embed`], unwrapped.
fn embed(dir: &std::path::Path, epochs: usize, covariate: CovariateArgs) -> (String, usize) {
    try_embed(dir, epochs, covariate).unwrap()
}

/// The fixture with [`MITO`] planted, embedded with `covariate`; the run's
/// output prefix and its cell rows.
fn try_embed(
    dir: &std::path::Path,
    epochs: usize,
    covariate: CovariateArgs,
) -> anyhow::Result<(String, usize)> {
    let (rna, atac) = write_fixture_with(dir, &MITO, |c| if high_mito(c) { 20.0 } else { 1.0 });
    let mut genes = gene_positions();
    genes.extend(MITO.iter().map(|_| None));
    let work = dir.join("run").to_string_lossy().into_owned();
    let cfg = TwoTrackConfig {
        embedding_dim: 8,
        epochs,
        num_levels: 2,
        sort_dim: 3,
        proj_dim: 8,
        feature_modules: 4,
        peak_modules: 4,
        phase1_cells_per_pb: 4,
        module_only_min_rows: 30,
        covariate,
        ..TwoTrackConfig::default()
    };
    let samples =
        chickpea::p2g::samples::pair(&[rna.as_str().into()], &[atac.as_str().into()]).unwrap();
    let input = TwoTrackInput {
        samples: &samples,
        batch_file: None,
        gene_positions: &genes,
        kernel: &kernel(),
        work_prefix: &work,
    };
    let out = embed_two_track(&input, &cfg)?;
    Ok((work, out.cell_rows.nrows()))
}

#[test]
fn a_planted_mito_share_takes_bins_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let all = CovariateArgs {
        covariates: vec!["all".into()],
        ..CovariateArgs::default()
    };
    let (work, n_cells) = embed(dir.path(), 20, all);
    assert_eq!(n_cells, N_CELLS);

    let model = CovariateModel::read(std::path::Path::new(&files::model(&work)))
        .unwrap()
        .expect("a covariate model is recorded");
    assert!(
        !model.rows.is_empty() && model.rows.iter().all(|r| r.ends_with("/rna")),
        "scored on the RNA rows only: {:?}",
        model.rows
    );
    let binned: Vec<&str> = model
        .bins
        .binned
        .iter()
        .map(|&j| model.functionals[j].label.as_ref())
        .collect();
    assert!(binned.contains(&"mito"), "binned {binned:?}");
    let sizes: Vec<usize> = model
        .levels
        .iter()
        .filter(|(l, _)| l.contains("|cv"))
        .map(|(_, &n)| n)
        .collect();
    // Two bins of about half each (60 planted high, 60 low); the like-cell
    // guard may leave a few cells plain, how many varying by machine.
    let plain = N_CELLS - sizes.iter().sum::<usize>();
    assert!(
        sizes.len() == 2
            && sizes
                .iter()
                .all(|&n| (40 * N_CELLS..=55 * N_CELLS).contains(&(n * 100)))
            && plain * 10 <= N_CELLS,
        "levels {:?}",
        model.levels
    );
    assert!(std::path::Path::new(&files::cells(&work)).is_file());
    assert!(std::path::Path::new(&files::effect(&work)).is_file());
}

#[test]
fn none_models_nothing_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (work, _) = embed(dir.path(), 5, CovariateArgs::default());
    assert!(!std::path::Path::new(&files::model(&work)).exists());
}

#[test]
fn a_named_covariate_without_its_genes_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let stress = CovariateArgs {
        covariates: vec!["stress".into()],
        ..CovariateArgs::default()
    };
    let err = try_embed(dir.path(), 5, stress).unwrap_err().to_string();
    assert!(err.contains("stress"), "{err}");
}
