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

impl Run {
    /// Open the run at `path` (its out prefix, or one of its tables).
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let prefix = prefix_of(path);
        let trained = crate::p2g::workflow::trained_run(&prefix)?;
        let links = table(&trained, LINKS);
        let (s, n) = read_table_columns(
            &links,
            &["gene", "peak"],
            &["distance", "abc", "gate", "corr", "score"],
        )
        .with_context(|| format!("reading {links}"))?;
        let ([gene, peak], [distance, abc, gate, corr, score]) = (&s[..], &n[..]) else {
            unreachable!("the columns asked for")
        };

        // Genes by name; a run writes each gene's links together, but any
        // order is taken.
        let mut genes: Vec<Box<str>> = gene
            .iter()
            .collect::<rustc_hash::FxHashSet<_>>()
            .into_iter()
            .cloned()
            .collect();
        genes.sort_unstable();
        let gene_of = crate::common::build_name2index_map(&genes);
        let id: Vec<usize> = gene.iter().map(|g| gene_of[g]).collect();
        let mut order: Vec<usize> = (0..gene.len()).collect();
        order.sort_unstable_by(|&i, &j| {
            id[i]
                .cmp(&id[j])
                .then(distance[i].total_cmp(&distance[j]))
                .then(i.cmp(&j))
        });
        let mut by_gene: Vec<Range<usize>> = vec![0..0; genes.len()];
        for (r, &i) in order.iter().enumerate() {
            let b = &mut by_gene[id[i]];
            if r == 0 || id[order[r - 1]] != id[i] {
                b.start = r;
            }
            b.end = r + 1;
        }
        let take = |v: &[f64]| order.iter().map(|&i| v[i] as f32).collect::<Vec<_>>();

