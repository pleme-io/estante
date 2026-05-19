//! Snapshot tests via insta — wire-format stability for the typed
//! renderers.
//!
//! Each snapshot pins the exact bytes a renderer emits. If anything
//! about the format changes (whitespace, field order, escape rules,
//! newline placement), the test fails until the operator reviews +
//! accepts via `cargo insta review`. This is the determinism guard
//! for the lockfile-as-contract — downstream Nix consumers depend
//! on these bytes being stable.
//!
//! Snapshots live in `tests/snapshots/` next to this file.

#![cfg(test)]

use estante_types::{LockedPkgSpec, Lockfile, Manifest, PkgSpec, nix_export};

fn fixed_manifest() -> Manifest {
    let mut m = Manifest::default();
    m.upsert(PkgSpec {
        name: "alpha".into(),
        version: "1.0.0".into(),
        source: "github:org/alpha@v1.0.0".into(),
        exports: vec![],
        deps: vec![],
        lazy: false,
    });
    m.upsert(PkgSpec {
        name: "beta".into(),
        version: "0.2.0".into(),
        source: "github:org/beta@v0.2.0".into(),
        exports: vec!["alias".into(), "hook".into()],
        deps: vec!["alpha@^1.0".into()],
        lazy: true,
    });
    m
}

fn fixed_lockfile() -> Lockfile {
    let mut l = Lockfile::default();
    l.upsert(LockedPkgSpec {
        name: "alpha".into(),
        source: "github:org/alpha".into(),
        rev: "abcdef0123456789abcdef0123456789abcdef01".into(),
        nar_hash: "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
        blake3: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        materialized_path: "/nix/store/abc-alpha-1.0.0/".into(),
        placement: "nix".into(),
    });
    l.upsert(LockedPkgSpec {
        name: "beta".into(),
        source: "local:/abs/path/to/beta".into(),
        rev: "deadbeefcafef00d".into(),
        nar_hash: String::new(),
        blake3: "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210".into(),
        materialized_path: "/Users/op/.cache/estante/store/beta-deadbeefcafef00d/".into(),
        placement: "cache".into(),
    });
    l.upsert(LockedPkgSpec {
        name: "gamma".into(),
        source: "github:org/gamma@v3.0".into(),
        rev: "abcdef0123456789abcdef0123456789abcdef02".into(),
        nar_hash: "sha256-BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=".into(),
        blake3: "1111111111111111111111111111111111111111111111111111111111111111".into(),
        materialized_path: "/nix/store/def-gamma-3.0/".into(),
        placement: "both".into(),
    });
    l
}

#[test]
fn snapshot_manifest_display() {
    insta::assert_snapshot!("manifest_display", fixed_manifest().to_string());
}

#[test]
fn snapshot_lockfile_display() {
    insta::assert_snapshot!("lockfile_display", fixed_lockfile().to_string());
}

#[test]
fn snapshot_nix_export() {
    insta::assert_snapshot!(
        "lockfile_nix_export",
        nix_export::lockfile_to_nix(&fixed_lockfile())
    );
}

#[test]
fn snapshot_empty_lockfile_display() {
    insta::assert_snapshot!("empty_lockfile_display", Lockfile::default().to_string());
}

#[test]
fn snapshot_empty_manifest_display() {
    insta::assert_snapshot!("empty_manifest_display", Manifest::default().to_string());
}
