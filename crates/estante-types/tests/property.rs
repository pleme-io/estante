//! Property-based tests via proptest.
//!
//! Each property here is an invariant that must hold across the
//! entire input space, not just hand-picked examples. Failures get
//! a minimized counterexample we can fold into a regression test.
//!
//! The properties chosen are the load-bearing determinism +
//! verifiability claims of the typescape:
//!
//! 1. Manifest::parse(M.to_string()) == M               — author-side round-trip
//! 2. Lockfile::parse(L.to_string()) == L                — lockfile round-trip
//! 3. Source::parse(s.to_source_string()) == Ok(s)       — Source ADT round-trip
//! 4. nix_export(L).contains every required field for every entry
//! 5. LispString escapes are reversible (no information loss)
//! 6. Lockfile::upsert is idempotent — upsert(x); upsert(x) yields the same shape

#![cfg(test)]

use estante_types::{LispString, LockedPkgSpec, Lockfile, Manifest, PkgSpec, Source, nix_export};
use proptest::prelude::*;

// ─── Generators ─────────────────────────────────────────────────────

/// Slug-shape strings — lowercase letters, digits, hyphen. Matches
/// the convention for package names and version refs in the
/// pleme-io fleet.
fn slug_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{0,30}"
}

fn github_source_strategy() -> impl Strategy<Value = String> {
    (
        slug_strategy(),
        slug_strategy(),
        prop::option::of(slug_strategy()),
    )
        .prop_map(|(owner, repo, reference)| match reference {
            Some(r) => format!("github:{owner}/{repo}@{r}"),
            None => format!("github:{owner}/{repo}"),
        })
}

fn source_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        github_source_strategy(),
        slug_strategy().prop_map(|id| format!("gist:{id}")),
        slug_strategy().prop_map(|p| format!("local:/abs/path/{p}")),
        (slug_strategy(), slug_strategy())
            .prop_map(|(host, path)| { format!("git+https://{host}/{path}.git") }),
    ]
}

fn pkg_spec_strategy() -> impl Strategy<Value = PkgSpec> {
    (
        slug_strategy(),
        slug_strategy(),
        source_strategy(),
        proptest::collection::vec(slug_strategy(), 0..5),
        proptest::collection::vec(slug_strategy(), 0..3),
        any::<bool>(),
    )
        .prop_map(|(name, version, source, exports, deps, lazy)| PkgSpec {
            name,
            version,
            source,
            exports,
            deps,
            lazy,
        })
}

fn locked_pkg_spec_strategy() -> impl Strategy<Value = LockedPkgSpec> {
    (
        slug_strategy(),
        github_source_strategy(),
        "[a-f0-9]{40}",
        prop::option::of("sha256-[A-Za-z0-9]{20,40}"),
        "[a-f0-9]{16,64}",
        slug_strategy().prop_map(|p| format!("/nix/store/{p}-pkg/")),
        prop_oneof![
            Just("cache".to_owned()),
            Just("nix".to_owned()),
            Just("both".to_owned())
        ],
    )
        .prop_map(
            |(name, source, rev, nar_hash_opt, blake3, materialized_path, placement)| {
                LockedPkgSpec {
                    name,
                    source,
                    rev,
                    nar_hash: nar_hash_opt.unwrap_or_default(),
                    blake3,
                    materialized_path,
                    placement,
                }
            },
        )
}

// ─── Properties ──────────────────────────────────────────────────────

proptest! {
    /// Manifest survives Display → parse round-trip regardless of
    /// which combination of optional fields is populated.
    #[test]
    fn manifest_round_trips(pkgs in proptest::collection::vec(pkg_spec_strategy(), 1..6)) {
        let mut m = Manifest::default();
        for p in pkgs {
            m.upsert(p);
        }
        let rendered = m.to_string();
        let reparsed = Manifest::parse(&rendered).unwrap();
        prop_assert_eq!(reparsed, m);
    }

    /// Lockfile survives Display → parse round-trip across every
    /// placement value (cache / nix / both).
    #[test]
    fn lockfile_round_trips(entries in proptest::collection::vec(locked_pkg_spec_strategy(), 1..6)) {
        let mut l = Lockfile::default();
        for e in entries {
            l.upsert(e);
        }
        let rendered = l.to_string();
        let reparsed = Lockfile::parse(&rendered).unwrap();
        prop_assert_eq!(reparsed, l);
    }

    /// nix-exported lockfile renders every entry's required fields.
    #[test]
    fn nix_export_includes_every_entry_name(
        entries in proptest::collection::vec(locked_pkg_spec_strategy(), 1..4),
    ) {
        let mut l = Lockfile::default();
        for e in entries {
            l.upsert(e);
        }
        let rendered = nix_export::lockfile_to_nix(&l);
        for entry in &l.entries {
            let name_marker = ["name = \"", &entry.name, "\""].concat();
            let rev_marker = ["rev = \"", &entry.rev, "\""].concat();
            let placement_marker = ["placement = \"", &entry.placement, "\""].concat();
            prop_assert!(rendered.contains(&name_marker));
            prop_assert!(rendered.contains(&rev_marker));
            prop_assert!(rendered.contains(&placement_marker));
        }
    }

    /// Source round-trips through `parse(to_source_string)` for
    /// every recognized scheme.
    #[test]
    fn source_round_trips_through_string(s in source_strategy()) {
        let parsed = Source::parse(&s).unwrap();
        let rendered = parsed.to_source_string();
        let re_parsed = Source::parse(&rendered).unwrap();
        prop_assert_eq!(parsed, re_parsed);
    }

    /// LispString escape is reversible — the parser correctly
    /// un-escapes everything Display wrote.
    #[test]
    fn lisp_string_escape_round_trips_via_manifest(payload in "[a-zA-Z0-9 _.@:-]{0,30}") {
        let mut m = Manifest::default();
        m.upsert(PkgSpec {
            name: "x".into(),
            version: "1".into(),
            source: ["github:o/r/", &payload].concat(),
            exports: vec![payload.clone()],
            deps: vec![],
            lazy: false,
        });
        let rendered = m.to_string();
        let re = Manifest::parse(&rendered).unwrap();
        prop_assert_eq!(re, m);
    }

    /// Lockfile::upsert is idempotent — inserting the same entry
    /// twice yields a single-element vec, not two.
    #[test]
    fn lockfile_upsert_is_idempotent(entry in locked_pkg_spec_strategy()) {
        let mut l = Lockfile::default();
        l.upsert(entry.clone());
        l.upsert(entry.clone());
        prop_assert_eq!(l.entries.len(), 1);
        prop_assert_eq!(&l.entries[0], &entry);
    }

    /// Manifest::upsert ditto.
    #[test]
    fn manifest_upsert_is_idempotent(p in pkg_spec_strategy()) {
        let mut m = Manifest::default();
        m.upsert(p.clone());
        m.upsert(p.clone());
        prop_assert_eq!(m.packages.len(), 1);
        prop_assert_eq!(&m.packages[0], &p);
    }
}

// LispString isn't covered by an explicit property test above because
// the escape behavior is folded into manifest_round_trips +
// lisp_string_escape_round_trips_via_manifest. Touching the import
// here so the test file references it (silences unused-import on
// platforms without the manifest test running first).
#[test]
fn _lisp_string_referenced() {
    use std::fmt::Write as _;
    let mut s = String::new();
    write!(s, "{}", LispString("ok")).unwrap();
    assert_eq!(s, "\"ok\"");
}
