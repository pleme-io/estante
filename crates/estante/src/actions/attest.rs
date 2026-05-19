//! `estante attest` — emit a deterministic JSON attestation receipt.
//!
//! The receipt is estante's tameshi-chain anchor: a one-line BLAKE3
//! of this JSON is a transferable proof that
//!
//!   1. The manifest at `manifest.path` was hashed at `manifest.blake3`.
//!   2. The lockfile at `lockfile.path` was hashed at `lockfile.blake3`.
//!   3. Each materialized entry produces the recorded BLAKE3.
//!
//! Anyone holding the receipt + the source manifest can re-derive
//! the entire chain and confirm no link drifted. The CI build that
//! emits this and the operator who installs from it agree on bytes,
//! or one side is lying.
//!
//! Wire shape (schemaVersion = 1):
//!
//! ```json
//! {
//!   "schemaVersion": 1,
//!   "estante": { "version": "0.1.0" },
//!   "manifest": { "path": "shellpkg.lisp", "blake3": "..." },
//!   "lockfile": { "path": "shellpkg.lock.lisp", "blake3": "..." },
//!   "entries": [
//!     { "name": "alpha", "blake3": "...", "placement": "cache", "materializedExists": true }
//!   ]
//! }
//! ```
//!
//! Determinism: entries are emitted in lockfile order; JSON
//! serialization uses `serde_json::to_string_pretty` with a 2-space
//! indent (stable); no timestamps, no PIDs, no environment leakage.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::lockfile_io;

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Receipt {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    pub estante: EstanteInfo,
    pub manifest: FileDigest,
    pub lockfile: FileDigest,
    pub entries: Vec<EntryDigest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EstanteInfo {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileDigest {
    pub path: String,
    pub blake3: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntryDigest {
    pub name: String,
    pub blake3: String,
    pub placement: String,
    #[serde(rename = "materializedExists")]
    pub materialized_exists: bool,
}

/// Compute a [`Receipt`] from a manifest + lockfile on disk.
///
/// Returns an error if either file is unreadable. Each entry's
/// BLAKE3 is taken verbatim from the lockfile (the lockfile is the
/// source of truth — `estante verify` is the separate check that
/// proves the lockfile's recorded BLAKE3 still matches the bytes on
/// disk).
pub fn build_receipt(manifest_path: &Path, lockfile_path: &Path) -> anyhow::Result<Receipt> {
    let manifest_bytes = std::fs::read(manifest_path)
        .with_context(|| ["reading manifest ", &manifest_path.display().to_string()].concat())?;
    let manifest_blake = blake3::hash(&manifest_bytes).to_hex().to_string();

    let lockfile_bytes = std::fs::read(lockfile_path)
        .with_context(|| ["reading lockfile ", &lockfile_path.display().to_string()].concat())?;
    let lockfile_blake = blake3::hash(&lockfile_bytes).to_hex().to_string();

    let lock = lockfile_io::read(lockfile_path)
        .with_context(|| ["parsing lockfile ", &lockfile_path.display().to_string()].concat())?;

    let entries = lock
        .entries
        .iter()
        .map(|e| EntryDigest {
            name: e.name.clone(),
            blake3: e.blake3.clone(),
            placement: if e.placement.is_empty() {
                "cache".to_owned()
            } else {
                e.placement.clone()
            },
            materialized_exists: PathBuf::from(&e.materialized_path).exists(),
        })
        .collect();

    Ok(Receipt {
        schema_version: SCHEMA_VERSION,
        estante: EstanteInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        manifest: FileDigest {
            path: manifest_path.display().to_string(),
            blake3: manifest_blake,
        },
        lockfile: FileDigest {
            path: lockfile_path.display().to_string(),
            blake3: lockfile_blake,
        },
        entries,
    })
}

/// Serialize a receipt to its canonical wire bytes. The JSON is
/// pretty-printed with `serde_json::to_string_pretty` (2-space
/// indent, fields in struct-definition order — both are stable
/// across serde_json versions for derive-generated impls).
#[must_use]
pub fn canonical_json(receipt: &Receipt) -> String {
    let mut s = serde_json::to_string_pretty(receipt).expect("Receipt is serializable");
    s.push('\n');
    s
}

/// BLAKE3 over the canonical JSON bytes — the one-line attestation
/// digest a downstream verifier can compare against.
#[must_use]
pub fn receipt_blake3(receipt: &Receipt) -> String {
    blake3::hash(canonical_json(receipt).as_bytes())
        .to_hex()
        .to_string()
}

/// Difference between two receipts. `None` means they're identical.
#[derive(Debug, Clone)]
pub struct ReceiptMismatch {
    pub field: String,
    pub claimed: String,
    pub actual: String,
}

/// Compare a claimed receipt against one re-derived from the current
/// manifest + lockfile + materialized state. Returns the full list
/// of mismatches in deterministic order (manifest → lockfile →
/// per-entry).
pub fn diff_receipts(claimed: &Receipt, actual: &Receipt) -> Vec<ReceiptMismatch> {
    let mut diffs: Vec<ReceiptMismatch> = Vec::new();
    if claimed.schema_version != actual.schema_version {
        diffs.push(ReceiptMismatch {
            field: "schemaVersion".into(),
            claimed: claimed.schema_version.to_string(),
            actual: actual.schema_version.to_string(),
        });
    }
    if claimed.manifest.blake3 != actual.manifest.blake3 {
        diffs.push(ReceiptMismatch {
            field: "manifest.blake3".into(),
            claimed: claimed.manifest.blake3.clone(),
            actual: actual.manifest.blake3.clone(),
        });
    }
    if claimed.lockfile.blake3 != actual.lockfile.blake3 {
        diffs.push(ReceiptMismatch {
            field: "lockfile.blake3".into(),
            claimed: claimed.lockfile.blake3.clone(),
            actual: actual.lockfile.blake3.clone(),
        });
    }
    // Pair entries by name; entries claimed but missing in actual count too.
    let actual_by_name: std::collections::HashMap<&str, &EntryDigest> = actual
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e))
        .collect();
    for c in &claimed.entries {
        match actual_by_name.get(c.name.as_str()) {
            Some(a) => {
                if c.blake3 != a.blake3 {
                    diffs.push(ReceiptMismatch {
                        field: ["entries[", &c.name, "].blake3"].concat(),
                        claimed: c.blake3.clone(),
                        actual: a.blake3.clone(),
                    });
                }
                if c.placement != a.placement {
                    diffs.push(ReceiptMismatch {
                        field: ["entries[", &c.name, "].placement"].concat(),
                        claimed: c.placement.clone(),
                        actual: a.placement.clone(),
                    });
                }
            }
            None => diffs.push(ReceiptMismatch {
                field: ["entries[", &c.name, "]"].concat(),
                claimed: "present".into(),
                actual: "missing".into(),
            }),
        }
    }
    // Entries new in actual but not claimed.
    let claimed_names: std::collections::HashSet<&str> =
        claimed.entries.iter().map(|e| e.name.as_str()).collect();
    for a in &actual.entries {
        if !claimed_names.contains(a.name.as_str()) {
            diffs.push(ReceiptMismatch {
                field: ["entries[", &a.name, "]"].concat(),
                claimed: "missing".into(),
                actual: "present".into(),
            });
        }
    }
    diffs
}

pub async fn run(
    manifest_path: &Path,
    lockfile_path: &Path,
    out: Option<&Path>,
    verify: Option<&Path>,
    json_out: bool,
) -> anyhow::Result<()> {
    if let Some(receipt_path) = verify {
        let claimed_bytes = std::fs::read(receipt_path)
            .with_context(|| ["reading receipt ", &receipt_path.display().to_string()].concat())?;
        let claimed: Receipt = serde_json::from_slice(&claimed_bytes)
            .with_context(|| ["parsing receipt ", &receipt_path.display().to_string()].concat())?;
        let actual = build_receipt(manifest_path, lockfile_path)?;
        let diffs = diff_receipts(&claimed, &actual);
        if json_out {
            let payload = serde_json::json!({
                "matched": diffs.is_empty(),
                "manifestBlake3": actual.manifest.blake3,
                "lockfileBlake3": actual.lockfile.blake3,
                "entryCount": actual.entries.len(),
                "diffs": diffs.iter().map(|d| serde_json::json!({
                    "field": d.field,
                    "claimed": d.claimed,
                    "actual": d.actual,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
            if !diffs.is_empty() {
                anyhow::bail!("receipt verification failed: {} mismatch(es)", diffs.len());
            }
            return Ok(());
        }
        if diffs.is_empty() {
            println!(
                "\x1b[32m✓\x1b[0m receipt matches; manifest.blake3 = {}, lockfile.blake3 = {}, entries = {}",
                actual.manifest.blake3,
                actual.lockfile.blake3,
                actual.entries.len(),
            );
            return Ok(());
        }
        for d in &diffs {
            eprintln!(
                "\x1b[31m✗\x1b[0m {}\n     claimed: {}\n     actual:  {}",
                d.field, d.claimed, d.actual,
            );
        }
        anyhow::bail!("receipt verification failed: {} mismatch(es)", diffs.len());
    }
    let receipt = build_receipt(manifest_path, lockfile_path)?;
    let json = canonical_json(&receipt);
    match out {
        Some(path) => {
            std::fs::write(path, json.as_bytes())
                .with_context(|| ["writing ", &path.display().to_string()].concat())?;
            eprintln!(
                "wrote receipt to {} ({}b)\nblake3 = {}",
                path.display(),
                json.len(),
                receipt_blake3(&receipt),
            );
        }
        None => {
            print!("{json}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use estante_types::{LockedPkgSpec, Lockfile};

    fn write_fixture(root: &Path) -> (PathBuf, PathBuf) {
        std::fs::create_dir_all(root).unwrap();
        let manifest = root.join("shellpkg.lisp");
        std::fs::write(&manifest, b"(defshellpkg :name \"a\" :version \"1\" :source \"local:./pkg\")\n").unwrap();

        let pkg_dir = root.join("pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("rc.lisp"), "(defalias :name \"a\" :value \"b\")").unwrap();

        let mut lock = Lockfile::default();
        lock.upsert(LockedPkgSpec {
            name: "a".into(),
            source: "local:./pkg".into(),
            rev: "abc".into(),
            nar_hash: String::new(),
            blake3: "deadbeef".repeat(8),
            materialized_path: pkg_dir.display().to_string(),
            placement: "cache".into(),
        });
        let lockfile = root.join("shellpkg.lock.lisp");
        std::fs::write(&lockfile, lock.to_string()).unwrap();

        (manifest, lockfile)
    }

    #[test]
    fn receipt_includes_manifest_and_lockfile_digests() {
        let tmp = std::env::temp_dir().join(["estante-attest-shape-", &std::process::id().to_string()].concat());
        let _ = std::fs::remove_dir_all(&tmp);
        let (m, l) = write_fixture(&tmp);
        let receipt = build_receipt(&m, &l).unwrap();
        assert_eq!(receipt.schema_version, SCHEMA_VERSION);
        assert_eq!(receipt.estante.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(receipt.manifest.path, m.display().to_string());
        assert!(!receipt.manifest.blake3.is_empty());
        assert!(!receipt.lockfile.blake3.is_empty());
        assert_ne!(
            receipt.manifest.blake3, receipt.lockfile.blake3,
            "different files must produce different digests"
        );
        assert_eq!(receipt.entries.len(), 1);
        assert_eq!(receipt.entries[0].name, "a");
        assert_eq!(receipt.entries[0].placement, "cache");
        assert!(receipt.entries[0].materialized_exists);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn receipt_is_deterministic_across_calls() {
        let tmp = std::env::temp_dir().join(["estante-attest-det-", &std::process::id().to_string()].concat());
        let _ = std::fs::remove_dir_all(&tmp);
        let (m, l) = write_fixture(&tmp);
        let r1 = build_receipt(&m, &l).unwrap();
        let r2 = build_receipt(&m, &l).unwrap();
        assert_eq!(r1, r2, "build_receipt must be deterministic");
        assert_eq!(canonical_json(&r1), canonical_json(&r2));
        assert_eq!(receipt_blake3(&r1), receipt_blake3(&r2));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn receipt_blake3_changes_when_manifest_byte_changes() {
        let tmp = std::env::temp_dir().join(["estante-attest-drift-", &std::process::id().to_string()].concat());
        let _ = std::fs::remove_dir_all(&tmp);
        let (m, l) = write_fixture(&tmp);
        let r1 = build_receipt(&m, &l).unwrap();
        // Append a byte to the manifest.
        let mut body = std::fs::read(&m).unwrap();
        body.push(b' ');
        std::fs::write(&m, body).unwrap();
        let r2 = build_receipt(&m, &l).unwrap();
        assert_ne!(r1.manifest.blake3, r2.manifest.blake3);
        assert_ne!(receipt_blake3(&r1), receipt_blake3(&r2));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn canonical_json_round_trips_through_serde() {
        let tmp = std::env::temp_dir().join(["estante-attest-rt-", &std::process::id().to_string()].concat());
        let _ = std::fs::remove_dir_all(&tmp);
        let (m, l) = write_fixture(&tmp);
        let r1 = build_receipt(&m, &l).unwrap();
        let json = canonical_json(&r1);
        let r2: Receipt = serde_json::from_str(&json).unwrap();
        assert_eq!(r1, r2);
        std::fs::remove_dir_all(&tmp).ok();
    }

    fn fixed_receipt() -> Receipt {
        Receipt {
            schema_version: 1,
            estante: EstanteInfo {
                version: "0.1.0".into(),
            },
            manifest: FileDigest {
                path: "shellpkg.lisp".into(),
                blake3: "m".repeat(64),
            },
            lockfile: FileDigest {
                path: "shellpkg.lock.lisp".into(),
                blake3: "l".repeat(64),
            },
            entries: vec![EntryDigest {
                name: "alpha".into(),
                blake3: "a".repeat(64),
                placement: "cache".into(),
                materialized_exists: true,
            }],
        }
    }

    #[test]
    fn diff_receipts_empty_when_identical() {
        let r = fixed_receipt();
        assert!(diff_receipts(&r, &r).is_empty());
    }

    #[test]
    fn diff_receipts_reports_manifest_blake3_drift() {
        let claimed = fixed_receipt();
        let mut actual = claimed.clone();
        actual.manifest.blake3 = "x".repeat(64);
        let d = diff_receipts(&claimed, &actual);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].field, "manifest.blake3");
    }

    #[test]
    fn diff_receipts_reports_lockfile_blake3_drift() {
        let claimed = fixed_receipt();
        let mut actual = claimed.clone();
        actual.lockfile.blake3 = "x".repeat(64);
        let d = diff_receipts(&claimed, &actual);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].field, "lockfile.blake3");
    }

    #[test]
    fn diff_receipts_reports_entry_blake3_drift() {
        let claimed = fixed_receipt();
        let mut actual = claimed.clone();
        actual.entries[0].blake3 = "x".repeat(64);
        let d = diff_receipts(&claimed, &actual);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].field, "entries[alpha].blake3");
    }

    #[test]
    fn diff_receipts_reports_missing_claimed_entry() {
        let claimed = fixed_receipt();
        let mut actual = claimed.clone();
        actual.entries.clear();
        let d = diff_receipts(&claimed, &actual);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].field, "entries[alpha]");
        assert_eq!(d[0].actual, "missing");
    }

    #[test]
    fn diff_receipts_reports_extra_actual_entry() {
        let claimed = fixed_receipt();
        let mut actual = claimed.clone();
        actual.entries.push(EntryDigest {
            name: "beta".into(),
            blake3: "b".repeat(64),
            placement: "cache".into(),
            materialized_exists: true,
        });
        let d = diff_receipts(&claimed, &actual);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].field, "entries[beta]");
        assert_eq!(d[0].claimed, "missing");
    }

    #[test]
    fn diff_receipts_reports_placement_drift() {
        let claimed = fixed_receipt();
        let mut actual = claimed.clone();
        actual.entries[0].placement = "nix".into();
        let d = diff_receipts(&claimed, &actual);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].field, "entries[alpha].placement");
    }

    #[test]
    fn empty_placement_normalizes_to_cache() {
        let tmp = std::env::temp_dir().join(["estante-attest-empty-", &std::process::id().to_string()].concat());
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let manifest = tmp.join("shellpkg.lisp");
        std::fs::write(&manifest, b"").unwrap();
        let pkg_dir = tmp.join("pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        let mut lock = Lockfile::default();
        lock.upsert(LockedPkgSpec {
            name: "n".into(),
            source: "local:./pkg".into(),
            rev: "abc".into(),
            nar_hash: String::new(),
            blake3: "x".into(),
            materialized_path: pkg_dir.display().to_string(),
            placement: String::new(),
        });
        let lockfile = tmp.join("shellpkg.lock.lisp");
        std::fs::write(&lockfile, lock.to_string()).unwrap();

        let receipt = build_receipt(&manifest, &lockfile).unwrap();
        assert_eq!(receipt.entries[0].placement, "cache");
        std::fs::remove_dir_all(&tmp).ok();
    }
}

// Property-based tests for the receipt-diff primitive. Each property
// asserts an invariant of `diff_receipts` that example-tests don't
// cover exhaustively: identical receipts always diff-empty; any
// mutation must surface in diffs; diff is symmetric in coverage.
#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    fn entry_strategy() -> impl Strategy<Value = EntryDigest> {
        (
            "[a-z][a-z0-9-]{0,15}",
            "[a-f0-9]{32,64}",
            prop_oneof![
                Just("cache".to_owned()),
                Just("nix".to_owned()),
                Just("both".to_owned()),
            ],
            any::<bool>(),
        )
            .prop_map(|(name, blake3, placement, materialized_exists)| EntryDigest {
                name,
                blake3,
                placement,
                materialized_exists,
            })
    }

    fn receipt_strategy() -> impl Strategy<Value = Receipt> {
        (
            "[a-f0-9]{32,64}",
            "[a-f0-9]{32,64}",
            proptest::collection::vec(entry_strategy(), 0..5),
        )
            .prop_map(|(m_blake, l_blake, entries)| {
                // Dedupe by name so the diff_receipts pair-by-name
                // pass behaves deterministically.
                let mut by_name = std::collections::BTreeMap::new();
                for e in entries {
                    by_name.insert(e.name.clone(), e);
                }
                Receipt {
                    schema_version: 1,
                    estante: EstanteInfo {
                        version: "0.1.0".into(),
                    },
                    manifest: FileDigest {
                        path: "shellpkg.lisp".into(),
                        blake3: m_blake,
                    },
                    lockfile: FileDigest {
                        path: "shellpkg.lock.lisp".into(),
                        blake3: l_blake,
                    },
                    entries: by_name.into_values().collect(),
                }
            })
    }

    proptest! {
        /// A receipt always diffs-empty against itself, regardless of
        /// its shape.
        #[test]
        fn self_diff_is_always_empty(r in receipt_strategy()) {
            prop_assert!(diff_receipts(&r, &r).is_empty());
        }

        /// The canonical JSON survives serde round-trip for any receipt
        /// in the strategy space.
        #[test]
        fn canonical_json_serde_round_trip(r in receipt_strategy()) {
            let json = canonical_json(&r);
            let parsed: Receipt = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(parsed, r);
        }

        /// BLAKE3 of the canonical JSON is deterministic and stable
        /// across repeated calls on the same Receipt.
        #[test]
        fn receipt_blake3_is_deterministic(r in receipt_strategy()) {
            let h1 = receipt_blake3(&r);
            let h2 = receipt_blake3(&r);
            prop_assert_eq!(h1, h2);
        }

        /// Mutating manifest.blake3 must always surface as the
        /// `manifest.blake3` field in diff_receipts.
        #[test]
        fn manifest_blake3_mutation_is_always_detected(
            r in receipt_strategy(),
            noise in "[a-f0-9]{32,64}",
        ) {
            prop_assume!(r.manifest.blake3 != noise);
            let mut mutated = r.clone();
            mutated.manifest.blake3 = noise;
            let diffs = diff_receipts(&r, &mutated);
            prop_assert!(diffs.iter().any(|d| d.field == "manifest.blake3"));
        }

        /// Mutating lockfile.blake3 always surfaces.
        #[test]
        fn lockfile_blake3_mutation_is_always_detected(
            r in receipt_strategy(),
            noise in "[a-f0-9]{32,64}",
        ) {
            prop_assume!(r.lockfile.blake3 != noise);
            let mut mutated = r.clone();
            mutated.lockfile.blake3 = noise;
            let diffs = diff_receipts(&r, &mutated);
            prop_assert!(diffs.iter().any(|d| d.field == "lockfile.blake3"));
        }

        /// Removing every entry from the actual side surfaces every
        /// claimed entry as missing, in deterministic order.
        #[test]
        fn dropping_all_entries_reports_each_as_missing(r in receipt_strategy()) {
            prop_assume!(!r.entries.is_empty());
            let mut empty = r.clone();
            empty.entries.clear();
            let diffs = diff_receipts(&r, &empty);
            prop_assert_eq!(diffs.len(), r.entries.len());
            for d in &diffs {
                prop_assert_eq!(&d.actual, "missing");
            }
        }

        /// diff_receipts is symmetric in *coverage* — every field a
        /// b-then-a diff reports must also be reported by a-then-b
        /// (claimed/actual may swap). No mutation can hide from one
        /// direction but appear in the other.
        #[test]
        fn diff_is_symmetric_in_field_coverage(
            a in receipt_strategy(),
            b in receipt_strategy(),
        ) {
            let fwd: std::collections::BTreeSet<String> =
                diff_receipts(&a, &b).into_iter().map(|d| d.field).collect();
            let rev: std::collections::BTreeSet<String> =
                diff_receipts(&b, &a).into_iter().map(|d| d.field).collect();
            prop_assert_eq!(fwd, rev);
        }
    }
}