        let mut run = Run {
            prefix: prefix.clone(),
            trained,
            tss: vec![None; genes.len()],
            genes,
            by_gene,
            gene_of,
            link_peak: Vec::with_capacity(order.len()),
            distance: order.iter().map(|&i| distance[i] as i64).collect(),
            score: take(score),
            gate: take(gate),
            corr: take(corr),
            abc: take(abc),
            peaks: Vec::new(),
            peak_of: FxHashMap::default(),
            place: Vec::new(),
            cluster_sizes: Vec::new(),
            cluster_names: Vec::new(),
            n_cells: 0,
            placed: Vec::new(),
            by_chr: FxHashMap::default(),
            chroms: Vec::new(),
            offset: FxHashMap::default(),
            rates: None,
            summaries: Mutex::new(None),
        };
        run.link_peak = order.iter().map(|&i| run.intern(&peak[i])).collect();
        run.read_places()?;
        run.read_tss()?;
        run.read_clusters()?;
        run.read_rates()?;
        run.index_chroms();
        Ok(run)
    }

    /// Peak `name`'s index in [`Run::peaks`], adding it when new.
    fn intern(&mut self, name: &str) -> u32 {
        if let Some(&p) = self.peak_of.get(name) {
            return p;
        }
        let p = self.peaks.len() as u32;
        self.peaks.push(name.into());
        self.peak_of.insert(name.into(), p);
        self.place.push(None);
        p
    }

    fn read_places(&mut self) -> anyhow::Result<()> {
        let path = table(&self.trained, PEAKS);
        if !Path::new(&path).exists() {
            return Ok(());
        }
        let (s, n) = read_table_columns(&path, &["peak", "chromosome"], &["start", "end"])?;
        let ([peak, chr], [start, end]) = (&s[..], &n[..]) else {
            unreachable!("the columns asked for")
        };
        for i in 0..peak.len() {
            let p = self.intern(&peak[i]) as usize;
            if start[i] >= 0.0 {
                self.place[p] = Some(Place {
                    chr: chr[i].clone(),
                    start: start[i] as i64,
                    end: end[i] as i64,
                });
            }
        }
        Ok(())
    }

    fn read_tss(&mut self) -> anyhow::Result<()> {
        let path = table(&self.trained, GENES);
        if Path::new(&path).exists() {
            let (s, n) = read_table_columns(&path, &["gene", "chromosome"], &["tss"])?;
            let ([gene, chr], [tss]) = (&s[..], &n[..]) else {
                unreachable!("the columns asked for")
            };
            for i in 0..gene.len() {
                let t = Tss {
                    chr: chr[i].clone(),
                    tss: tss[i] as i64,
                    inferred: false,
                };
                if let Some(&g) = self.gene_of.get(&gene[i]) {
                    self.tss[g] = Some(t.clone());
                }
                self.placed.push((gene[i].clone(), t));
            }
            return Ok(());
        }
        // An older run: the TSS each peak allows (its midpoint ± its
        // distance), the one most of them agree on.
        for g in 0..self.genes.len() {
            let mut votes: FxHashMap<(Box<str>, i64), usize> = FxHashMap::default();
            for r in self.by_gene[g].clone() {
                if let Some(p) = &self.place[self.link_peak[r] as usize] {
                    let d = self.distance[r];
                    for t in [p.mid() - d, p.mid() + d] {
                        *votes.entry((p.chr.clone(), t)).or_default() += 1;
                    }
                }
            }
            self.tss[g] = votes
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0 .1.cmp(&a.0 .1)))
                .map(|((chr, tss), _)| Tss {
                    chr,
                    tss,
                    inferred: true,
                });
            if let Some(t) = &self.tss[g] {
                self.placed.push((self.genes[g].clone(), t.clone()));
            }
        }
        Ok(())
    }

    fn read_rates(&mut self) -> anyhow::Result<()> {
        let path = table(&self.prefix, PEAK_RATES);
        if !Path::new(&path).exists() {
            return Ok(());
        }
        let names: Vec<String> = (0..self.cluster_sizes.len())
            .map(|k| k.to_string())
            .collect();
        let mut numeric: Vec<&str> = vec!["all"];
        numeric.extend(names.iter().map(String::as_str));
        let (s, n) = read_table_columns(&path, &["peak"], &numeric)
            .with_context(|| format!("reading {path}"))?;
        let at: Vec<usize> = s[0].iter().map(|p| self.intern(p) as usize).collect();
        let by_peak = |v: &[f64]| {
            let mut out = vec![0f32; self.peaks.len()];
            for (&p, &x) in at.iter().zip(v) {
                out[p] = x as f32;
            }
            out
        };
        let mut rows = n.iter().map(|v| by_peak(v));
        let all = rows.next().expect("the all column");
        self.rates = Some(Rates {
            all,
            clusters: rows.collect(),
        });
        Ok(())
    }

    /// Each chromosome's peaks and genes in order, and the chromosomes end
    /// to end (each to its last gene or peak).
    fn index_chroms(&mut self) {
        let mut by_chr: FxHashMap<Box<str>, Chrom> = FxHashMap::default();
        for (p, place) in self.place.iter().enumerate() {
            if let Some(place) = place {
                let c = by_chr.entry(chr_stripped(&place.chr).into()).or_default();
                c.peaks.push(p as u32);
                c.widest = c.widest.max(place.end - place.start);
            }
        }
        for (i, (_, t)) in self.placed.iter().enumerate() {
            by_chr
                .entry(chr_stripped(&t.chr).into())
                .or_default()
                .genes
                .push(i);
        }
        for c in by_chr.values_mut() {
            c.peaks
                .sort_by_key(|&p| self.place[p as usize].as_ref().map_or(0, |q| q.start));
            c.genes.sort_by_key(|&i| self.placed[i].1.tss);
        }
        self.by_chr = by_chr;

        let mut len: FxHashMap<&str, u64> = FxHashMap::default();
        for t in self.tss.iter().flatten() {
            let e = len.entry(&t.chr).or_default();
            *e = (*e).max(t.tss.max(0) as u64);
        }
        for p in self.place.iter().flatten() {
            if let Some(e) = len.get_mut(&*p.chr) {
                *e = (*e).max(p.end.max(0) as u64);
            }
        }
        let mut names: Vec<&str> = len.keys().copied().collect();
        names.sort_by_key(|c| chrom_order(c));
        let mut at = 0u64;
        for c in names {
            self.offset.insert(c.into(), at);
            let l = len[c].max(1);
            self.chroms.push((chr_stripped(c).into(), at, l));
            at += l;
        }
    }

    /// Whether the run kept its peaks' accessibility (runs before the
    /// `peak_rates` table did not).
    #[must_use]
    pub fn has_rates(&self) -> bool {
        self.rates.is_some()
    }

    /// Gene `g`'s locus by `m`: `half` bp each side of its TSS (by default
    /// its farthest link and a tenth more), its links' values in all the
    /// cells and in each cluster. In a row, a link's share is its weight
    /// (the gate, or the ABC prior for [`Measure::Abc`]) times its peak's
    /// accessibility there, over the gene's candidates (as
    /// `links_by_cluster` has it); [`Measure::Score`] is that share times the
    /// link's corr, and [`Measure::Corr`] is the corr alone, where the peak
    /// is open. Without the accessibility, every row has the run's values.
    #[must_use]
    pub fn locus(&self, g: usize, m: Measure, half: Option<i64>) -> Option<Locus> {
        let tss = self.tss[g].clone()?;
        let rows_of_g = self.by_gene[g].clone();
        let far = rows_of_g
            .clone()
            .map(|r| self.distance[r])
            .max()
            .unwrap_or(0);
        let half = half.unwrap_or((far + far / 10).max(20_000)).max(1_000);
        let (lo, hi) = ((tss.tss - half).max(0), tss.tss + half);
        let inside = |p: &Place| {
            chr_stripped(&p.chr) == chr_stripped(&tss.chr) && p.end >= lo && p.start <= hi
        };
        let shown: Vec<usize> = rows_of_g
            .clone()
            .filter(|&r| {
                self.place[self.link_peak[r] as usize]
                    .as_ref()
                    .is_some_and(inside)
            })
            .collect();
        let chrom = self.by_chr.get(chr_stripped(&tss.chr));
        let peaks: Vec<u32> = chrom.map_or_else(Vec::new, |c| {
            let start = |p: u32| self.place[p as usize].as_ref().map_or(0, |q| q.start);
            let a = c.peaks.partition_point(|&p| start(p) < lo - c.widest);
            let b = c.peaks.partition_point(|&p| start(p) <= hi);
            c.peaks[a..b]
                .iter()
                .copied()
                .filter(|&p| self.place[p as usize].as_ref().is_some_and(inside))
                .collect()
        });
        let genes: Vec<(Box<str>, i64)> = chrom.map_or_else(Vec::new, |c| {
            let a = c.genes.partition_point(|&i| self.placed[i].1.tss < lo);
            let b = c.genes.partition_point(|&i| self.placed[i].1.tss <= hi);
            c.genes[a..b]
                .iter()
                .map(|&i| (self.placed[i].0.clone(), self.placed[i].1.tss))
                .collect()
        });

        let n_rows = 1 + self.rates.as_ref().map_or(0, |r| r.clusters.len());
        let rows = (0..n_rows)
            .map(|r| {
                let (label, cells) = match r {
                    0 => ("All cells".to_string(), Some(self.n_cells)),
                    r => (
                        self.cluster_names
                            .get(r - 1)
                            .cloned()
                            .unwrap_or_else(|| format!("C{}", r - 1)),
                        self.cluster_sizes.get(r - 1).copied(),
                    ),
                };
                let activity = self.activity(g, m, r);
                let value = self.values(&shown, m, r, activity);
                let rate = peaks.iter().map(|&p| self.rate(p, r)).collect();
                Row {
                    label,
                    cells,
                    value,
                    rate,
                    activity,
                }
            })
            .collect();
        Some(Locus {
            gene: self.genes[g].clone(),
            tss,
            lo,
            hi,
            measure: m,
            links: shown.into_iter().map(|r| self.link(r)).collect(),
            peaks: peaks
                .into_iter()
                .filter_map(|p| {
                    Some((
                        self.peaks[p as usize].clone(),
                        self.place[p as usize].clone()?,
                    ))
                })
                .collect(),
            rows,
            genes,
            steps: [-1, 1].map(|by| self.neighbour(g, by).map(|o| self.genes[o].clone())),
        })
    }

    /// Peak `p`'s accessibility in row `r` of a locus (0 all the cells,
    /// `k + 1` group `k`); 1 where the run kept none.
    fn rate(&self, p: u32, r: usize) -> f32 {
        self.rates.as_ref().map_or(1.0, |rt| rt.get(p, r))
    }

    /// The weights a gene's links pool its peaks by: their ABC prior by
    /// abc, else their gates.
    fn weight(&self, m: Measure) -> &[f32] {
        if m == Measure::Abc {
            &self.abc
        } else {
            &self.gate
        }
    }

    /// Gene `g`'s candidates' accessibility in row `r`, pooled through
    /// [`Run::weight`]: the gene's ATAC side there.
    fn activity(&self, g: usize, m: Measure, r: usize) -> f32 {
        let weight = self.weight(m);
        self.by_gene[g]
            .clone()
            .map(|k| weight[k] * self.rate(self.link_peak[k], r))
            .sum()
    }

    /// A gene's links `shown` (rows of the link columns), valued by `m` in
    /// row `r`: each one's share of `total`, the gene's [`Run::activity`]
    /// over all its candidates, not only those shown, so a link's value
    /// does not change with the window.
    fn values(&self, shown: &[usize], m: Measure, r: usize, total: f32) -> Vec<f32> {
        let weight = self.weight(m);
        shown
            .iter()
            .map(|&k| {
                let open = self.rate(self.link_peak[k], r);
                let share = if total > 0.0 {
                    weight[k] * open / total
                } else {
                    0.0
                };
                match m {
                    Measure::Score => share * self.corr[k],
                    Measure::Gate | Measure::Abc => share,
                    Measure::Corr if open > 0.0 => self.corr[k],
                    Measure::Corr => 0.0,
                }
            })
            .collect()
    }

    /// The groups, from `groups` (named) or, for runs before it,
    /// `cell_clusters` (numbered); a cell in no group (`-1`) counts only
    /// among all the cells.
    fn read_clusters(&mut self) -> anyhow::Result<()> {
        let path = table(&self.prefix, CELL_CLUSTERS);
        if !Path::new(&path).exists() {
            return Ok(());
        }
        let (_, n) = read_table_columns(&path, &[], &["cluster"])?;
        self.n_cells = n[0].len();
        let groups = table(&self.prefix, GROUPS);
        if Path::new(&groups).exists() {
            let (s, n) = read_table_columns(&groups, &["group"], &["cluster", "cells"])
                .with_context(|| format!("reading {groups}"))?;
            let mut rows: Vec<(usize, String, usize)> = (0..s[0].len())
                .map(|i| (n[0][i] as usize, s[0][i].to_string(), n[1][i] as usize))
                .collect();
            rows.sort_by_key(|r| r.0);
            self.cluster_names = rows.iter().map(|r| r.1.clone()).collect();
            self.cluster_sizes = rows.iter().map(|r| r.2).collect();
            return Ok(());
        }
        for &k in n[0].iter().filter(|&&k| k >= 0.0) {
            let k = k as usize;
            if k >= self.cluster_sizes.len() {
                self.cluster_sizes.resize(k + 1, 0);
            }
            self.cluster_sizes[k] += 1;
        }
        self.cluster_names = (0..self.cluster_sizes.len())
            .map(|k| format!("C{k}"))
            .collect();
        Ok(())
    }

    /// Up to `n` of the run's genes named most like `name`: those holding
    /// it (any case), then those sharing the longest start with it.
    #[must_use]
    pub fn genes_like(&self, name: &str, n: usize) -> Vec<String> {
        let q = name.to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let common = |g: &str| g.chars().zip(q.chars()).take_while(|(a, b)| a == b).count();
        let mut scored: Vec<(usize, usize, &str)> = self
            .genes
            .iter()
            .filter_map(|g| {
                let l = g.to_lowercase();
                let holds = l.contains(&q) || q.contains(&l);
                let c = common(&l);
                (holds || c >= 2).then_some((usize::from(!holds), usize::MAX - c, &**g))
            })
            .collect();
        scored.sort_by(|a, b| (a.0, a.1, a.2.len(), a.2).cmp(&(b.0, b.1, b.2.len(), b.2)));
        scored
            .into_iter()
            .take(n)
            .map(|(_, _, g)| g.to_string())
            .collect()
    }

    /// The locus row `name` names: `all` (all the cells), a group's name
    /// (any case), or a row's number as the locus lists it (0 all the
    /// cells, 1 the first group).
    pub fn row_of(&self, name: &str) -> anyhow::Result<usize> {
        let n = name.trim();
        if n.eq_ignore_ascii_case("all") || n.eq_ignore_ascii_case("all cells") {
            return Ok(0);
        }
        if let Some(k) = self
            .cluster_names
            .iter()
            .position(|g| g.eq_ignore_ascii_case(n))
        {
            return Ok(k + 1);
        }
        match n.parse::<usize>() {
            Ok(r) if r <= self.cluster_names.len() => Ok(r),
            _ => anyhow::bail!(
                "no row {n:?}: all, or one of the groups ({})",
                self.cluster_names.join(", ")
            ),
        }
    }

    /// Row `r`'s name, as [`Run::row_of`] reads it.
    #[must_use]
    pub fn row_name(&self, r: usize) -> String {
        match r {
            0 => "all".into(),
            r => self
                .cluster_names
                .get(r - 1)
                .cloned()
                .unwrap_or_else(|| r.to_string()),
        }
    }

    #[must_use]
    pub fn n_pairs(&self) -> usize {
        self.link_peak.len()
    }

    /// The gene named `name`.
    #[must_use]
    pub fn gene(&self, name: &str) -> Option<usize> {
        self.gene_of.get(name).copied()
    }

    /// Gene `g`'s links, nearest first.
    #[must_use]
    pub fn links(&self, g: usize) -> Vec<Link> {
        self.by_gene[g].clone().map(|r| self.link(r)).collect()
    }

    fn link(&self, r: usize) -> Link {
        let p = self.link_peak[r] as usize;
        Link {
            peak: self.peaks[p].clone(),
            place: self.place[p].clone(),
            distance: self.distance[r],
            score: self.score[r],
            gate: self.gate[r],
            corr: self.corr[r],
            abc: self.abc[r],
        }
    }

    /// The linked gene next to `g` along its chromosome by TSS, `step` of
    /// them on (back when negative); genes without links are stepped over.
    #[must_use]
    pub fn neighbour(&self, g: usize, step: isize) -> Option<usize> {
        let tss = self.tss[g].as_ref()?;
        let along = &self.by_chr.get(chr_stripped(&tss.chr))?.genes;
        // The gene among the chromosome's, by TSS (those at the same TSS
        // by name), then the linked ones from there.
        let from = along.partition_point(|&i| self.placed[i].1.tss < tss.tss);
        let at = from
            + along[from..]
                .iter()
                .position(|&i| self.placed[i].0 == self.genes[g])?;
        let linked = |&i: &usize| {
            self.gene_of
                .get(&self.placed[i].0)
                .copied()
                .filter(|&o| o != g && !self.by_gene[o].is_empty())
        };
        let n = step.unsigned_abs().checked_sub(1)?;
        if step > 0 {
            along[at + 1..].iter().filter_map(linked).nth(n)
        } else {
            along[..at].iter().rev().filter_map(linked).nth(n)
        }
    }

    /// Gene `g`'s links in one number.
    #[must_use]
    pub fn summary(&self, g: usize, m: Measure, how: Summary) -> f32 {
        let values = self.by_gene[g].clone().map(|r| self.column(m)[r]);
        match how {
            Summary::Best => values.filter(|v| !v.is_nan()).fold(f32::NAN, f32::max),
            Summary::Count(cut) => values.filter(|&v| v >= cut).count() as f32,
        }
    }

    /// Every gene's [`Run::summary`], worked out once for the measure and
    /// summary last asked.
    fn summaries(&self, m: Measure, how: Summary) -> Arc<Vec<f32>> {
        let mut held = self.summaries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((hm, hh, v)) = &*held {
            if *hm == m && *hh == how {
                return Arc::clone(v);
            }
        }
        let v = Arc::new(
            (0..self.genes.len())
                .map(|g| self.summary(g, m, how))
                .collect(),
        );
        *held = Some((m, how, Arc::clone(&v)));
        v
    }

    /// Candidates gene `g` has.
    #[must_use]
    pub fn n_candidates(&self, g: usize) -> usize {
        self.by_gene[g].len()
    }

    /// Each link's `m`, in row order.
    fn column(&self, m: Measure) -> &[f32] {
        match m {
            Measure::Score => &self.score,
            Measure::Gate => &self.gate,
            Measure::Corr => &self.corr,
            Measure::Abc => &self.abc,
        }
    }

    /// Genes ordered by their summary, the highest first, undefined last.
    #[must_use]
    pub fn ranked(&self, m: Measure, how: Summary) -> Vec<(usize, f32)> {
        let mut v: Vec<(usize, f32)> = self.summaries(m, how).iter().copied().enumerate().collect();
        let key = |x: f32| if x.is_nan() { f32::NEG_INFINITY } else { x };
        v.sort_by(|a, b| key(b.1).total_cmp(&key(a.1)));
        v
    }

    /// The genome end to end, each linked gene with a place on it by its
    /// links in one number.
    #[must_use]
    pub fn genome(&self, m: Measure, how: Summary) -> Genome {
        let summaries = self.summaries(m, how);
        let points = (0..self.genes.len())
            .filter_map(|g| {
                let t = self.tss[g].as_ref()?;
                Some((g, self.offset[&*t.chr] + t.tss.max(0) as u64, summaries[g]))
            })
            .filter(|&(_, _, v)| v.is_finite())
            .collect();
        Genome {
            chroms: self.chroms.clone(),
            points,
            measure: m,
            summary: how,
        }
    }

    /// The run in a few numbers, for `view_open` and the screen's head.
    #[must_use]
    pub fn describe(&self) -> serde_json::Value {
        let placed = self.tss.iter().filter(|t| t.is_some()).count();
        let inferred = self.tss.iter().flatten().filter(|t| t.inferred).count();
        let mut chrs: Vec<&str> = self.tss.iter().flatten().map(|t| &*t.chr).collect();
        chrs.sort_by_key(|c| chrom_order(c));
        chrs.dedup();
        serde_json::json!({
            "run": self.prefix,
            "genes": self.genes.len(),
            "pairs": self.n_pairs(),
            "peaks_placed": self.place.iter().flatten().count(),
            "genes_placed": placed,
            "tss_inferred": inferred,
            "chromosomes": chrs,
            "cells": self.n_cells,
            "groups": self.cluster_names,
            "peak_rates": self.has_rates(),
            "group_cells": self.cluster_sizes,
            "measures": Measure::ALL.map(Measure::name),
        })
    }
}

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
