//! Typed receipt + diff primitives — the tameshi-chain anchor.
//!
//! A `Receipt` is a deterministic JSON artifact (emitted by `estante
//! attest`) containing the BLAKE3 of every load-bearing artifact in
//! an estante workspace: the manifest source, the lockfile, and
//! each locked entry's content digest.
//!
//! These types live in `estante-types` (not the bin crate) so any
//! downstream Rust tool — caixa-frost renderer, frostmourne
//! migrator, CI helper, etc. — can produce and verify receipts
//! without depending on the estante CLI. The CLI's `attest` action
//! is a thin wrapper around [`canonical_json`] / [`receipt_blake3`]
//! / [`diff_receipts`] — re-exported here from the bin crate via
//! `actions::attest::{Receipt, …}` for backward compat.
//!
//! Pillar 12: solve once, in the typed primitive lib. The bin gets
//! the I/O surface (reading manifest/lockfile files); the lib owns
//! the typed shape + the canonicalization.

use serde::{Deserialize, Serialize};

/// Schema version of the wire JSON. Bumping breaks downstream
/// loaders; substrate's `receipt-loader.nix` validates this.
pub const SCHEMA_VERSION: u32 = 1;

/// The full attestation receipt for one estante workspace.
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

/// Serialize a receipt to its canonical wire bytes. Pretty-printed
/// with `serde_json::to_string_pretty` (2-space indent, fields in
/// struct-definition order — both stable across serde_json
/// versions for derive-generated impls). Trailing newline is part
/// of the contract.
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

/// Difference between a claimed and an actual receipt's field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptMismatch {
    pub field: String,
    pub claimed: String,
    pub actual: String,
}

/// Compare a claimed receipt against one re-derived from the
/// current manifest + lockfile + materialized state. Returns the
/// full list of mismatches in deterministic order (manifest →
/// lockfile → per-entry).
#[must_use]
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
    // Pair entries by name; entries claimed but missing in actual
    // count too.
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn canonical_json_round_trip() {
        let r = fixed_receipt();
        let json = canonical_json(&r);
        let parsed: Receipt = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn canonical_json_ends_with_newline() {
        let r = fixed_receipt();
        let json = canonical_json(&r);
        assert!(json.ends_with('\n'));
    }

    #[test]
    fn diff_receipts_empty_on_identity() {
        let r = fixed_receipt();
        assert!(diff_receipts(&r, &r).is_empty());
    }

    #[test]
    fn diff_receipts_reports_manifest_drift() {
        let claimed = fixed_receipt();
        let mut actual = claimed.clone();
        actual.manifest.blake3 = "x".repeat(64);
        let d = diff_receipts(&claimed, &actual);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].field, "manifest.blake3");
    }

    #[test]
    fn diff_receipts_reports_schema_drift() {
        let claimed = fixed_receipt();
        let mut actual = claimed.clone();
        actual.schema_version = 2;
        let d = diff_receipts(&claimed, &actual);
        assert!(d.iter().any(|m| m.field == "schemaVersion"));
    }

    #[test]
    fn receipt_blake3_deterministic() {
        let r = fixed_receipt();
        assert_eq!(receipt_blake3(&r), receipt_blake3(&r));
    }
}
