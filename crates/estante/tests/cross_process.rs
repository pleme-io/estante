//! Cross-process determinism + verifiability integration tests.
//!
//! The in-process determinism test in `end_to_end.rs` proves that two
//! consecutive calls into the resolver library yield byte-identical
//! lockfiles. This file proves the stronger claim that downstream
//! consumers actually depend on:
//!
//!   - Two separate invocations of the **estante binary** produce
//!     byte-identical lockfile bytes (and therefore identical BLAKE3
//!     attestations) given the same manifest + source state.
//!
//!   - A lockfile written by one process verifies cleanly when re-
//!     hashed by another process — i.e. the BLAKE3 receipt is a
//!     transferable proof of content, not a process-local accident.
//!
//! These are the load-bearing verifiability properties of the
//! typescape — a CI job that produces a lockfile and an operator who
//! installs it later must agree on the bytes, or estante's "lockfile
//! is a one-line attestation receipt" claim falls.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Path to the estante binary built by cargo for this test target.
fn estante_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_estante"))
}

/// Per-test sandbox rooted at a unique tempdir.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "estante-xp-{name}-{}-{}",
            std::process::id(),
            // monotonic suffix in case tests reuse the same pid.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn join(&self, p: &str) -> PathBuf {
        self.root.join(p)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Write a fake package whose rc.lisp body is one deterministic
/// alias declaration. Returns the directory the manifest's `local:`
/// source should point at.
fn write_pkg(sandbox: &Sandbox, name: &str, value: &str) -> PathBuf {
    let pkg = sandbox.join(&["pkg-", name].concat());
    std::fs::create_dir_all(&pkg).unwrap();
    let body = [
        "(defalias :name \"",
        name,
        "\" :value \"",
        value,
        "\")",
    ]
    .concat();
    std::fs::write(pkg.join("rc.lisp"), body).unwrap();
    pkg
}

/// Write a manifest at `manifest_path` that locks one package named
/// `name` against the local fixture at `pkg_dir`.
fn write_manifest(manifest_path: &Path, name: &str, pkg_dir: &Path) {
    let source_uri = ["local:", &pkg_dir.display().to_string()].concat();
    let body = [
        "(defshellpkg\n  :name \"",
        name,
        "\"\n  :version \"0.1.0\"\n  :source \"",
        &source_uri,
        "\"\n)\n",
    ]
    .concat();
    std::fs::write(manifest_path, body).unwrap();
}

/// Build a Command for the estante binary with a fresh sandbox cache
/// pointed at `cache_root`. Returns the Command before `args` so the
/// caller can attach subcommand-specific flags.
fn estante_cmd(cache_root: &Path) -> Command {
    let mut c = Command::new(estante_bin());
    c.env("XDG_CACHE_HOME", cache_root);
    // Force a clean tracing config so test output is stable.
    c.env("RUST_LOG", "estante=warn");
    c
}

