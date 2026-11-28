//! Shared planted multiome fixture: two cell programs over genes with two
//! peaks each, written as tiny RNA and ATAC zarrs.

use chickpea::p2g::cis::AbcKernel;
use chickpea::p2g::samples::Sample;
use chickpea::p2g::two_track::{TwoTrackConfig, TwoTrackInput};
use data_beans::sparse_io::{create_sparse_from_triplets, SparseIoBackend};
use genomic_data::coordinates::GeneTss;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

/// Flags that keep a run on the fixture small, as `(flag, value)`: the
/// command line's [`small_config`].
pub const SMALL: [(&str, &str); 9] = [
    ("embedding-dim", "8"),
    ("epochs", "60"),
    ("num-levels", "2"),
    ("sort-dim", "3"),
    ("proj-dim", "8"),
    ("feature-modules", "4"),
    ("peak-modules", "4"),
    ("phase1-cells-per-pb", "4"),
    ("module-only-min-rows", "30"),
];

/// An embedding small enough for the fixture, as [`SMALL`] spells it.
pub fn small_config() -> TwoTrackConfig {
    TwoTrackConfig {
        embedding_dim: 8,
        epochs: 60,
        num_levels: 2,
        sort_dim: 3,
        proj_dim: 8,
        feature_modules: 4,
        peak_modules: 4,
        phase1_cells_per_pb: 4,
        // 24 genes < 30 ≤ 48 peaks → ATAC module-only, RNA keeps residuals.
        module_only_min_rows: 30,
        ..TwoTrackConfig::default()
    }
}

/// What a [`TwoTrackInput`] on the fixture borrows: one sample, the genes'
/// positions, [`kernel`], and the output prefix `{dir}/run`.
pub struct Fixture {
    samples: Vec<Sample>,
    /// The RNA and ATAC rows, as the samples have them.
    rows: [Vec<Box<str>>; 2],
    genes: Vec<Option<GeneTss>>,
    kernel: AbcKernel,
    pub work: String,
}

impl Fixture {
    /// The sample `(rna, atac)`, as [`write_fixture`] returns it, with genes
    /// at `genes`.
    pub fn new(
        dir: &std::path::Path,
        (rna, atac): (String, String),
        genes: Vec<Option<GeneTss>>,
    ) -> Self {
        use chickpea::p2g::samples::{pair, same_rows, Side};
        let samples = pair(&[rna.into()], &[atac.into()]).unwrap();
        let rows = [Side::Rna, Side::Atac].map(|side| same_rows(&samples, side).unwrap());
        Self {
            samples,
            rows,
            genes,
            kernel: kernel(),
            work: dir.join("run").to_string_lossy().into_owned(),
        }
    }

    pub fn input(&self) -> TwoTrackInput<'_> {
        TwoTrackInput {
            samples: &self.samples,
            genes: &self.rows[0],
            peaks: &self.rows[1],
            batch_file: None,
            gene_positions: &self.genes,
            kernel: &self.kernel,
            work_prefix: &self.work,
        }
    }
}

/// The chickpea binary these tests are built with.
pub fn chickpea() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_chickpea"))
}

/// [`SMALL`] as command-line arguments.
pub fn small() -> Vec<String> {
    SMALL
        .iter()
        .flat_map(|(flag, value)| [format!("--{flag}"), value.to_string()])
        .collect()
}

/// Fails the test with the child's stderr unless it succeeded.
pub fn assert_ok(st: &std::process::Output) {
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
}

/// `chickpea peak-to-gene` on the fixture written to `dir`, with [`SMALL`],
/// a window that reaches each gene's own peaks only, then `extra`; the out
/// prefix.
pub fn run_p2g(dir: &std::path::Path, extra: &[&str]) -> String {
    let (rna, atac) = write_fixture(dir);
    let genes = write_gene_coords(dir);
    let out = dir.join("run").to_string_lossy().into_owned();
    let st = chickpea()
        .args(["peak-to-gene", "--rna", &rna, "--atac", &atac])
        .args(["--gene-coords", &genes, "--cis-window", "10000", "-o", &out])
        .args(small())
        .args(extra)
        .output()
        .unwrap();
    assert_ok(&st);
    out
}

pub const N_GENES: usize = 24;
pub const N_CELLS: usize = 120;
/// Two peaks per gene: one at the TSS, one 3 kb upstream.
pub const PEAKS_PER_GENE: usize = 2;

/// Gene `g` belongs to program `g % 2`; cell `c` to program `c % 2`.
pub fn program_of(i: usize) -> usize {
    i % 2
}

