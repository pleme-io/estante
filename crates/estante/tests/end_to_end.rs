//! End-to-end integration test — proves the substrate works.
//!
//! Drives the full pipeline using `local:` sources so the test is
//! offline + deterministic:
//!
//! ```text
//! Manifest (local: src)         author authors
//!     │
//!     ▼
//! estante::Resolver::resolve    fetch (filesystem copy) + unpack + hash
//!     │
//!     ▼
//! Lockfile                      `(deflockedpkg …)`
//!     │
//!     ▼
//! frost-lisp::load_rc           consumer's `.frostrc.lisp` does
//!                                `(defsource :path "./shellpkg.lock.lisp")`
//!                                `(defload   :pkg "fixture")`
//!     │
//!     ▼
//! ShellEnv                      alias from the package's rc.lisp now lives
//!                                in env.aliases — proves the chain holds.
//! ```
//!
//! Determinism assertion: running the resolver twice over the same
//! fixture produces byte-identical lockfile content (and identical
//! BLAKE3 over the rendered lockfile). This is the verifiability
//! claim — the lockfile is content-addressed all the way down.

use std::path::PathBuf;

use estante::config::Config;
use estante::resolver::Resolver;
use estante_types::{Lockfile, Manifest, PkgSpec};
use frost_exec::ShellEnv;
use frost_lisp::load_rc;

