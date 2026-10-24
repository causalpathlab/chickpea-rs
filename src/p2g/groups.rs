//! Cell groups given from outside: a lupin round's cell types, or any
//! cell → group table, in place of chickpea's own Leiden clusters.
//!
//! Read from:
//! - a lupin round manifest (`.json`): its `annotate.argmax` table;
//! - a labels table with a header, parquet or delimited text (`.gz` fine):
//!   `cell` (or `barcode`) and `cell_type` (or `group`, `label`), as lupin's
//!   `{round}.argmax.parquet` / `.argmax.tsv` have them;
//! - a header-less text table: cell, then group.
//!
//! Cells labelled `unassigned` (lupin's word for a cell it could not call),
//! `NA` or nothing get no group. A cell is found by its name, or else a
//! `{barcode}@{batch}` and a bare `{barcode}` join (never two batches'
//! cells with one barcode); a bare barcode two groups claim is no one's.

use legume_numeric::matrix::table::TableReader;
use rustc_hash::FxHashMap;
use std::path::Path;

/// Each named cell's group, and the groups in order.
#[derive(Debug, Clone, Default)]
pub struct Groups {
    /// The groups, by name in natural order (`C2` before `C10`).
    pub names: Vec<Box<str>>,
    exact: FxHashMap<Box<str>, usize>,
    /// Cells listed as `{barcode}@{batch}`, by barcode; `None` where two
    /// groups claim it.
    base: FxHashMap<Box<str>, Option<usize>>,
    /// The table read, after a manifest is followed.
    pub table: String,
    /// Cells the table lists without a group.
    pub n_unassigned: usize,
}

/// Labels that name no group.
fn no_group(label: &str) -> bool {
    let l = label.trim();
    l.is_empty()
        || l.eq_ignore_ascii_case("unassigned")
        || l.eq_ignore_ascii_case("na")
        || l.eq_ignore_ascii_case("nan")
}

/// A name's sort key: its runs of digits by value, the rest as text.
fn natural(name: &str) -> Vec<(String, u64)> {
    let mut key = Vec::new();
    let mut text = String::new();
    let mut chars = name.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut n = 0u64;
            while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                n = n.saturating_mul(10).saturating_add(u64::from(d));
                chars.next();
            }
            key.push((std::mem::take(&mut text), n));
        } else {
            text.push(c);
            chars.next();
        }
    }
    key.push((text, 0));
    key
}

impl Groups {
    /// Read the groups at `path` (see the module docs for the forms).
    pub fn read(path: &str) -> anyhow::Result<Self> {
        let table = if path.ends_with(".json") {
            argmax_of(path)?
        } else {
            path.to_string()
        };
        let pairs = read_pairs(&table)?;
        let mut g = Self::from_pairs(pairs);
        g.table = table;
        anyhow::ensure!(
            !g.names.is_empty(),
            "{}: no cell has a group (all unassigned?)",
            g.table
        );
        Ok(g)
    }

    /// Groups from `(cell, label)` pairs; a later pair for a cell wins.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (Box<str>, Box<str>)>) -> Self {
        let mut n_unassigned = 0;
        let mut labelled: FxHashMap<Box<str>, Box<str>> = FxHashMap::default();
        for (cell, label) in pairs {
            if no_group(&label) {
                n_unassigned += 1;
                labelled.remove(&cell);
            } else {
                labelled.insert(cell, label.trim().into());
            }
        }
        let mut names: Vec<Box<str>> = labelled.values().cloned().collect();
        // By name too, so that names of one natural key (C1, C01) are
        // next to each other for dedup.
        names.sort_by_cached_key(|n| (natural(n), n.clone()));
        names.dedup();
        let index: FxHashMap<&str, usize> =
            names.iter().enumerate().map(|(i, n)| (&**n, i)).collect();
        let mut exact = FxHashMap::default();
        let mut by_base: FxHashMap<Box<str>, Option<usize>> = FxHashMap::default();
        for (cell, label) in &labelled {
            let k = index[&**label];
            if let Some((barcode, _)) = cell.split_once('@') {
                by_base
                    .entry(barcode.into())
                    .and_modify(|e| {
                        if *e != Some(k) {
                            *e = None;
                        }
                    })
                    .or_insert(Some(k));
            }
            exact.insert(cell.clone(), k);
        }
        Self {
            names,
            exact,
            base: by_base,
            table: String::new(),
            n_unassigned,
        }
    }

    /// The group of `cell`, by its name; else a `{barcode}@{batch}` cell
    /// takes the group listed for the bare barcode, and a bare barcode the
    /// group of its one `{barcode}@{batch}`.
    #[must_use]
    pub fn of(&self, cell: &str) -> Option<usize> {
        if let Some(&k) = self.exact.get(cell) {
            return Some(k);
        }
        match cell.split_once('@') {
            Some((barcode, _)) => self.exact.get(barcode).copied(),
            None => self.base.get(cell).copied().flatten(),
        }
    }

    /// Each of `cells`' group, logging how many found one.
    #[must_use]
    pub fn label(&self, cells: &[Box<str>]) -> Vec<Option<usize>> {
        let labels: Vec<Option<usize>> = cells.iter().map(|c| self.of(c)).collect();
        let found = labels.iter().filter(|l| l.is_some()).count();
        log::info!(
            "Groups: {found} of {} cells in {} groups from {} ({} listed unassigned)",
            cells.len(),
            self.names.len(),
            self.table,
            self.n_unassigned
        );
        if found == 0 {
            log::warn!(
                "no cell of the run is named in {}: are they the same cells?",
                self.table
            );
        }
        labels
    }
}