pub fn tss(g: usize) -> i64 {
    100_000 * (g as i64 + 1)
}

pub fn names(prefix: &str, n: usize) -> Vec<Box<str>> {
    (1..=n).map(|i| format!("{prefix}{i}").into()).collect()
}

/// Writes `{dir}/rna.zarr` and `{dir}/atac.zarr` for the same barcodes.
pub fn write_fixture(dir: &std::path::Path) -> (String, String) {
    write_fixture_with(dir, &[], |_| 0.0)
}

/// [`write_fixture`] with `extra` genes after the planted ones, each
/// counting `extra_count(cell)` in every cell (no peaks, no position).
pub fn write_fixture_with(
    dir: &std::path::Path,
    extra: &[&str],
    extra_count: impl Fn(usize) -> f32,
) -> (String, String) {
    let mut rng = StdRng::seed_from_u64(7);
    let mut count = |on: bool| -> f32 {
        let mean = if on { 8.0 } else { 0.3 };
        // Small deterministic Poisson-like draw.
        let u: f32 = rng.random();
        (mean * (0.5 + u)).floor()
    };
    let barcodes = names("CELL", N_CELLS);

    let mut rna = Vec::new();
    for c in 0..N_CELLS {
        for g in 0..N_GENES {
            let v = count(program_of(g) == program_of(c));
            if v > 0.0 {
                rna.push((g as u64, c as u64, v));
            }
        }
        for e in 0..extra.len() {
            let v = extra_count(c);
            if v > 0.0 {
                rna.push(((N_GENES + e) as u64, c as u64, v));
            }
        }
    }
    let n_peaks = N_GENES * PEAKS_PER_GENE;
    let mut atac = Vec::new();
    for c in 0..N_CELLS {
        for p in 0..n_peaks {
            let v = count(program_of(p / PEAKS_PER_GENE) == program_of(c));
            if v > 0.0 {
                atac.push((p as u64, c as u64, v));
            }
        }
    }
    let peak_names: Vec<Box<str>> = (0..n_peaks)
        .map(|p| {
            let mid = tss(p / PEAKS_PER_GENE) - 3_000 * (p % PEAKS_PER_GENE) as i64;
            format!("chr1:{}-{}", mid - 250, mid + 250).into()
        })
        .collect();

    let write = |name: &str, trip: &[(u64, u64, f32)], rows: &[Box<str>]| -> String {
        let path = dir.join(name).to_string_lossy().into_owned();
        let mut m = create_sparse_from_triplets(
            trip,
            (rows.len(), N_CELLS, trip.len()),
            Some(&path),
            Some(&SparseIoBackend::Zarr),
        )
        .unwrap();
        m.register_row_names_vec(rows);
        m.register_column_names_vec(&barcodes);
        path
    };
    let mut rna_names = gene_names();
    rna_names.extend(extra.iter().map(|&e| e.into()));
    (
        write("rna.zarr", &rna, &rna_names),
        write("atac.zarr", &atac, &peak_names),
    )
}

/// Every gene at its TSS on chr1.
pub fn gene_positions() -> Vec<Option<GeneTss>> {
    (0..N_GENES)
        .map(|g| {
            Some(GeneTss {
                chr: "chr1".into(),
                tss: tss(g),
            })
        })
        .collect()
}

/// A window that reaches each gene's own two peaks only.
pub fn kernel() -> AbcKernel {
    AbcKernel {
        window: 10_000,
        ..AbcKernel::default()
    }
}

/// The index after the name prefix, 0-based: GENE3 -> 2.
pub fn index(name: &str) -> usize {
    name.trim_start_matches(|ch: char| !ch.is_ascii_digit())
        .parse::<usize>()
        .unwrap()
        - 1
}

/// `{dir}/genes.tsv`: `gene chr tss` with a header, as the simulator writes.
pub fn write_gene_coords(dir: &std::path::Path) -> String {
    let path = dir.join("genes.tsv").to_string_lossy().into_owned();
    let mut s = String::from("gene\tchr\ttss\n");
    for (g, name) in gene_names().iter().enumerate() {
        s.push_str(&format!("{name}\tchr1\t{}\n", tss(g)));
    }
    std::fs::write(&path, s).unwrap();
    path
}

/// `GENE_1`, `GENE_2`, …: an underscore, as simulator and 10x names carry, so
/// every test covers names a gene-symbol canonicalizer would rewrite.
pub fn gene_names() -> Vec<Box<str>> {
    names("GENE_", N_GENES)
}
