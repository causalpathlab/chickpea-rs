//! The samples of a run: one RNA and one ATAC file each, paired by
//! position (`--rna a.zarr b.zarr --atac a.atac.zarr b.atac.zarr`).
//!
//! With several samples a cell is `{barcode}@{sample}`, so two samples'
//! cells that share a barcode stay two cells, and each sample is a batch
//! unless `--batch` says otherwise. The samples are named as senna's
//! multiome load names its sample groups, so the same files tag their
//! cells alike in both (and a lupin round of a senna run joins). A single
//! sample's cells keep their barcodes.

use data_beans::sparse_io::{open_sparse_matrix_by_path, COLUMN_SEP};
use genomic_data::coordinates::parse_peak_coordinates;
use legume_numeric::matrix::common_io::basename;
use rustc_hash::FxHashSet;
use std::borrow::Cow;

/// One sample's files, and its name when there are several.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    /// The `@` tag of its cells; `None` for a run of one sample.
    pub name: Option<Box<str>>,
    pub rna: Box<str>,
    pub atac: Box<str>,
}

/// `barcode` as a run names it, in the sample tagged `tag`.
#[must_use]
pub fn cell<'a>(tag: Option<&str>, barcode: &'a str) -> Cow<'a, str> {
    match tag {
        Some(t) => Cow::Owned(format!("{barcode}{COLUMN_SEP}{t}")),
        None => Cow::Borrowed(barcode),
    }
}

impl Sample {
    /// Whether its genes and peaks are rows of one file (a 10x multiome
    /// matrix, say), told apart by name: a peak is `chr:start-end`.
    #[must_use]
    pub fn mixed(&self) -> bool {
        self.rna == self.atac
    }

    /// `barcode` as the run names it.
    #[must_use]
    pub fn cell<'a>(&self, barcode: &'a str) -> Cow<'a, str> {
        cell(self.name.as_deref(), barcode)
    }
}

/// The samples of `rna` and `atac`, paired by position and named after
/// their files.
pub fn pair(rna: &[Box<str>], atac: &[Box<str>]) -> anyhow::Result<Vec<Sample>> {
    anyhow::ensure!(
        rna.len() == atac.len(),
        "{} RNA files and {} ATAC files: give one of each per sample, in the same order",
        rna.len(),
        atac.len()
    );
    anyhow::ensure!(!rna.is_empty(), "no RNA and ATAC files");
    let names = (rna.len() > 1).then(|| names(rna, atac));
    Ok(rna
        .iter()
        .zip(atac)
        .enumerate()
        .map(|(i, (r, a))| Sample {
            name: names.as_ref().map(|n| n[i].clone()),
            rna: r.clone(),
            atac: a.clone(),
        })
        .collect())
}

