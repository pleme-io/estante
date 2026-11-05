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
//! is a thin wrapper around [`canonical_json`] / [`receipt_address`]
//! / [`diff_receipts`] — re-exported here from the bin crate via
//! `actions::attest::{Receipt, …}` for backward compat.
//!
//! Pillar 12: solve once, in the typed primitive lib. The bin gets
//! the I/O surface (reading manifest/lockfile files); the lib owns
//! the typed shape + the canonicalization.

use serde::{Deserialize, Serialize};
pub use tameshi::content_address::{ContentAddress, ContentAddressError};

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

/// The receipt's content address: BLAKE3 over sorted-key compact JSON, printed `b3:<hex>`.
///
/// It does not depend on struct field order, and a receipt with no
/// canonical form is refused rather than hashed.
pub fn receipt_address(receipt: &Receipt) -> Result<ContentAddress, ContentAddressError> {
    ContentAddress::of_json(receipt)
}

/// A digest a verifier holds for a receipt, in either address form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptDigest {
    /// `b3:<hex>`: the content address, from estante 0.2.0 on.
    Content(ContentAddress),
    /// Bare 64-hex: BLAKE3 over [`canonical_json`], emitted before 0.2.0.
    Legacy([u8; 32]),
}

impl ReceiptDigest {
    /// Parse `b3:<64 hex>` as the content address, or bare 64 lowercase hex as the legacy form.
    pub fn parse(text: &str) -> Result<Self, ContentAddressError> {
        if text.starts_with("b3:") {
            return ContentAddress::from_b3(text).map(Self::Content);
        }
        let refuse = || ContentAddressError::NotAnAddress(text.to_owned());
        if text.len() != 64 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(refuse());
        }
        blake3::Hash::from_hex(text)
            .map(|h| Self::Legacy(*h.as_bytes()))
            .map_err(|_| refuse())
    }

    /// Whether this digest names `receipt`, under the form the digest was written in.
    pub fn verifies(&self, receipt: &Receipt) -> Result<bool, ContentAddressError> {
        match self {
            Self::Content(address) => Ok(receipt_address(receipt)? == *address),
            Self::Legacy(bytes) => {
                Ok(blake3::hash(canonical_json(receipt).as_bytes()).as_bytes() == bytes)
            }
        }
    }
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

    #[derive(Serialize)]
    struct ReorderedReceipt<'a> {
        entries: &'a Vec<EntryDigest>,
        lockfile: &'a FileDigest,
        manifest: &'a FileDigest,
        estante: &'a EstanteInfo,
        #[serde(rename = "schemaVersion")]
        schema_version: u32,
    }

    const GOLDEN_ADDRESS: &str =
        "b3:edf47eb94964c239671b02b070273933098f373da55fe92b9f35ff6446423ec3";

    const LEGACY_DIGEST: &str = "28fc36a241f20077f7223aae91170b378fec6804f3b0470b9a9bbf23f49e1f67";

    #[test]
    fn receipt_address_deterministic() {
        let r = fixed_receipt();
        assert_eq!(receipt_address(&r).unwrap(), receipt_address(&r).unwrap());
    }

    #[test]
    fn receipt_address_ignores_field_order() {
        let r = fixed_receipt();
        let reordered = ReorderedReceipt {
            entries: &r.entries,
            lockfile: &r.lockfile,
            manifest: &r.manifest,
            estante: &r.estante,
            schema_version: r.schema_version,
        };
        assert_eq!(
            receipt_address(&r).unwrap(),
            ContentAddress::of_json(&reordered).unwrap()
        );
    }

    #[test]
    fn receipt_address_golden_vector() {
        assert_eq!(
            receipt_address(&fixed_receipt()).unwrap().to_b3(),
            GOLDEN_ADDRESS
        );
    }

    #[test]
    fn receipt_address_is_blake3_of_sorted_compact_json() {
        let r = fixed_receipt();
        let compact = serde_json::to_vec(&serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(
            receipt_address(&r).unwrap().as_bytes(),
            blake3::hash(&compact).as_bytes()
        );
    }

    #[test]
    fn legacy_digest_still_verifies() {
        let r = fixed_receipt();
        let digest = ReceiptDigest::parse(LEGACY_DIGEST).unwrap();
        assert!(matches!(digest, ReceiptDigest::Legacy(_)));
        assert!(digest.verifies(&r).unwrap());
    }

    #[test]
    fn content_digest_verifies() {
        let r = fixed_receipt();
        let digest = ReceiptDigest::parse(GOLDEN_ADDRESS).unwrap();
        assert!(matches!(digest, ReceiptDigest::Content(_)));
        assert!(digest.verifies(&r).unwrap());
    }

    #[test]
    fn digests_reject_a_changed_receipt() {
        let mut r = fixed_receipt();
        r.entries[0].placement = "nix".into();
        for text in [LEGACY_DIGEST, GOLDEN_ADDRESS] {
            assert!(!ReceiptDigest::parse(text).unwrap().verifies(&r).unwrap());
        }
    }

    #[test]
    fn digest_parse_refuses_malformed_text() {
        for text in [
            "",
            "b3:",
            "b3:xyz",
            &"A".repeat(64),
            &"a".repeat(63),
            "blake3:00",
        ] {
            assert!(ReceiptDigest::parse(text).is_err(), "{text:?}");
        }
    }
}
