//! Gene positions from a table (`gene`, `chr`, `tss`, with a header), as
//! the multiome simulator writes them.

use crate::common::*;
use genomic_data::coordinates::{align_gene_tss, GeneTss};
use legume_numeric::matrix::table::TableReader;

/// Load gene TSS positions from a table with a header naming its `gene`,
/// `chr` (or `chromosome`) and `tss` columns, in any order (tab, comma or
/// whitespace delimited, `.gz` fine, or parquet). Produced by sim-link as
/// {out}.gene_coords.tsv.gz.
///
/// Matching uses [`align_gene_tss`] (`GeneIndexResolver`: ENSG / HGNC /
/// `ENSG…_SYMBOL`), same as the GFF path.
pub fn load_gene_coords_tsv(
    path: &str,
    gene_names: &[Box<str>],
) -> anyhow::Result<Vec<Option<GeneTss>>> {
    let t =
        TableReader::open(path).map_err(|e| anyhow::anyhow!("reading gene coords {path}: {e}"))?;
    let col = |names: &[&str]| {
        t.find_column(names).ok_or_else(|| {
            anyhow::anyhow!(
                "{path}: no {} column; give a header of gene, chr and tss (it has {})",
                names.join(" or "),
                t.header().join(", ")
            )
        })
    };
    let cols = [
        col(&["gene"])?,
        col(&["chr", "chromosome"])?,
        col(&["tss"])?,
    ];
    let mut tss_map: rustc_hash::FxHashMap<Box<str>, GeneTss> = Default::default();
    for (i, row) in t.rows(&cols)?.enumerate() {
        // Data rows count from 1, the header not counted.
        let row = row.map_err(|e| anyhow::anyhow!("{path}: row {}: {e}", i + 1))?;
        let [gene, chr, tss] =
            <[Box<str>; 3]>::try_from(row).expect("a row of the three columns asked for");
        if gene.is_empty() {
            continue;
        }
        let tss: i64 = tss.parse().map_err(|e| {
            anyhow::anyhow!("{path}: row {} (gene {gene}): tss `{tss}`: {e}", i + 1)
        })?;
        tss_map.insert(gene, GeneTss { chr, tss });
    }

    info!(
        "Loaded {} gene positions from {}, matching against {} genes",
        tss_map.len(),
        path,
        gene_names.len()
    );

    let result = align_gene_tss(gene_names, &tss_map);
    let matched = result.iter().filter(|x| x.is_some()).count();
    info!(
        "Matched {}/{} genes to coordinates",
        matched,
        gene_names.len()
    );

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_found_by_header_and_bad_rows_named() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.tsv");
        std::fs::write(&path, "tss\tchromosome\tgene\n100\tchr1\tA\n200\tchr2\tB\n").unwrap();
        let p = path.to_str().unwrap();
        let genes: Vec<Box<str>> = vec!["B".into(), "A".into(), "C".into()];
        let at = load_gene_coords_tsv(p, &genes).unwrap();
        assert_eq!(
            at[0].as_ref().map(|g| (&*g.chr, g.tss)),
            Some(("chr2", 200))
        );
        assert_eq!(at[1].as_ref().map(|g| g.tss), Some(100));
        assert!(at[2].is_none());

        std::fs::write(&path, "gene\tchr\ttss\nA\tchr1\t100\nB\tchr1\tx\n").unwrap();
        let err = load_gene_coords_tsv(p, &genes).unwrap_err().to_string();
        assert!(err.contains(p) && err.contains("row 2"), "{err}");
    }
}
