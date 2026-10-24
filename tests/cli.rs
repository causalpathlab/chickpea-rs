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

/// `{dir}/{name}.rna.zarr` and `{dir}/{name}.atac.zarr`: the fixture, as
/// sample `name`'s files.
fn write_sample(dir: &std::path::Path, name: &str) -> (String, String) {
    let tmp = dir.join(format!("{name}.tmp"));
    std::fs::create_dir_all(&tmp).unwrap();
    let (rna, atac) = write_fixture(&tmp);
    let to = |from: &str, side: &str| -> String {
        let p = dir.join(format!("{name}.{side}.zarr"));
        std::fs::rename(from, &p).unwrap();
        p.to_string_lossy().into_owned()
    };
    (to(&rna, "rna"), to(&atac, "atac"))
}

fn links_by_cluster(out: &str) -> Vec<Vec<f64>> {
    let (_, n) = legume_numeric::matrix::parquet::read_table_columns(
        &format!("{out}.links_by_cluster.parquet"),
        &[],
        &["gene_idx", "peak_idx", "cluster", "gate", "abc"],
    )
    .unwrap();
    n
}

fn regroup(args: &[&str]) -> std::process::Output {
    chickpea().arg("regroup").args(args).output().unwrap()
}

#[test]
fn regroup_reads_each_samples_atac_from_the_run_record() {
    let dir = tempfile::tempdir().unwrap();
    let (rna_a, atac_a) = write_sample(dir.path(), "a");
    let (rna_b, atac_b) = write_sample(dir.path(), "b");
    let genes = write_gene_coords(dir.path());
    let out = dir.path().join("run").to_string_lossy().into_owned();
    let st = chickpea()
        .args(["peak-to-gene", "--rna", &rna_a, &rna_b])
        .args(["--atac", &atac_a, &atac_b])
        .args(["--gene-coords", &genes, "--cis-window", "10000", "-o", &out])
        .args(small())
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{out}.run.json")).unwrap()).unwrap();
    let samples = record["samples"].as_array().expect("the samples");
    assert_eq!(samples[0]["name"], "a");
    assert_eq!(samples[1]["atac"], "b.atac.zarr");

    // The run's own groups again, its ATAC found in the record.
    let groups = format!("{out}.cell_clusters.parquet");
    let same = dir.path().join("same").to_string_lossy().into_owned();
    let st = regroup(&["--run", &out, "--groups", &groups, "-o", &same]);
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    assert_eq!(links_by_cluster(&same), links_by_cluster(&out));

    // A sample's ATAC moved: refused by name, then given by its name.
    let moved = dir.path().join("moved");
    std::fs::create_dir_all(&moved).unwrap();
    let b_moved = moved.join("b.atac.zarr").to_string_lossy().into_owned();
    std::fs::rename(&atac_b, &b_moved).unwrap();
    let again = dir.path().join("again").to_string_lossy().into_owned();
    let st = regroup(&["--run", &out, "--groups", &groups, "-o", &again]);
    assert!(!st.status.success());
    let err = String::from_utf8_lossy(&st.stderr);
    assert!(err.contains("sample b") && err.contains("--atac"), "{err}");
    let st = regroup(&[
        "--run", &out, "--atac", &b_moved, "--groups", &groups, "-o", &again,
    ]);
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    assert_eq!(links_by_cluster(&again), links_by_cluster(&out));
    let read: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{again}.regroup.json")).unwrap())
            .unwrap();
    assert_eq!(read["atac"][0], "a.atac.zarr");
    assert_eq!(
        read["atac"][1], "moved/b.atac.zarr",
        "b's file in b's place"
    );

    // A file that names none of the run's samples is refused.
    let other = dir.path().join("other").to_string_lossy().into_owned();
    let c = dir
        .path()
        .join("c.atac.zarr")
        .to_string_lossy()
        .into_owned();
    let st = regroup(&[
        "--run", &out, "--atac", &c, "--groups", &groups, "-o", &other,
    ]);
    assert!(!st.status.success());
    let err = String::from_utf8_lossy(&st.stderr);
    assert!(err.contains("names none of the run's samples"), "{err}");
}
