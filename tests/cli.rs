//! `chickpea peak-to-gene` as a user runs it.

mod common;

use common::{write_fixture, write_gene_coords};
use std::process::Command;

fn chickpea() -> Command {
    Command::new(env!("CARGO_BIN_EXE_chickpea"))
}

/// [`common::SMALL`] as command-line arguments.
fn small() -> Vec<String> {
    common::SMALL
        .iter()
        .flat_map(|(flag, value)| [format!("--{flag}"), value.to_string()])
        .collect()
}

#[test]
fn peak_to_gene_runs_end_to_end_and_writes_the_link_tables() {
    let dir = tempfile::tempdir().unwrap();
    let (rna, atac) = write_fixture(dir.path());
    let genes = write_gene_coords(dir.path());
    let out = dir.path().join("run").to_string_lossy().into_owned();
    let st = chickpea()
        .args([
            "peak-to-gene",
            "--rna",
            &rna,
            "--atac",
            &atac,
            "--gene-coords",
            &genes,
        ])
        .args(["--cis-window", "10000", "-o", &out])
        .args(small())
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    for stem in [
        "links",
        "links_by_cluster",
        "cell_clusters",
        "peaks",
        "gene_embedding",
    ] {
        let f = format!("{out}.{stem}.parquet");
        assert!(std::path::Path::new(&f).exists(), "missing {f}");
    }
}

#[test]
fn gene_positions_are_required() {
    let dir = tempfile::tempdir().unwrap();
    let (rna, atac) = write_fixture(dir.path());
    let out = dir.path().join("run").to_string_lossy().into_owned();
    let st = chickpea()
        .args(["peak-to-gene", "--rna", &rna, "--atac", &atac, "-o", &out])
        .output()
        .unwrap();
    assert!(!st.status.success());
    let err = String::from_utf8_lossy(&st.stderr);
    assert!(err.contains("gene-coords") && err.contains("gff"), "{err}");
}

#[test]
fn qc_report_is_written_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let (rna, atac) = write_fixture(dir.path());
    let genes = write_gene_coords(dir.path());
    let out = dir.path().join("run").to_string_lossy().into_owned();
    let report = dir.path().join("qc.tsv");
    let st = chickpea()
        .args(["peak-to-gene", "--rna", &rna, "--atac", &atac])
        .args(["--gene-coords", &genes, "--cis-window", "10000", "-o", &out])
        .arg(format!("--qc-report={}", report.display()))
        .args(small())
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let table = std::fs::read_to_string(&report).expect("the QC report");
    assert!(table.starts_with("#cell\t"), "{table}");
    assert_eq!(table.lines().count(), 1 + common::N_CELLS);
}
