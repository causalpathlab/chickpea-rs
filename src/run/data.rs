//! The data pane: a step's count files, one row per sample (its RNA and its
//! ATAC, for peak-to-gene), set apart from the other flags as senna's data
//! screen is; a step without count files (regroup) has none. Files are
//! added from the browser, several at once: each is told RNA or ATAC by
//! its name (or, failing that, by whether its rows are peaks) and paired by
//! the words of the names.

use crate::p2g::samples::{self, Holds};
use legume_tui::form::{emit, Many};
use std::path::{Path, PathBuf};

pub use crate::p2g::samples::Side;

impl Side {
    /// Its flag, without `--`.
    #[must_use]
    pub fn long(self) -> &'static str {
        match self {
            Side::Rna => "rna",
            Side::Atac => "atac",
        }
    }

    /// Its column's title.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Side::Rna => "RNA",
            Side::Atac => "ATAC",
        }
    }
}

/// One sample's files, by [`Side`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pair(pub [Option<PathBuf>; 2]);

impl Pair {
    #[must_use]
    pub fn get(&self, side: Side) -> Option<&PathBuf> {
        self.0[side as usize].as_ref()
    }

    /// The files it has.
    pub fn files(&self) -> impl Iterator<Item = &PathBuf> {
        self.0.iter().flatten()
    }
}

/// A sample's files, in column order.
pub const SIDES: [Side; 2] = [Side::Rna, Side::Atac];

/// The samples, and the cell under the cursor.
#[derive(Default)]
pub struct Data {
    pub pairs: Vec<Pair>,
    /// The row under the cursor; `pairs.len()` is the "add" row.
    pub at: usize,
    /// The column under the cursor, into [`SIDES`]; kept in range by its
    /// writers.
    pub col: usize,
}

impl Data {
    /// The rows given as flags: `--rna a b --atac c d` read back.
    pub fn prefill(&mut self, values: impl Fn(Side) -> Vec<String>) {
        let lists: Vec<(Side, Vec<String>)> = SIDES.iter().map(|&s| (s, values(s))).collect();
        let n = lists.iter().map(|(_, l)| l.len()).max().unwrap_or(0);
        self.pairs = (0..n)
            .map(|i| {
                let mut p = Pair::default();
                for (s, list) in &lists {
                    p.0[*s as usize] = list.get(i).map(PathBuf::from);
                }
                p
            })
            .collect();
    }

    /// The side under the cursor.
    #[must_use]
    pub fn side(&self) -> Side {
        SIDES[self.col]
    }

    /// The flags the rows stand for, each side's files in row order.
    #[must_use]
    pub fn argv(&self) -> Vec<String> {
        SIDES
            .iter()
            .flat_map(|&s| {
                let vs: Vec<String> = self
                    .pairs
                    .iter()
                    .filter_map(|p| p.get(s))
                    .map(|p| crate::tui::shown(p))
                    .collect();
                emit(&format!("--{}", s.long()), Many::Listed, vs)
            })
            .collect()
    }