/// A file's name, its backend extension dropped, split into words (as
/// senna's multiome load splits it).
#[must_use]
pub fn words(path: &str) -> Vec<String> {
    let base = basename(path).map_or_else(|_| path.to_string(), |b| b.to_string());
    base.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Each sample's name, by the rule of senna's multiome load: the words its
/// two files share, less those every sample's share (`s1` of
/// `s1.rna` and `s1.atac` beside `s2.*`), joined by `_`; `g0`,
/// `g1`, … when that leaves a sample nameless or two alike.
fn names(rna: &[Box<str>], atac: &[Box<str>]) -> Vec<Box<str>> {
    let shared: Vec<Vec<String>> = rna
        .iter()
        .zip(atac)
        .map(|(r, a)| {
            let other = words(a);
            words(r).into_iter().filter(|t| other.contains(t)).collect()
        })
        .collect();
    let everywhere: FxHashSet<&String> = shared[0]
        .iter()
        .filter(|t| shared.iter().all(|s| s.contains(t)))
        .collect();
    let named: Vec<Box<str>> = shared
        .iter()
        .map(|s| {
            let own: Vec<&str> = s
                .iter()
                .filter(|t| !everywhere.contains(t))
                .map(String::as_str)
                .collect();
            own.join("_").into()
        })
        .collect();
    let distinct: FxHashSet<&str> = named.iter().map(AsRef::as_ref).collect();
    if named.iter().any(|n| n.is_empty()) || distinct.len() != named.len() {
        return (0..rna.len()).map(|k| format!("g{k}").into()).collect();
    }
    named
}

/// The sample tags of a finished run whose cells are `cells`, for `n`
/// ATAC files given in the order peak-to-gene was given them: the order
/// the tags first appear among the cells (a run of one sample has none).
/// Barcodes do not tell samples on one whitelist apart, so the order is
/// the pairing.
pub fn tags_of(cells: &[Box<str>], n: usize) -> anyhow::Result<Vec<Option<Box<str>>>> {
    let mut names: Vec<&str> = Vec::new();
    let mut seen: FxHashSet<&str> = FxHashSet::default();
    for c in cells {
        if let Some((_, tag)) = c.rsplit_once(COLUMN_SEP) {
            if seen.insert(tag) {
                names.push(tag);
            }
        }
    }
    if names.is_empty() && n == 1 {
        return Ok(vec![None]);
    }
    anyhow::ensure!(
        names.len() == n,
        "{n} ATAC files for a run of {} samples ({}): give each sample's, in the order \
         peak-to-gene was given them",
        names.len().max(1),
        names.join(", ")
    );
    Ok(names.into_iter().map(|t| Some(t.into())).collect())
}

/// One side of a sample: its genes or its peaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Rna = 0,
    Atac = 1,
}

/// Which of `names` are peaks (`chr:start-end`, or `chr-start-end`).
#[must_use]
pub fn peak_rows(names: &[Box<str>]) -> Vec<bool> {
    parse_peak_coordinates(names)
        .iter()
        .map(Option::is_some)
        .collect()
}

/// The rows of `sample`'s `side`: all its file's rows, or for a mixed file
/// the genes (rows that are not peaks) or the peaks.
pub fn rows(sample: &Sample, side: Side) -> anyhow::Result<Vec<Box<str>>> {
    let file = match side {
        Side::Rna => &sample.rna,
        Side::Atac => &sample.atac,
    };
    let names = open_sparse_matrix_by_path(file)?.row_names()?;
    if !sample.mixed() {
        return Ok(names);
    }
    let peak = peak_rows(&names);
    Ok(names
        .into_iter()
        .zip(peak)
        .filter(|&(_, p)| p == (side == Side::Atac))
        .map(|(n, _)| n)
        .collect())
}

/// Refuse samples whose `side` (genes, or peaks) differs from the first
/// sample's: the run has one axis of each. The first's rows.
pub fn same_rows(samples: &[Sample], side: Side) -> anyhow::Result<Vec<Box<str>>> {
    let what = match side {
        Side::Rna => "genes",
        Side::Atac => "peaks",
    };
    let first = rows(&samples[0], side)?;
    anyhow::ensure!(!first.is_empty(), "{} has no {what}", samples[0].rna);
    for s in &samples[1..] {
        let mine = rows(s, side)?;
        anyhow::ensure!(
            mine == first,
            "{} has other {what} than {} ({} and {}): give every sample the same {what}, in \
             the same order (data-beans subset or merge can make them so)",
            match side {
                Side::Rna => &s.rna,
                Side::Atac => &s.atac,
            },
            samples[0].rna,
            mine.len(),
            first.len()
        );
    }
    Ok(first)
}

/// What a count file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holds {
    Rna,
    Atac,
    /// Genes and peaks, as rows of one file.
    Both,
}

/// Words of file names that say what a file holds, not whose it is.
pub const RNA_WORDS: &[&str] = &["rna", "gex", "gene", "genes", "expression"];
pub const ATAC_WORDS: &[&str] = &["atac", "peak", "peaks", "chromatin", "accessibility"];
pub const BOTH_WORDS: &[&str] = &["multiome", "arc", "joint"];

