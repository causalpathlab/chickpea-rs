//! `chickpea view`'s reading of a finished run.

use crate::common::{chickpea, gene_names, small, write_fixture, write_gene_coords};
use chickpea::view::data::{Measure, Run};
use serde_json::{json, Value};

/// A run on the fixture, its genes reaching their own peaks only.
fn run(dir: &std::path::Path) -> Run {
    Run::open(&run_at(dir)).unwrap()
}

/// [`run`], by its out prefix.
fn run_at(dir: &std::path::Path) -> String {
    let (rna, atac) = write_fixture(dir);
    let genes = write_gene_coords(dir);
    let out = dir.join("run").to_string_lossy().into_owned();
    let st = chickpea()
        .args(["peak-to-gene", "--rna", &rna, "--atac", &atac])
        .args(["--gene-coords", &genes, "--cis-window", "10000", "-o", &out])
        .args(small())
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    out
}

#[test]
fn a_genes_activity_is_its_peaks_pooled_through_their_gates() {
    let dir = tempfile::tempdir().unwrap();
    let run = run(dir.path());
    let g = run.gene(&gene_names()[5]).unwrap();
    // Wide enough for every candidate of the gene.
    let locus = run.locus(g, Measure::Score, Some(50_000)).unwrap();
    assert_eq!(locus.links.len(), run.n_candidates(g));
    assert!(locus.rows.len() > 1, "groups to compare");
    assert!(
        locus.rows.iter().any(|r| r.activity > 0.0),
        "the gene's peaks open somewhere"
    );
    for row in &locus.rows {
        let pooled: f32 = locus
            .links
            .iter()
            .map(|l| {
                let p = locus.peaks.iter().position(|(n, _)| *n == l.peak).unwrap();
                l.gate * row.rate[p]
            })
            .sum();
        assert!(
            (row.activity - pooled).abs() <= 1e-5 * pooled.max(1e-6),
            "{}: {} against {pooled}",
            row.label,
            row.activity
        );
    }
}

#[test]
fn a_genes_neighbours_are_the_nearest_linked_genes_either_way() {
    let dir = tempfile::tempdir().unwrap();
    let run = run(dir.path());
    let names = gene_names();
    // The first of `at` the run kept.
    let kept = |at: &mut dyn Iterator<Item = usize>| {
        for i in at {
            if let Some(g) = run.gene(&names[i]) {
                return Some(g);
            }
        }
        None
    };
    let g = run.gene(&names[5]).unwrap();
    // A gene the run did not keep is stepped over.
    assert_eq!(run.neighbour(g, 1), kept(&mut (6..names.len())));
    assert_eq!(run.neighbour(g, -1), kept(&mut (0..5).rev()));
    // The locus names the genes ← and → step to.
    let locus = run.locus(g, Measure::Score, Some(250_000)).unwrap();
    let name = |o: Option<usize>| o.map(|o| run.genes[o].clone());
    assert_eq!(
        locus.steps,
        [name(run.neighbour(g, -1)), name(run.neighbour(g, 1))]
    );
    let first = kept(&mut (0..names.len())).unwrap();
    assert_eq!(run.neighbour(first, -1), None, "none before the first");
}

#[test]
fn a_refused_locus_call_changes_no_setting() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_at(dir.path());
    let names = gene_names();
    let mut s = crate::mcp::Session::start(dir.path());
    s.call("view_open", json!({"prefix": out})).unwrap();
    let text = |mut args: Value| {
        args["format"] = json!("text");
        args
    };
    let set = s
        .call(
            "view_locus",
            text(json!({"gene": &*names[5], "cutoff": 0.2})),
        )
        .unwrap()["settings"]
        .clone();
    for refused in [
        json!({"gene": &*names[7], "cutoff": 0.5, "layout": "tree"}),
        json!({"measure": "gate", "cutoff": 0.5, "gene": "NO_SUCH_GENE"}),
    ] {
        assert!(
            s.call("view_locus", text(refused.clone())).is_err(),
            "{refused}"
        );
        let now = s.call("view_locus", text(json!({}))).unwrap()["settings"].clone();
        assert_eq!(now, set, "after {refused}");
    }
}
