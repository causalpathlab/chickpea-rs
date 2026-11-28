//! What the views ask of an open run: loci, rows, genes, summaries.

use super::*;

impl Run {
    /// Whether the run kept its peaks' accessibility (runs before the
    /// `peak_rates` table did not).
    #[must_use]
    pub fn has_rates(&self) -> bool {
        self.rates.is_some()
    }

    /// Gene `g`'s locus by `m`: `half` bp each side of its TSS (by default
    /// its farthest link and a tenth more, at least 20 kb; never under
    /// 1 kb), its links' values in all the cells and in each cluster. In a row, a link's share is its weight
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

        let n_rows = self.n_rows();
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
        let r = match self
            .cluster_names
            .iter()
            .position(|g| g.eq_ignore_ascii_case(n))
        {
            Some(k) => k + 1,
            None => match n.parse::<usize>() {
                Ok(r) if r <= self.cluster_names.len() => r,
                _ => anyhow::bail!(
                    "no row {n:?}: all, or one of the groups ({})",
                    self.cluster_names.join(", ")
                ),
            },
        };
        anyhow::ensure!(
            r < self.n_rows(),
            "no row {n:?}: this run kept no peak_rates, so a locus has only the row `all`"
        );
        Ok(r)
    }

    /// Rows in each locus: all the cells, then each group when the run kept
    /// its peaks' accessibility.
    #[must_use]
    pub fn n_rows(&self) -> usize {
        1 + self.rates.as_ref().map_or(0, |r| r.clusters.len())
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
