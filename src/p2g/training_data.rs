//! data-beans' cells and pseudobulks as the graph embedding's training data:
//! the embedding reads them through its own interfaces and depends on no
//! storage crate, so the two are joined here.

use data_beans::bipartite_graph_embedding_input::{
    BipartiteEmbeddingCells, BipartiteEmbeddingPseudobulks,
};
use graph_embedding_util as ge;
use nalgebra::DMatrix;
use std::borrow::Cow;
use std::ops::Range;

/// The unified cells' counts on their compact feature axis.
pub(crate) struct Cells<'a>(pub &'a BipartiteEmbeddingCells);

impl ge::CellCountStream for Cells<'_> {
    fn n_cells(&self) -> usize {
        self.0.n_cells()
    }

    fn n_features(&self) -> usize {
        self.0.n_features()
    }

    fn nnz_hint(&self) -> Option<usize> {
        self.0.nnz_hint()
    }

    fn for_each_count(
        &self,
        cells: Range<usize>,
        f: &mut dyn FnMut(u32, u32, f32),
    ) -> anyhow::Result<()> {
        self.0.for_each_count(cells, f)
    }
}

/// The pseudobulk levels collapsed from those cells.
pub(crate) struct Pseudobulks<'a>(pub &'a BipartiteEmbeddingPseudobulks);

impl ge::PseudobulkLevels for Pseudobulks<'_> {
    fn n_levels(&self) -> usize {
        self.0.n_levels()
    }

    fn rates(&self, level: usize) -> Cow<'_, DMatrix<f32>> {
        self.0.rates(level)
    }

    fn cell_to_pb(&self, level: usize) -> &[usize] {
        self.0.cell_to_pb(level)
    }

    fn finest_counts(&self) -> anyhow::Result<(DMatrix<f32>, Vec<f32>)> {
        self.0.finest_counts()
    }

    fn finest_delta(&self) -> Option<Cow<'_, DMatrix<f32>>> {
        self.0.finest_delta()
    }
}
