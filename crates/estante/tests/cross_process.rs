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

/// `estante attest` produces byte-identical JSON across two
/// independent processes — and therefore matching BLAKE3 receipts.
/// This is the externally-visible attestation primitive: the digest
/// of the receipt is a transferable proof of the manifest + lockfile
/// + per-entry bytes.
#[test]
fn attest_receipt_is_byte_identical_across_two_separate_processes() {
    let sb = Sandbox::new("attest-determinism");
    let pkg = write_pkg(&sb, "delta", "echo delta-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "delta", &pkg);

    // Lock once so both attestation runs see the same lockfile.
    let cache = sb.join("cache");
    let lockfile = sb.join("shellpkg.lock.lisp");
    run_lock(&manifest, &lockfile, &cache);

    let out_a = sb.join("receipt-a.json");
    let out_b = sb.join("receipt-b.json");

    let status_a = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--out")
        .arg(&out_a)
        .status()
        .expect("spawn estante attest #1");
    assert!(status_a.success(), "attest #1 must succeed");

    let status_b = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--out")
        .arg(&out_b)
        .status()
        .expect("spawn estante attest #2");
    assert!(status_b.success(), "attest #2 must succeed");

    let bytes_a = std::fs::read(&out_a).unwrap();
    let bytes_b = std::fs::read(&out_b).unwrap();
    assert_eq!(bytes_a, bytes_b, "receipt JSON must be byte-identical");

    // The receipt is parseable JSON with the canonical shape.
    let json: serde_json::Value = serde_json::from_slice(&bytes_a).unwrap();
    assert_eq!(json["schemaVersion"], 1);
    assert!(json["manifest"]["blake3"].as_str().unwrap().len() > 16);
    assert!(json["lockfile"]["blake3"].as_str().unwrap().len() > 16);
    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "delta");
    assert_eq!(entries[0]["placement"], "cache");
    assert_eq!(entries[0]["materializedExists"], true);

    // BLAKE3 of the receipt — the externally-citable attestation
    // digest. Two operators running attest against the same lock
    // get the same digest, period.
    let digest_a = blake3::hash(&bytes_a).to_hex().to_string();
    let digest_b = blake3::hash(&bytes_b).to_hex().to_string();
    assert_eq!(digest_a, digest_b);
}

