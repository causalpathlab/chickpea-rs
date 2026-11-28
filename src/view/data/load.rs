//! Opening a run: its links, places, TSSs, accessibility and groups.

use super::*;

impl Run {
    /// Open the run at `path` (its out prefix, or one of its tables).
    pub fn open(path: &str) -> anyhow::Result<Self> {
        let prefix = prefix_of(path);
        let trained = crate::p2g::workflow::trained_run(&prefix)?;
        let links = table(&trained, LINKS);
        let crate::p2g::workflow::LinkColumns {
            gene,
            peak,
            distance,
            abc,
            gate,
            corr,
            score,
        } = crate::p2g::workflow::read_links(&links)?;

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
            score: take(&score),
            gate: take(&gate),
            corr: take(&corr),
            abc: take(&abc),
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
}
