//! `estante` library surface.
//!
//! The bin (`src/main.rs`) is a thin clap-derive shell — every
//! load-bearing module is re-exported here so integration tests can
//! drive the same code paths via `use estante::resolver::Resolver` etc.
//! Keeping the modules `pub` here (not just `mod` inside the bin)
//! makes the substrate testable end-to-end without subprocess gymnastics.

#![forbid(unsafe_code)]

pub mod actions;
pub mod cache;
pub mod config;
pub mod fetch;
pub mod hash;
pub mod inline_metadata;
pub mod lockfile_io;
pub mod manifest_io;
pub mod resolver;