/// `estante attest --verify` round-trip — emit a receipt, verify
/// against the unchanged tree (zero exit), then mutate the manifest
/// and confirm verify exits non-zero with manifest.blake3 in the
/// reported drift. Closes the attestation loop end-to-end.
#[test]
fn attest_verify_round_trips_clean_then_fails_on_drift() {
    let sb = Sandbox::new("attest-verify");
    let pkg = write_pkg(&sb, "epsilon", "echo epsilon-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "epsilon", &pkg);

    let cache = sb.join("cache");
    let lockfile = sb.join("shellpkg.lock.lisp");
    run_lock(&manifest, &lockfile, &cache);

    let receipt = sb.join("receipt.json");
    let status = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--out")
        .arg(&receipt)
        .status()
        .expect("spawn attest --out");
    assert!(status.success());

    // Clean verify — zero exit.
    let clean = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--verify")
        .arg(&receipt)
        .output()
        .expect("spawn attest --verify clean");
    assert!(
        clean.status.success(),
        "unmodified state must verify against its own receipt; stderr:\n{}",
        String::from_utf8_lossy(&clean.stderr),
    );

    // Mutate manifest by appending a byte → manifest.blake3 changes
    // → attest --verify must fail.
    let mut body = std::fs::read(&manifest).unwrap();
    body.push(b' ');
    std::fs::write(&manifest, body).unwrap();

    let drifted = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--verify")
        .arg(&receipt)
        .output()
        .expect("spawn attest --verify drifted");
    assert!(
        !drifted.status.success(),
        "mutated manifest must fail receipt verification",
    );
    let stderr = String::from_utf8_lossy(&drifted.stderr);
    assert!(
        stderr.contains("manifest.blake3"),
        "drift report must call out manifest.blake3; stderr:\n{stderr}",
    );
}

/// `estante attest --check` is the ergonomic CI gate: looks for a
/// sibling `shellpkg.receipt.json` next to the manifest, diffs the
/// current state against it, and exits non-zero on drift. Composes
/// with the existing --verify path but with zero path arguments.
#[test]
fn attest_check_passes_for_fresh_sibling_receipt() {
    let sb = Sandbox::new("attest-check-clean");
    let pkg = write_pkg(&sb, "nu", "echo nu-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "nu", &pkg);

    let cache = sb.join("cache");
    let lockfile = sb.join("shellpkg.lock.lisp");
    run_lock(&manifest, &lockfile, &cache);

    // Emit receipt at the canonical sibling path.
    let receipt = sb.join("shellpkg.receipt.json");
    let st = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--out")
        .arg(&receipt)
        .status()
        .expect("attest --out");
    assert!(st.success());

    // --check finds the sibling automatically and exits zero.
    let out = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--check")
        .output()
        .expect("attest --check");
    assert!(
        out.status.success(),
        "fresh sibling receipt must pass --check; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("receipt matches"));
}

#[test]
fn attest_check_fails_with_clear_message_when_no_sibling_exists() {
    let sb = Sandbox::new("attest-check-missing");
    let pkg = write_pkg(&sb, "xi", "echo xi-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "xi", &pkg);

    let cache = sb.join("cache");
    let lockfile = sb.join("shellpkg.lock.lisp");
    run_lock(&manifest, &lockfile, &cache);

    // No receipt emitted → --check should fail with an actionable
    // error pointing at the expected path + the command to fix it.
    let out = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--check")
        .output()
        .expect("attest --check (no sibling)");
    assert!(!out.status.success(), "missing sibling must fail --check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no sibling receipt") && stderr.contains("estante attest"),
        "error message must be actionable; stderr:\n{stderr}",
    );
}

#[test]
fn attest_check_detects_drift_against_sibling_receipt() {
    let sb = Sandbox::new("attest-check-drift");
    let pkg = write_pkg(&sb, "omicron", "echo omicron-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "omicron", &pkg);

    let cache = sb.join("cache");
    let lockfile = sb.join("shellpkg.lock.lisp");
    run_lock(&manifest, &lockfile, &cache);
    let receipt = sb.join("shellpkg.receipt.json");
    let st = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--out")
        .arg(&receipt)
        .status()
        .expect("attest --out");
    assert!(st.success());

    // Mutate manifest → receipt manifest.blake3 no longer matches.
    let mut body = std::fs::read(&manifest).unwrap();
    body.push(b' ');
    std::fs::write(&manifest, body).unwrap();

    let out = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--check")
        .arg("--json")
        .output()
        .expect("attest --check --json");
    assert!(!out.status.success(), "drift must fail --check");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["matched"], serde_json::Value::Bool(false));
    let fields: Vec<&str> = v["diffs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["field"].as_str().unwrap())
        .collect();
    assert!(fields.contains(&"manifest.blake3"));
}

/// `estante lock --check` is the CI gate enforcing "the committed
/// lockfile reflects the committed manifest." Exits zero when the
/// lockfile would not change, non-zero when it would. Mutating the
/// rc.lisp body changes the BLAKE3 → check must fail.
#[test]
fn lock_check_exits_clean_when_lockfile_is_up_to_date() {
    let sb = Sandbox::new("lock-check-clean");
    let pkg = write_pkg(&sb, "kappa", "echo kappa-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "kappa", &pkg);

    let cache = sb.join("cache");
    let lockfile = sb.join("shellpkg.lock.lisp");
    run_lock(&manifest, &lockfile, &cache);

    let out = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("lock")
        .arg("--check")
        .output()
        .expect("spawn lock --check");
    assert!(
        out.status.success(),
        "freshly locked workspace must pass lock --check; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("up to date"));
}

#[test]
fn lock_check_exits_dirty_when_manifest_adds_a_package() {
    let sb = Sandbox::new("lock-check-drift");
    let pkg = write_pkg(&sb, "lambda", "echo lambda-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "lambda", &pkg);

    let cache = sb.join("cache");
    let lockfile = sb.join("shellpkg.lock.lisp");
    run_lock(&manifest, &lockfile, &cache);

    // Add a second package to the manifest without rerunning lock —
    // this is the exact "manifest edit got merged without an
    // updated lockfile" failure mode `lock --check` exists to catch.
    let pkg2 = write_pkg(&sb, "mu", "echo mu-value");
    let pkg2_uri = ["local:", &pkg2.display().to_string()].concat();
    let extra = [
        "\n(defshellpkg\n  :name \"mu\"\n  :version \"0.1.0\"\n  :source \"",
        &pkg2_uri,
        "\"\n)\n",
    ]
    .concat();
    let mut body = std::fs::read(&manifest).unwrap();
    body.extend_from_slice(extra.as_bytes());
    std::fs::write(&manifest, body).unwrap();

    let out = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("lock")
        .arg("--check")
        .output()
        .expect("spawn lock --check drift");
    assert!(
        !out.status.success(),
        "drifted manifest must fail lock --check",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("out of date") && stderr.contains("blake3"),
        "drift report must explain the mismatch; stderr:\n{stderr}",
    );
}

/// `estante doctor --json` emits a structured report whose schema
/// is the CI contract: { checks: [...], passed, failed, total }.
/// Pinned here so accidental shape drift breaks the test before it
/// breaks downstream CI parsers.
#[test]
fn doctor_json_output_is_parseable_and_schema_stable() {
    let sb = Sandbox::new("doctor-json");
    let pkg = write_pkg(&sb, "theta", "echo theta-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "theta", &pkg);

    let cache = sb.join("cache");
    let lockfile = sb.join("shellpkg.lock.lisp");
    run_lock(&manifest, &lockfile, &cache);

    let out = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("doctor")
        .arg("--json")
        .output()
        .expect("spawn doctor --json");
    assert!(out.status.success(), "doctor --json failed; stderr:\n{}", String::from_utf8_lossy(&out.stderr));

    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("doctor --json stdout not parseable JSON ({e})\nstdout:\n{}", String::from_utf8_lossy(&out.stdout)));
    assert!(v.get("checks").is_some(), "missing checks array");
    assert!(v.get("passed").and_then(|x| x.as_u64()).is_some());
    assert!(v.get("failed").and_then(|x| x.as_u64()).is_some());
    assert!(v.get("total").and_then(|x| x.as_u64()).is_some());
    assert_eq!(
        v["passed"].as_u64().unwrap() + v["failed"].as_u64().unwrap(),
        v["total"].as_u64().unwrap(),
        "passed + failed must sum to total",
    );

    let checks = v["checks"].as_array().unwrap();
    assert!(!checks.is_empty(), "at least one check must run");
    for c in checks {
        assert!(c.get("name").and_then(|n| n.as_str()).is_some());
        assert!(c.get("passed").and_then(|p| p.as_bool()).is_some());
        assert!(c.get("detail").and_then(|d| d.as_str()).is_some());
    }
}

/// `estante attest --verify --json` emits a structured diff payload
/// with the same shape regardless of whether the receipt matched.
/// On match: { matched: true, diffs: [] }. On drift: { matched:
/// false, diffs: [{ field, claimed, actual }] }. Process exits
/// non-zero on drift.
#[test]
fn attest_verify_json_output_shape_is_stable_clean_and_drifted() {
    let sb = Sandbox::new("attest-verify-json");
    let pkg = write_pkg(&sb, "iota", "echo iota-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "iota", &pkg);

    let cache = sb.join("cache");
    let lockfile = sb.join("shellpkg.lock.lisp");
    run_lock(&manifest, &lockfile, &cache);

    let receipt = sb.join("receipt.json");
    let st = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--out")
        .arg(&receipt)
        .status()
        .expect("attest --out");
    assert!(st.success());

    // Clean case: matched=true, diffs=[].
    let clean = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--verify")
        .arg(&receipt)
        .arg("--json")
        .output()
        .expect("attest --verify --json clean");
    assert!(clean.status.success());
    let v: serde_json::Value = serde_json::from_slice(&clean.stdout).unwrap();
    assert_eq!(v["matched"], serde_json::Value::Bool(true));
    assert_eq!(v["diffs"].as_array().unwrap().len(), 0);
    assert_eq!(v["entryCount"].as_u64().unwrap(), 1);

    // Drift case: mutate, expect matched=false + diffs populated.
    let mut body = std::fs::read(&manifest).unwrap();
    body.push(b' ');
    std::fs::write(&manifest, body).unwrap();

    let drifted = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--verify")
        .arg(&receipt)
        .arg("--json")
        .output()
        .expect("attest --verify --json drifted");
    assert!(!drifted.status.success());
    let v: serde_json::Value = serde_json::from_slice(&drifted.stdout).unwrap();
    assert_eq!(v["matched"], serde_json::Value::Bool(false));
    let diffs = v["diffs"].as_array().unwrap();
    assert!(!diffs.is_empty(), "drift must populate diffs");
    let fields: Vec<&str> = diffs.iter().map(|d| d["field"].as_str().unwrap()).collect();
    assert!(fields.contains(&"manifest.blake3"));
    for d in diffs {
        assert!(d.get("claimed").and_then(|v| v.as_str()).is_some());
        assert!(d.get("actual").and_then(|v| v.as_str()).is_some());
    }
}

/// `estante doctor` finds a sibling `shellpkg.receipt.json` and
/// fails the `receipt:matches` check when the manifest is mutated
/// after the receipt was emitted. Closes the "did a published
/// attestation go stale?" loop at the operator-facing entry point.
#[test]
fn doctor_fails_when_sibling_receipt_does_not_match_current_state() {
    let sb = Sandbox::new("doctor-receipt");
    let pkg = write_pkg(&sb, "zeta", "echo zeta-value");
    let manifest = sb.join("shellpkg.lisp");
    write_manifest(&manifest, "zeta", &pkg);

    let cache = sb.join("cache");
    let lockfile = sb.join("shellpkg.lock.lisp");
    run_lock(&manifest, &lockfile, &cache);

    let receipt = sb.join("shellpkg.receipt.json");
    let st = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("attest")
        .arg("--out")
        .arg(&receipt)
        .status()
        .expect("attest --out");
    assert!(st.success());

    // Doctor on the unmodified tree passes.
    let clean = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("doctor")
        .output()
        .expect("spawn doctor clean");
    assert!(
        clean.status.success(),
        "clean doctor must pass; stderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&clean.stderr),
        String::from_utf8_lossy(&clean.stdout),
    );
    let stdout = String::from_utf8_lossy(&clean.stdout);
    assert!(
        stdout.contains("receipt:matches"),
        "doctor output must include receipt:matches check; stdout:\n{stdout}",
    );

    // Mutate the manifest → receipt should no longer match → doctor
    // exits non-zero with receipt:matches in the failing checks.
    let mut body = std::fs::read(&manifest).unwrap();
    body.push(b' ');
    std::fs::write(&manifest, body).unwrap();

    let drifted = estante_cmd(&cache)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--lockfile")
        .arg(&lockfile)
        .arg("doctor")
        .output()
        .expect("spawn doctor drifted");
    let stdout = String::from_utf8_lossy(&drifted.stdout);
    assert!(
        !drifted.status.success(),
        "mutated manifest must fail doctor; stdout:\n{stdout}",
    );
    assert!(
        stdout.contains("receipt:matches"),
        "drift report must call out receipt:matches; stdout:\n{stdout}",
    );
}