/// The `annotate.argmax` table a lupin round manifest records, as a path.
fn argmax_of(manifest: &str) -> anyhow::Result<String> {
    let text = std::fs::read_to_string(manifest)
        .map_err(|e| anyhow::anyhow!("reading {manifest}: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("{manifest} is not JSON: {e}"))?;
    let rel = v
        .pointer("/annotate/argmax")
        .and_then(|a| a.as_str())
        .ok_or_else(|| {
            anyhow::anyhow!("{manifest} has no annotate.argmax: give an annotated lupin round")
        })?;
    let p = Path::new(rel);
    let dir = Path::new(manifest).parent().unwrap_or(Path::new(""));
    Ok(if p.is_absolute() {
        rel.to_string()
    } else {
        dir.join(p).to_string_lossy().into_owned()
    })
}

/// `(cell, label)` rows of a labels table, by header or by position.
fn read_pairs(path: &str) -> anyhow::Result<Vec<(Box<str>, Box<str>)>> {
    let t = TableReader::open(path).map_err(|e| anyhow::anyhow!("reading groups {path}: {e}"))?;
    let cell = t.find_column(&["cell", "barcode"]);
    let label = t.find_column(&["cell_type", "group", "label"]);
    let mut out = Vec::new();
    let cols = match (cell, label) {
        (Some(c), Some(l)) => vec![c, l],
        (Some(_), None) => anyhow::bail!(
            "{path}: no cell_type, group or label column; the header has {}",
            t.header().join(", ")
        ),
        (None, _) => {
            anyhow::ensure!(
                t.header().len() >= 2,
                "{path}: give cell and cell_type columns, or a cell, group table"
            );
            // No header: the first line is a row.
            out.push((t.header()[0].clone(), t.header()[1].clone()));
            vec![0, 1]
        }
    };
    for row in t.rows(&cols)? {
        let mut row = row?;
        if row[0].is_empty() {
            continue;
        }
        let label = std::mem::take(&mut row[1]);
        out.push((std::mem::take(&mut row[0]), label));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(v: &[(&str, &str)]) -> Vec<(Box<str>, Box<str>)> {
        v.iter().map(|&(c, l)| (c.into(), l.into())).collect()
    }

    #[test]
    fn groups_sort_in_natural_order() {
        let g = Groups::from_pairs(pairs(&[("a", "C10"), ("b", "C2"), ("c", "B"), ("d", "C1")]));
        let names: Vec<&str> = g.names.iter().map(|n| &**n).collect();
        assert_eq!(names, ["B", "C1", "C2", "C10"]);
        let g = Groups::from_pairs(pairs(&[
            ("a", "C01"),
            ("b", "C1"),
            ("c", "C01"),
            ("d", "C1"),
        ]));
        assert_eq!(
            g.names.len(),
            2,
            "one natural key, two names: {:?}",
            g.names
        );
    }

    #[test]
    fn unassigned_cells_have_no_group() {
        let g = Groups::from_pairs(pairs(&[
            ("a", "T"),
            ("b", "unassigned"),
            ("c", "B"),
            ("d", ""),
        ]));
        assert_eq!(g.names, vec![Box::from("B"), Box::from("T")]);
        assert_eq!(g.of("a"), Some(1));
        assert_eq!(g.of("b"), None);
        assert_eq!(g.n_unassigned, 2);
    }

    #[test]
    fn batch_suffixes_join_either_way_unless_ambiguous() {
        let g = Groups::from_pairs(pairs(&[
            ("AC@s1", "T"),
            ("GT", "B"),
            ("TT@s1", "T"),
            ("TT@s2", "B"),
        ]));
        assert_eq!(g.of("AC"), Some(1));
        assert_eq!(g.of("AC@s1"), Some(1));
        assert_eq!(g.of("GT@s9"), Some(0));
        assert_eq!(g.of("TT@s2"), Some(0), "an exact name still joins");
        assert_eq!(g.of("TT"), None, "two groups claim the bare barcode");
        assert_eq!(g.of("AC@s2"), None, "another batch's cell is another cell");
    }

    #[test]
    fn a_lupin_round_is_followed_to_its_argmax() {
        let dir = tempfile::tempdir().unwrap();
        let tsv = dir.path().join("X.L1.argmax.tsv");
        std::fs::write(
            &tsv,
            "cell\tcell_type\tprobability\nAC\tB\t0.9\nGT\tunassigned\tNaN\n",
        )
        .unwrap();
        let manifest = dir.path().join("X.L1.json");
        std::fs::write(&manifest, r#"{"annotate":{"argmax":"X.L1.argmax.tsv"}}"#).unwrap();
        let g = Groups::read(manifest.to_str().unwrap()).unwrap();
        assert_eq!(g.names, vec![Box::from("B")]);
        assert_eq!(g.of("AC@s1"), Some(0));
        assert_eq!(g.of("GT"), None);
    }

    #[test]
    fn a_headerless_table_is_read_by_position() {
        let dir = tempfile::tempdir().unwrap();
        let tsv = dir.path().join("g.tsv");
        std::fs::write(&tsv, "AC\tB\nGT\tT\n").unwrap();
        let g = Groups::read(tsv.to_str().unwrap()).unwrap();
        assert_eq!(g.of("AC"), Some(0));
        assert_eq!(g.of("GT"), Some(1));
    }
}