/// What `path` holds by its name, when the name says (`s1_gex.h5`,
/// `s2.atac.zarr.zip`, `multiome.h5`).
#[must_use]
pub fn holds_by_name(path: &str) -> Option<Holds> {
    let w: Vec<String> = words(path).iter().map(|w| w.to_lowercase()).collect();
    let has = |list: &[&str]| w.iter().any(|t| list.contains(&t.as_str()));
    match (has(RNA_WORDS), has(ATAC_WORDS), has(BOTH_WORDS)) {
        (true, false, false) => Some(Holds::Rna),
        (false, true, false) => Some(Holds::Atac),
        (false, false, true) => Some(Holds::Both),
        _ => None,
    }
}

/// What `path` holds by its rows: peaks, genes, or both (a few stray
/// names of the other kind aside).
pub fn holds(path: &str) -> anyhow::Result<Holds> {
    let names = open_sparse_matrix_by_path(path)?.row_names()?;
    let peaks = peak_rows(&names).into_iter().filter(|&p| p).count();
    let n = names.len().max(1);
    Ok(if peaks * 100 <= n {
        Holds::Rna
    } else if peaks * 100 >= n * 99 {
        Holds::Atac
    } else {
        Holds::Both
    })
}

/// The words of a file's name that may name its sample: all but those
/// that say what it holds, lower case.
fn sample_words(path: &str) -> Vec<String> {
    words(path)
        .into_iter()
        .map(|w| w.to_lowercase())
        .filter(|w| {
            ![RNA_WORDS, ATAC_WORDS, BOTH_WORDS]
                .concat()
                .contains(&w.as_str())
        })
        .collect()
}

/// RNA files paired with ATAC files by their names, as indices into
/// each: each pair the two that share the most words of a sample's name
/// (`s1_gex.h5` and `s1_atac.h5`), the best first; files that share
/// none, in the order given. The files left over have no partner (`None`).
#[must_use]
pub fn pair_by_name(
    rna: &[impl AsRef<str>],
    atac: &[impl AsRef<str>],
) -> Vec<(Option<usize>, Option<usize>)> {
    let rw: Vec<Vec<String>> = rna.iter().map(|p| sample_words(p.as_ref())).collect();
    let aw: Vec<Vec<String>> = atac.iter().map(|p| sample_words(p.as_ref())).collect();
    let mut scored: Vec<(usize, usize, usize)> = Vec::new();
    for (i, r) in rw.iter().enumerate() {
        for (j, a) in aw.iter().enumerate() {
            let shared = r.iter().filter(|w| a.contains(w)).count();
            if shared > 0 {
                scored.push((shared, i, j));
            }
        }
    }
    // Most words first; among equals, the order given.
    scored.sort_by_key(|&(n, i, j)| (std::cmp::Reverse(n), i, j));
    let (mut r_of, mut a_of) = (vec![None; rna.len()], vec![false; atac.len()]);
    for (_, i, j) in scored {
        if r_of[i].is_none() && !a_of[j] {
            r_of[i] = Some(j);
            a_of[j] = true;
        }
    }
    // The rest by order.
    let mut free = (0..atac.len()).filter(|&j| !a_of[j]);
    for slot in r_of.iter_mut().filter(|s| s.is_none()) {
        *slot = free.next();
    }
    let mut out: Vec<(Option<usize>, Option<usize>)> = r_of
        .iter()
        .enumerate()
        .map(|(i, &j)| (Some(i), j))
        .collect();
    let taken: FxHashSet<usize> = r_of.iter().flatten().copied().collect();
    out.extend(
        (0..atac.len())
            .filter(|j| !taken.contains(j))
            .map(|j| (None, Some(j))),
    );
    out
}

