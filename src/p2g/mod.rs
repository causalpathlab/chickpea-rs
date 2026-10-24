//! peak-to-gene: cis-regulatory links for paired RNA + ATAC.
//! Entry point: [`run::run_peak_to_gene`].
//!
//! RNA genes and ATAC peaks share one multiome axis ([`two_track`]); ATAC is
//! module-only. Cis ABC pairs feed phase-1 ReLU gates (engine mix); link tables
//! come from [`workflow`].

pub mod cis;
pub mod context;
pub mod groups;
pub mod input;
pub mod record;
pub mod regroup;
pub mod run;
pub mod samples;
pub mod two_track;
pub mod workflow;

pub use regroup::{run_regroup, RegroupArgs};
pub use run::{run_peak_to_gene, PeakToGeneArgs};
