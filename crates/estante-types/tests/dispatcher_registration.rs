//! Verify estante-types registers Placement + Source into the
//! gen-platform fleet catalog. estante is the THIRTEENTH
//! consumer class — the typed pleme-io shell-package manager
//! (cargo for shell).
//!
//! Two registrations:
//! - estante.placement (3 unit variants — cache / nix / both)
//! - estante.source (5 named-field variants — github / git-https /
//!   git-ssh / gist / local)

use estante_types::{Placement, Source};
use gen_platform::{catalog, TypedDispatcherTrait};

#[test]
fn placement_registers() {
    let entry = catalog::by_label("estante.placement")
        .expect("estante-types must register Placement");
    assert_eq!((entry.variant_count)(), 3);
}

#[test]
fn source_registers() {
    let entry = catalog::by_label("estante.source")
        .expect("estante-types must register Source");
    assert_eq!((entry.variant_count)(), 5);
}

#[test]
fn placement_variant_kinds_kebab() {
    assert_eq!(Placement::variant_kinds(), vec!["cache", "nix", "both"]);
}

#[test]
fn source_variant_kinds_kebab() {
    assert_eq!(
        Source::variant_kinds(),
        vec!["github", "git-https", "git-ssh", "gist", "local"]
    );
}

#[test]
fn placement_round_trip() {
    for v in [Placement::Cache, Placement::Nix, Placement::Both] {
        let k = v.discriminant();
        let back: Placement = k
            .parse()
            .unwrap_or_else(|_| panic!("FromStr accept own discriminant: {k}"));
        assert_eq!(back.discriminant(), v.discriminant());
    }
}

#[test]
fn placement_predicates() {
    let nix = Placement::Nix;
    assert!(nix.is_nix());
    assert!(!nix.is_cache());
    assert!(!nix.is_both());
}

#[test]
fn source_predicates_structural_vs_semantic() {
    // The hand-rolled is_github_platform() returns true for BOTH
    // Github and Gist (gists resolve via the GitHub API).
    // The auto-derived is_github() is STRUCTURAL — only true for
    // the Github variant. These coexist cleanly post-rename.
    let github = Source::Github {
        owner: "x".into(),
        repo: "y".into(),
        reference: "main".into(),
    };
    let gist = Source::Gist {
        id: "abc".into(),
        reference: "HEAD".into(),
    };

    // Structural (auto-derived by IsVariant)
    assert!(github.is_github());
    assert!(!gist.is_github());
    assert!(gist.is_gist());
    assert!(!github.is_gist());

    // Semantic (hand-rolled)
    assert!(github.is_github_platform());
    assert!(gist.is_github_platform());
}

#[test]
fn source_discriminant_returns_kebab() {
    let github = Source::Github {
        owner: "x".into(),
        repo: "y".into(),
        reference: "main".into(),
    };
    assert_eq!(github.discriminant(), "github");

    let git_https = Source::GitHttps {
        url: "https://x.org/y.git".into(),
        reference: "HEAD".into(),
    };
    assert_eq!(git_https.discriminant(), "git-https");
}