/// One self-contained fixture rooted at `root`. Drops itself on exit.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(test_name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("estante-e2e-{test_name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Write a fake package at `<fixture-root>/pkg-<name>/rc.lisp` whose
/// content is one alias declaration. Returns the path the manifest's
/// `local:` source should point at.
fn write_fixture_pkg(fx: &Fixture, name: &str, alias_value: &str) -> PathBuf {
    let pkg_dir = fx.root.join(format!("pkg-{name}"));
    std::fs::create_dir_all(&pkg_dir).unwrap();
    let body = format!(r#"(defalias :name "{name}" :value "{alias_value}")"#);
    std::fs::write(pkg_dir.join("rc.lisp"), body).unwrap();
    pkg_dir
}

/// Build an estante Config rooted at this fixture's tempdir (no
/// global cache pollution; concurrent test runs don't collide).
fn fixture_config(fx: &Fixture) -> Config {
    Config {
        cache_dir: fx.root.join("cache"),
        github_token: None,
    }
}

#[tokio::test]
async fn local_pipeline_yields_loadable_lockfile() {
    let fx = Fixture::new("loadable");
    let pkg_a = write_fixture_pkg(&fx, "alpha", "echo alpha-value");

    let mut manifest = Manifest::default();
    manifest.upsert(PkgSpec {
        name: "alpha".into(),
        version: "0.1.0".into(),
        source: format!("local:{}", pkg_a.display()),
        exports: vec!["alias".into()],
        deps: vec![],
        lazy: false,
    });

    let cfg = fixture_config(&fx);
    let resolver = Resolver::new(&cfg).unwrap();
    let lockfile = resolver.resolve(&manifest).await.unwrap();

    // The lockfile has exactly the expected shape.
    assert_eq!(lockfile.entries.len(), 1);
    let entry = &lockfile.entries[0];
    assert_eq!(entry.name, "alpha");
    assert!(
        !entry.materialized_path.is_empty(),
        "materialized path should be populated"
    );
    assert!(
        std::path::Path::new(&entry.materialized_path)
            .join("rc.lisp")
            .is_file(),
        "rc.lisp should have been copied to the cache"
    );
    assert!(
        !entry.blake3.is_empty(),
        "blake3 should be populated by the local-copy fetcher"
    );

    // Render the lockfile to disk; consumer's .frostrc.lisp
    // defsources it then defloads. Same path frost takes at startup.
    let lock_path = fx.root.join("shellpkg.lock.lisp");
    std::fs::write(&lock_path, lockfile.to_string()).unwrap();
    let frostrc = fx.root.join("frostrc.lisp");
    std::fs::write(
        &frostrc,
        format!(
            r#"
            (defsource :path "{lock}")
            (defload   :pkg "alpha")
            "#,
            lock = lock_path.display(),
        ),
    )
    .unwrap();

    // Apply the rc.lisp through frost-lisp's real load_rc — same path
    // a real frost startup takes. If this works, the substrate is
    // genuinely end-to-end.
    let mut env = ShellEnv::new();
    let summary = load_rc(&frostrc, &mut env).unwrap();
    assert_eq!(summary.loads, 1, "exactly one defload should fire");
    assert_eq!(
        env.aliases.get("alpha").map(String::as_str),
        Some("echo alpha-value"),
        "the package's defalias should be in env.aliases"
    );
}

#[tokio::test]
async fn resolver_output_is_deterministic_across_runs() {
    let fx = Fixture::new("determinism");
    let pkg_a = write_fixture_pkg(&fx, "beta", "echo beta-value");
    let pkg_b = write_fixture_pkg(&fx, "gamma", "echo gamma-value");

    let mut manifest = Manifest::default();
    manifest.upsert(PkgSpec {
        name: "beta".into(),
        version: "0.1.0".into(),
        source: format!("local:{}", pkg_a.display()),
        exports: vec![],
        deps: vec![],
        lazy: false,
    });
    manifest.upsert(PkgSpec {
        name: "gamma".into(),
        version: "0.1.0".into(),
        source: format!("local:{}", pkg_b.display()),
        exports: vec![],
        deps: vec![],
        lazy: false,
    });

    let cfg = fixture_config(&fx);
    let resolver = Resolver::new(&cfg).unwrap();
    let lock1 = resolver.resolve(&manifest).await.unwrap();
    let lock2 = resolver.resolve(&manifest).await.unwrap();

    // Lockfile equality (the typed model).
    assert_eq!(lock1, lock2, "typed lockfile must compare equal");

    // Rendered byte-equality (the wire form).
    let rendered1 = lock1.to_string();
    let rendered2 = lock2.to_string();
    assert_eq!(
        rendered1, rendered2,
        "rendered lockfile must be byte-identical"
    );

    // Content-addressing — BLAKE3 of the rendered lockfile is itself
    // stable. This is the verifiability anchor: anyone who fetches the
    // same sources at the same revs gets the same lockfile, and the
    // BLAKE3 of that lockfile is a one-line attestation receipt.
    let blake1 = blake3::hash(rendered1.as_bytes()).to_hex().to_string();
    let blake2 = blake3::hash(rendered2.as_bytes()).to_hex().to_string();
    assert_eq!(blake1, blake2);
}

#[tokio::test]
async fn lockfile_validates_then_renders_then_re_parses_identically() {
    let fx = Fixture::new("roundtrip");
    let pkg = write_fixture_pkg(&fx, "delta", "echo delta-value");

    let mut manifest = Manifest::default();
    manifest.upsert(PkgSpec {
        name: "delta".into(),
        version: "0.1.0".into(),
        source: format!("local:{}", pkg.display()),
        exports: vec!["alias".into()],
        deps: vec![],
        lazy: true,
    });

    let cfg = fixture_config(&fx);
    let resolver = Resolver::new(&cfg).unwrap();
    let lock = resolver.resolve(&manifest).await.unwrap();
    lock.validate_materialized().unwrap();

    let rendered = lock.to_string();
    let reparsed = Lockfile::parse(&rendered).unwrap();
    assert_eq!(
        reparsed, lock,
        "typed lockfile must be invariant under render + parse"
    );
}

#[tokio::test]
async fn cache_hit_short_circuits_second_resolve() {
    // First resolve fetches; second resolve should observe the cache
    // is_unpacked_pkg() and skip the work. The materialized_path
    // must be IDENTICAL across both passes — same content-addressed
    // store layout.
    let fx = Fixture::new("cache-hit");
    let pkg = write_fixture_pkg(&fx, "epsilon", "echo epsilon-value");

    let mut manifest = Manifest::default();
    manifest.upsert(PkgSpec {
        name: "epsilon".into(),
        version: "0.1.0".into(),
        source: format!("local:{}", pkg.display()),
        exports: vec![],
        deps: vec![],
        lazy: false,
    });

    let cfg = fixture_config(&fx);
    let resolver = Resolver::new(&cfg).unwrap();
    let lock1 = resolver.resolve(&manifest).await.unwrap();
    let lock2 = resolver.resolve(&manifest).await.unwrap();
    assert_eq!(
        lock1.entries[0].materialized_path, lock2.entries[0].materialized_path,
        "materialized path is content-addressed by rev; must not change"
    );
    // Note: blake3 on the cache-hit path is empty by design (v0.1);
    // M1d will re-hash from disk for stability.
}
