//! A finished peak-to-gene run, as the viewer reads it: its links by gene,
//! each gene's place on the genome, and summaries for the genome-wide view.
//!
//! What the TUI, the MCP tools and the figures draw all comes from here, so
//! they show the same numbers.

use crate::p2g::workflow::{table, CELL_CLUSTERS, GENES, GROUPS, LINKS, PEAKS, PEAK_RATES};
use anyhow::Context;
use genomic_data::coordinates::chr_stripped;
use legume_numeric::matrix::parquet::read_table_columns;
use rustc_hash::FxHashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A link's numbers, as a run writes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Measure {
    /// `gate × corr`.
    Score,
    /// The gene's share of the pair under the trained distance prior.
    Gate,
    /// Correlation across pseudobulks of the peak module's score and the gene's.
    Corr,
    /// The fixed contact prior's share.
    Abc,
}

impl Measure {
    pub const ALL: [Measure; 4] = [Measure::Score, Measure::Gate, Measure::Corr, Measure::Abc];

    /// The measure after it in [`Measure::ALL`], round to the first.
    #[must_use]
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&m| m == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// What a locus figure's links show by it (each caller says whether it
    /// is a row's own: corr is not a share of anything).
    #[must_use]
    pub fn shown(self) -> &'static str {
        match self {
            Measure::Corr => "corr, where the peak is open",
            Measure::Abc => "ABC prior × accessibility",
            Measure::Gate => "gate × accessibility",
            Measure::Score => "gate × accessibility × corr",
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Measure::Score => "score",
            Measure::Gate => "gate",
            Measure::Corr => "corr",
            Measure::Abc => "abc",
        }
    }

    pub fn parse(s: &str) -> anyhow::Result<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.name() == s)
            .with_context(|| format!("measure {s:?}: one of score, gate, corr, abc"))
    }
}

/// How a gene's links are summarised in one number, for the genome-wide
/// view. A sum over a gene's links is left out on purpose: it grows with
/// the number of candidates, so it would map gene density.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Summary {
    /// The gene's best link.
    Best,
    /// Its links at or above a cutoff.
    Count(f32),
}

impl Summary {
    /// In a few words, led by the word MCP's `summary` takes: `best`,
    /// `count ≥ 0.1`.
    #[must_use]
    pub fn name(self) -> String {
        match self {
            Summary::Best => "best".into(),
            Summary::Count(c) => format!("count ≥ {c}"),
        }
    }
}

/// A peak's place: chromosome, start and end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub chr: Box<str>,
    pub start: i64,
    pub end: i64,
}

impl Place {
    #[must_use]
    pub fn mid(&self) -> i64 {
        (self.start + self.end) / 2
    }
}

/// A gene's place: chromosome and TSS. `inferred` when the run kept no
/// genes table and it was worked out from the gene's peaks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tss {
    pub chr: Box<str>,
    pub tss: i64,
    pub inferred: bool,
}

/// One link of a gene's locus.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub peak: Box<str>,
    pub place: Option<Place>,
    pub distance: i64,
    pub score: f32,
    pub gate: f32,
    pub corr: f32,
    pub abc: f32,
}

impl Link {
    #[must_use]
    pub fn get(&self, m: Measure) -> f32 {
        match m {
            Measure::Score => self.score,
            Measure::Gate => self.gate,
            Measure::Corr => self.corr,
            Measure::Abc => self.abc,
        }
    }
}

/// The genome laid end to end, and each linked gene on it, as the
/// genome-wide figure draws them.
#[derive(Clone, Debug, PartialEq)]
pub struct Genome {
    /// Each chromosome: its name, where it starts end to end, its length
    /// (to its last gene or peak).
    pub chroms: Vec<(Box<str>, u64, u64)>,
    /// Each placed gene: its index in [`Run::genes`], its place end to end
    /// and its value.
    pub points: Vec<(usize, u64, f32)>,
    pub measure: Measure,
    pub summary: Summary,
}

impl Genome {
    /// Its length, end to end.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.chroms.last().map_or(0, |(_, o, l)| o + l)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The chromosome (its index in [`Genome::chroms`]) at `pos` end to end.
    #[must_use]
    pub fn chrom_at(&self, pos: u64) -> usize {
        self.chroms
            .partition_point(|(_, o, _)| *o <= pos)
            .saturating_sub(1)
    }

