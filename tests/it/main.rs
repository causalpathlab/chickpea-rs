//! The integration tests, one binary: the shared fixture is compiled once
//! and every helper of it is some test's.

mod cis_pairs;
mod cli;
mod common;
mod context_links;
mod embed_two_track;
mod mcp;
#[cfg(feature = "tui")]
mod view;
mod workflow;