    /// Why the rows cannot run yet: no files, or a sample without one.
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        if self.pairs.is_empty() {
            let what: Vec<&str> = SIDES.iter().map(|s| s.title()).collect();
            return Some(format!("add the {} files (a)", what.join(" and ")));
        }
        self.pairs.iter().enumerate().find_map(|(i, p)| {
            let missing = SIDES.iter().find(|&&s| p.get(s).is_none())?;
            Some(format!("sample {} has no {} file", i + 1, missing.title()))
        })
    }

    /// Each row's sample name, as the run will tag its cells; none for one
    /// sample, or while a row lacks a file.
    #[must_use]
    pub fn names(&self) -> Vec<Option<String>> {
        let none = vec![None; self.pairs.len()];
        if self.problem().is_some() {
            return none;
        }
        let list = |s: Side| -> Vec<Box<str>> {
            self.pairs
                .iter()
                .filter_map(|p| p.get(s))
                .map(|p| p.to_string_lossy().into())
                .collect()
        };
        samples::pair(&list(Side::Rna), &list(Side::Atac)).map_or(none, |ss| {
            ss.into_iter().map(|s| s.name.map(String::from)).collect()
        })
    }

    /// Put `path` in the cell under the cursor (a new row on the add row).
    pub fn set(&mut self, path: PathBuf) {
        let side = self.side();
        if self.at >= self.pairs.len() {
            self.pairs.push(Pair::default());
            self.at = self.pairs.len() - 1;
        }
        self.pairs[self.at].0[side as usize] = Some(path);
    }

    /// Drop the row under the cursor.
    pub fn remove(&mut self) {
        if self.at < self.pairs.len() {
            self.pairs.remove(self.at);
        }
    }

    /// Take `files` chosen together: each told RNA, ATAC or both (a file
    /// of both is a row by itself), gaps in the rows filled first, then the
    /// rest paired by the words of their names. Says what it did.
    pub fn add(&mut self, files: Vec<PathBuf>) -> String {
        let have: Vec<&PathBuf> = self.pairs.iter().flat_map(Pair::files).collect();
        let files: Vec<PathBuf> = files.into_iter().filter(|f| !have.contains(&f)).collect();
        let mut pools: [Vec<PathBuf>; 2] = [Vec::new(), Vec::new()];
        let mut both = 0;
        for f in files {
            match holds(&f) {
                Holds::Rna => pools[0].push(f),
                Holds::Atac => pools[1].push(f),
                Holds::Both => {
                    both += 1;
                    self.pairs.push(Pair([Some(f.clone()), Some(f)]));
                }
            }
        }
        let [n_rna, n_atac] = [pools[0].len(), pools[1].len()];
        // Rows short of a side take the file of it named most alike.
        for p in &mut self.pairs {
            for side in SIDES {
                let (k, other) = (side as usize, 1 - side as usize);
                let (None, Some(o)) = (&p.0[k], &p.0[other]) else {
                    continue;
                };
                let pool = names(&pools[k]);
                let mate = samples::pair_by_name(&[o.to_string_lossy()], &pool);
                if let Some(j) = mate.first().and_then(|&(_, j)| j) {
                    p.0[k] = Some(pools[k].remove(j));
                }
            }
        }
        let [rna, atac] = pools;
        let pairs = samples::pair_by_name(&names(&rna), &names(&atac));
        self.pairs.extend(
            pairs
                .into_iter()
                .map(|(r, a)| Pair([r.map(|i| rna[i].clone()), a.map(|j| atac[j].clone())])),
        );
        match (n_rna, n_atac, both) {
            (0, 0, 0) => "nothing new".into(),
            (_, _, 0) => format!("{n_rna} RNA and {n_atac} ATAC files added"),
            _ => format!("{n_rna} RNA, {n_atac} ATAC and {both} RNA + ATAC files added"),
        }
    }
}

/// Paths as names, to pair by.
fn names(paths: &[PathBuf]) -> Vec<std::borrow::Cow<'_, str>> {
    paths.iter().map(|p| p.to_string_lossy()).collect()
}

/// What a file holds: by its name when that says, else by its rows (a
/// file that cannot be read is taken as RNA, and the run says why).
fn holds(path: &Path) -> Holds {
    let p = path.to_string_lossy();
    samples::holds_by_name(&p)
        .or_else(|| samples::holds(&p).ok())
        .unwrap_or(Holds::Rna)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn files_are_sorted_and_paired_by_name() {
        let mut d = Data::default();
        d.add(vec![
            p("s2.atac.zarr.zip"),
            p("s1.rna.zarr.zip"),
            p("s1.atac.zarr.zip"),
            p("s2.gex.h5"),
        ]);
        assert_eq!(
            d.pairs,
            [
                Pair([Some(p("s1.rna.zarr.zip")), Some(p("s1.atac.zarr.zip"))]),
                Pair([Some(p("s2.gex.h5")), Some(p("s2.atac.zarr.zip"))]),
            ]
        );
        assert_eq!(d.problem(), None);
        assert_eq!(
            d.argv(),
            [
                "--rna",
                "s1.rna.zarr.zip",
                "s2.gex.h5",
                "--atac",
                "s1.atac.zarr.zip",
                "s2.atac.zarr.zip"
            ]
        );
        assert_eq!(d.names(), [Some("s1".to_string()), Some("s2".to_string())]);
    }

    #[test]
    fn a_file_of_both_is_a_sample_by_itself() {
        let mut d = Data::default();
        d.add(vec![
            p("X2_dB_gex.h5"),
            p("dA_multiome.h5"),
            p("X9_dB_atac.h5"),
        ]);
        assert_eq!(
            d.pairs,
            [
                Pair([Some(p("dA_multiome.h5")), Some(p("dA_multiome.h5"))]),
                Pair([Some(p("X2_dB_gex.h5")), Some(p("X9_dB_atac.h5"))]),
            ]
        );
        assert_eq!(d.names(), [Some("dA_multiome".into()), Some("dB".into())]);
    }

    #[test]
    fn a_missing_side_is_filled_by_the_next_files() {
        let mut d = Data::default();
        d.add(vec![p("s1_rna.h5")]);
        assert!(d.problem().unwrap().contains("no ATAC"));
        d.add(vec![p("s1_atac.h5")]);
        assert_eq!(d.pairs.len(), 1);
        assert_eq!(d.problem(), None);
        assert_eq!(d.names(), [None], "one sample keeps its barcodes");
    }
}