/// Sub-process invocation of `estante lock --manifest M --lockfile L`,
/// asserting clean exit + returning the lockfile bytes.
fn run_lock(manifest: &Path, lockfile: &Path, cache_root: &Path) -> Vec<u8> {
    let out = estante_cmd(cache_root)
        .arg("--manifest")
        .arg(manifest)
        .arg("--lockfile")
        .arg(lockfile)
        .arg("lock")
        .output()
        .expect("spawn estante lock");
    assert!(
        out.status.success(),
        "estante lock failed; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
    std::fs::read(lockfile).expect("read lockfile after lock")
}

/// Sub-process invocation of `estante verify --json` returning the
/// parsed JSON value and the process exit success flag. We accept
/// both clean (exit 0) and drifted (exit non-zero) outcomes — the
/// caller decides which is expected.
fn run_verify_json(
    manifest: &Path,
    lockfile: &Path,
    cache_root: &Path,
    strict: bool,
) -> (serde_json::Value, bool) {
    let mut cmd = estante_cmd(cache_root);
    cmd.arg("--manifest")
        .arg(manifest)
        .arg("--lockfile")
        .arg(lockfile)
        .arg("verify")
        .arg("--json");
    if strict {
        cmd.arg("--strict");
    }
    let out = cmd.output().expect("spawn estante verify");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!(
            "verify --json stdout is not parseable JSON ({e}); stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        ));
    (json, out.status.success())
}

#[test]
fn lock_is_byte_identical_across_two_separate_processes() {
    let sb = Sandbox::new("lock-determinism");
    let pkg = write_pkg(&sb, "alpha", "echo alpha-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "alpha", &pkg);

    // Two separate child processes; each gets its own cache root.
    // If anything in the lock pipeline carried hidden process-local
    // state (PIDs in paths, time-dependent serialization, HashMap
    // iteration order leaking), this test would expose it.
    let cache_a = sb.join("cache-a");
    let cache_b = sb.join("cache-b");
    let lock_a = sb.join("a.lock.lisp");
    let lock_b = sb.join("b.lock.lisp");

    let bytes_a = run_lock(&manifest, &lock_a, &cache_a);
    let bytes_b = run_lock(&manifest, &lock_b, &cache_b);

    // String equality (the wire form).
    assert_eq!(
        bytes_a, bytes_b,
        "lockfiles produced by two separate processes must be byte-identical",
    );

    // BLAKE3 receipt equality — the attestation primitive. Two
    // operators running `estante lock` against the same manifest +
    // sources must derive the same one-line BLAKE3 receipt.
    let receipt_a = blake3::hash(&bytes_a).to_hex().to_string();
    let receipt_b = blake3::hash(&bytes_b).to_hex().to_string();
    assert_eq!(
        receipt_a, receipt_b,
        "BLAKE3 attestation receipts must match across processes",
    );
}

#[test]
fn verify_reports_clean_for_a_freshly_locked_workspace() {
    let sb = Sandbox::new("verify-clean");
    let pkg = write_pkg(&sb, "beta", "echo beta-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "beta", &pkg);

    let cache = sb.join("cache");
    let lockfile = sb.join("shellpkg.lock.lisp");
    run_lock(&manifest, &lockfile, &cache);

    let (report, ok) = run_verify_json(&manifest, &lockfile, &cache, true);
    assert!(ok, "fresh lock must verify clean");

    // The JSON wire shape is the verify contract — pinning it here
    // doubles as a schema test for the report struct.
    let drifted = report.get("drifted").and_then(|v| v.as_array()).unwrap();
    let missing = report.get("missing").and_then(|v| v.as_array()).unwrap();
    let verified = report.get("verified").and_then(|v| v.as_array()).unwrap();
    assert!(drifted.is_empty(), "no drift expected on a fresh lock");
    assert!(missing.is_empty(), "no missing materializations expected");
    assert_eq!(verified.len(), 1, "exactly one entry should verify");
    assert_eq!(verified[0].as_str(), Some("beta"));
}

#[test]
fn verify_detects_drift_when_a_materialized_file_is_tampered() {
    let sb = Sandbox::new("verify-drift");
    let pkg = write_pkg(&sb, "gamma", "echo gamma-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "gamma", &pkg);

    let cache = sb.join("cache");
    let lockfile = sb.join("shellpkg.lock.lisp");
    run_lock(&manifest, &lockfile, &cache);

    // Find the materialized rc.lisp in the cache and modify a byte.
    // We don't hardcode the path — read it back out of the lockfile
    // to avoid coupling to the cache layout.
    let lock_bytes = std::fs::read(&lockfile).unwrap();
    let lock_str = std::str::from_utf8(&lock_bytes).unwrap();
    let materialized_path = lock_str
        .lines()
        .find_map(|line| line.trim().strip_prefix(":materialized-path \""))
        .and_then(|tail| tail.strip_suffix("\""))
        .expect("locked entry must record :materialized-path");
    let rc = Path::new(materialized_path).join("rc.lisp");
    let mut body = std::fs::read(&rc).unwrap();
    body.extend_from_slice(b" ;; tamper");
    std::fs::write(&rc, body).unwrap();

    let (report, ok) = run_verify_json(&manifest, &lockfile, &cache, false);
    assert!(!ok, "tampered tree must exit non-zero");
    let drifted = report.get("drifted").and_then(|v| v.as_array()).unwrap();
    assert_eq!(drifted.len(), 1, "tamper must surface as drift");
    let entry = &drifted[0];
    assert_eq!(entry.get("name").and_then(|v| v.as_str()), Some("gamma"));
    assert!(entry.get("expected").is_some());
    assert!(entry.get("actual").is_some());
    assert!(entry.get("path").is_some());
}
