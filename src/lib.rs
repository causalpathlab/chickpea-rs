//! chickpea: peak-to-gene cis-regulatory linkage for paired single-cell RNA + ATAC.
//! The library holds the pipeline; `main.rs` is the CLI over it.

pub mod common;
pub mod mcp;
pub mod p2g;
#[cfg(feature = "tui")]
pub mod run;
#[cfg(feature = "tui")]
pub mod tui;
#[cfg(feature = "tui")]
pub mod view;