    /// Each point in sight of `view`.
    pub fn in_view<'a>(
        &'a self,
        view: &super::figure::GenomeView,
    ) -> impl Iterator<Item = &'a (usize, u64, f32)> {
        let len = self.len().max(1) as f32;
        let (from, span) = view.window();
        self.points.iter().filter(move |p| {
            let t = p.1 as f32 / len;
            t >= from && t <= from + span
        })
    }

    /// The points in sight of `view` with a value above 0, best first.
    #[must_use]
    pub fn best_in_view(&self, view: &super::figure::GenomeView) -> Vec<&(usize, u64, f32)> {
        let mut best: Vec<_> = self.in_view(view).filter(|p| p.2 > 0.0).collect();
        best.sort_by(|a, b| b.2.total_cmp(&a.2));
        best
    }

    /// The place `t` (a fraction of the genome) is: its chromosome's name
    /// and the bp along it. As the `end` of a window, the last base in
    /// sight, not the first past it.
    #[must_use]
    pub fn place_at(&self, t: f32, end: bool) -> (&str, u64) {
        let pos = (t.clamp(0.0, 1.0) * self.len() as f32) as u64;
        let pos = pos.saturating_sub(u64::from(end));
        let (name, off, _) = &self.chroms[self.chrom_at(pos)];
        (name, pos.saturating_sub(*off))
    }

    /// The chromosome a window (`from`, `span`, as fractions of the genome)
    /// is the whole of, if one.
    #[must_use]
    pub fn whole_chrom(&self, from: f32, span: f32) -> Option<&str> {
        self.chroms.iter().map(|(n, _, _)| &**n).find(|n| {
            self.span_of(n)
                .is_some_and(|(f, s)| (f - from).abs() < 1e-6 && (s - span).abs() < 1e-6)
        })
    }

    /// Chromosome `chr`'s span end to end, as fractions of the genome.
    #[must_use]
    pub fn span_of(&self, chr: &str) -> Option<(f32, f32)> {
        let len = self.len().max(1) as f32;
        let bare = chr_stripped(chr);
        self.chroms
            .iter()
            .find(|(n, _, _)| &**n == bare)
            .map(|(_, o, l)| (*o as f32 / len, *l as f32 / len))
    }
}

/// A chromosome's place in the genome's order: 1, 2, …, then X, Y, the
/// mitochondrion, then the rest by name.
#[must_use]
pub fn chrom_order(chr: &str) -> (u32, String) {
    let c = chr_stripped(chr);
    match c {
        "X" => (1_000, String::new()),
        "Y" => (1_001, String::new()),
        "M" | "MT" => (1_002, String::new()),
        _ => match c.parse::<u32>() {
            Ok(n) => (n, String::new()),
            Err(_) => (2_000, c.to_string()),
        },
    }
}

/// The least value a link counts at, for [`Summary::Count`], by default.
pub const COUNT_CUTOFF: f32 = 0.1;

/// A finished run.
pub struct Run {
    /// Its out prefix.
    pub prefix: String,
    /// Where its trained tables are: [`Run::prefix`], or for a regrouped
    /// run, the run it regrouped.
    pub trained: String,
    pub genes: Vec<Box<str>>,
    /// Each gene's links: rows of the columns below.
    by_gene: Vec<Range<usize>>,
    gene_of: FxHashMap<Box<str>, usize>,
    pub tss: Vec<Option<Tss>>,
    /// Each link's peak, in [`Run::peaks`].
    link_peak: Vec<u32>,
    distance: Vec<i64>,
    score: Vec<f32>,
    gate: Vec<f32>,
    corr: Vec<f32>,
    abc: Vec<f32>,
    /// Every peak the run names (linked, or in its `peaks`), and where it
    /// is, when it has a place.
    peaks: Vec<Box<str>>,
    peak_of: FxHashMap<Box<str>, u32>,
    place: Vec<Option<Place>>,
    /// Cells in each group (a Leiden cluster, or a group given to the
    /// run), when the run kept them.
    pub cluster_sizes: Vec<usize>,
    /// Each group's name: `C0`, `C1`, … for Leiden clusters.
    pub cluster_names: Vec<String>,
    /// Cells that passed QC, in a group or not.
    pub n_cells: usize,
    /// Every gene the run placed, linked or not, by chromosome and TSS:
    /// the gene track of a locus.
    placed: Vec<(Box<str>, Tss)>,
    /// By chromosome (without `chr`): its placed peaks and genes, in order.
    by_chr: FxHashMap<Box<str>, Chrom>,
    /// The chromosomes end to end, as [`Genome::chroms`] has them, and
    /// where each starts (by its name in the run).
    chroms: Vec<(Box<str>, u64, u64)>,
    offset: FxHashMap<Box<str>, u64>,
    /// Each peak's accessibility, when the run kept it.
    rates: Option<Rates>,
    /// Each gene's summary, for the measure and summary last asked.
    summaries: Mutex<Option<Held>>,
}