/// The samples of count files given in any order (`--data`): each told
/// RNA, ATAC or both by its rows, a file of both a sample by itself, and
/// the RNA and ATAC files paired by name.
pub fn infer(files: &[Box<str>]) -> anyhow::Result<Vec<Sample>> {
    let mut rna: Vec<Box<str>> = Vec::new();
    let mut atac: Vec<Box<str>> = Vec::new();
    let mut both: Vec<Box<str>> = Vec::new();
    for f in files {
        match holds(f)? {
            Holds::Rna => rna.push(f.clone()),
            Holds::Atac => atac.push(f.clone()),
            Holds::Both => both.push(f.clone()),
        }
    }
    let mut pairs: Vec<(Box<str>, Box<str>)> = both.into_iter().map(|f| (f.clone(), f)).collect();
    for (r, a) in pair_by_name(&rna, &atac) {
        match (r.map(|i| rna[i].clone()), a.map(|j| atac[j].clone())) {
            (Some(r), Some(a)) => {
                log::info!("Sample: RNA {r} with ATAC {a}");
                pairs.push((r, a));
            }
            (Some(f), None) | (None, Some(f)) => anyhow::bail!(
                "{f} has no partner: {} RNA and {} ATAC files (and {} of both) among --data",
                rna.len(),
                atac.len(),
                pairs.len()
            ),
            (None, None) => {}
        }
    }
    let (r, a): (Vec<Box<str>>, Vec<Box<str>>) = pairs.into_iter().unzip();
    pair(&r, &a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(v: &[&str]) -> Vec<Box<str>> {
        v.iter().map(|&s| s.into()).collect()
    }

    fn named(rna: &[&str], atac: &[&str]) -> Vec<Option<Box<str>>> {
        pair(&b(rna), &b(atac))
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect()
    }

    #[test]
    fn one_sample_keeps_its_barcodes() {
        let s = pair(&b(&["r.zarr"]), &b(&["a.zarr"])).unwrap();
        assert_eq!(s[0].name, None);
        assert_eq!(&*s[0].cell("AC"), "AC");
    }

    #[test]
    fn samples_are_named_after_their_files() {
        assert_eq!(
            named(
                &["d/s1.rna.zarr.zip", "d/s2.rna.zarr.zip"],
                &["d/s1.atac.zarr.zip", "d/s2.atac.zarr.zip"]
            ),
            [Some("s1".into()), Some("s2".into())]
        );
        assert_eq!(
            named(
                &["x/rna.zarr", "y/rna.zarr"],
                &["x/atac.zarr", "y/atac.zarr"]
            ),
            [Some("g0".into()), Some("g1".into())],
            "numbered when the files are named alike, as senna numbers them"
        );
        assert_eq!(
            named(&["rna1.h5", "rna2.h5"], &["atac_a.h5", "atac_b.h5"]),
            [Some("g0".into()), Some("g1".into())]
        );
        let s = pair(
            &b(&["s1.rna.h5", "s2.rna.h5"]),
            &b(&["s1.atac.h5", "s2.atac.h5"]),
        )
        .unwrap();
        assert_eq!(&*s[1].cell("AC"), "AC@s2");
    }

    #[test]
    fn files_pair_by_the_words_of_their_names() {
        let p = pair_by_name(
            &["X2_s_b_gex.h5", "X1_s_a_gex.h5"],
            &["X3_s_a_atac.h5", "X4_s_b_atac.h5"],
        );
        assert_eq!(p, [(Some(0), Some(1)), (Some(1), Some(0))]);
        let p = pair_by_name(&["x.h5", "y.h5"], &["u.h5"]);
        assert_eq!(p, [(Some(0), Some(0)), (Some(1), None)]);
        assert_eq!(holds_by_name("d/s1_GEX.h5"), Some(Holds::Rna));
        assert_eq!(holds_by_name("s1.atac.zarr.zip"), Some(Holds::Atac));
        assert_eq!(holds_by_name("filtered_feature_bc_matrix.h5"), None);
    }

    #[test]
    fn files_pair_one_to_one() {
        assert!(pair(&b(&["a", "b"]), &b(&["c"])).is_err());
    }
}