/// Every gene's summary, and the measure and summary it is by.
type Held = (Measure, Summary, Arc<Vec<f32>>);

/// One chromosome's placed peaks (by start) and genes (by TSS).
#[derive(Default)]
struct Chrom {
    peaks: Vec<u32>,
    /// The longest peak's length, to find those reaching into a window.
    widest: i64,
    genes: Vec<usize>,
}

/// Each peak's accessibility in all the cells and in each cluster, by its
/// index in [`Run::peaks`].
struct Rates {
    all: Vec<f32>,
    clusters: Vec<Vec<f32>>,
}

impl Rates {
    /// Peak `p`'s rate in row `r`: 0 all the cells, `k + 1` cluster `k`.
    fn get(&self, p: u32, r: usize) -> f32 {
        let v = match r {
            0 => &self.all,
            r => &self.clusters[r - 1],
        };
        v.get(p as usize).copied().unwrap_or(0.0)
    }
}

/// A row of the locus figure: all the cells, or one cluster.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: String,
    /// Its cells, when known.
    pub cells: Option<usize>,
    /// Each link's value in this row, in [`Locus::links`] order.
    pub value: Vec<f32>,
    /// Each peak in the window's accessibility in this row, in
    /// [`Locus::peaks`] order.
    pub rate: Vec<f32>,
    /// The gene's ATAC side in this row: all its candidates' accessibility
    /// pooled through their gates (their ABC prior, by abc).
    pub activity: f32,
}

/// A gene's locus, as the figure draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Locus {
    pub gene: Box<str>,
    pub tss: Tss,
    /// The window shown, in bp on the gene's chromosome.
    pub lo: i64,
    pub hi: i64,
    pub measure: Measure,
    /// The gene's links with a place in the window.
    pub links: Vec<Link>,
    /// Every placed peak in the window.
    pub peaks: Vec<(Box<str>, Place)>,
    /// All the cells, then each cluster.
    pub rows: Vec<Row>,
    /// Placed genes with their TSS in the window, the gene itself included.
    pub genes: Vec<(Box<str>, i64)>,
    /// The linked genes before and after it along the chromosome, as
    /// [`Run::neighbour`] steps to them.
    pub steps: [Option<Box<str>>; 2],
}

#[cfg(test)]
impl Locus {
    /// GENE1 at 50 kb on chr1, in a window from 0 to 100 kb, for tests:
    /// `links` as `(peak, start, score)` (a 500 bp peak, its gate and ABC
    /// prior the score, its corr 1), and `rows` as `(label, each link's
    /// value)`, each of 10 cells and activity 1.
    pub fn for_tests(links: &[(&str, i64, f32)], rows: &[(&str, &[f32])]) -> Self {
        let tss = 50_000;
        Locus {
            gene: "GENE1".into(),
            tss: Tss {
                chr: "chr1".into(),
                tss,
                inferred: false,
            },
            lo: 0,
            hi: 100_000,
            measure: Measure::Score,
            links: links
                .iter()
                .map(|&(peak, start, score)| Link {
                    peak: peak.into(),
                    place: Some(Place {
                        chr: "chr1".into(),
                        start,
                        end: start + 500,
                    }),
                    distance: (start + 250 - tss).abs(),
                    score,
                    gate: score,
                    corr: 1.0,
                    abc: score,
                })
                .collect(),
            peaks: Vec::new(),
            rows: rows
                .iter()
                .map(|&(label, value)| Row {
                    label: label.into(),
                    cells: Some(10),
                    value: value.to_vec(),
                    rate: Vec::new(),
                    activity: 1.0,
                })
                .collect(),
            genes: vec![("GENE1".into(), tss)],
            steps: [None, None],
        }
    }
}

pub use crate::p2g::workflow::prefix_of;

mod load;
mod query;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_names_its_run() {
        assert_eq!(prefix_of("out/p2g.links.parquet"), "out/p2g");
        assert_eq!(prefix_of("out/p2g.peaks"), "out/p2g");
        assert_eq!(prefix_of("out/p2g"), "out/p2g");
    }

    #[test]
    fn chromosomes_go_in_their_natural_order() {
        let mut c = vec!["chrX", "chr10", "chr2", "chrM", "chr1", "chrUn_gl1", "chrY"];
        c.sort_by_key(|c| chrom_order(c));
        assert_eq!(
            c,
            ["chr1", "chr2", "chr10", "chrX", "chrY", "chrM", "chrUn_gl1"]
        );
    }

    #[test]
    fn a_measure_is_parsed_by_name() {
        assert_eq!(Measure::parse("corr").unwrap(), Measure::Corr);
        assert!(Measure::parse("sum").is_err());
    }
}
