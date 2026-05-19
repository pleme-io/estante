//! `estante doctor` — full health audit. Composes verify + validate
//! + cache-size + placement consistency checks.
//!
//! Outputs a per-check pass/fail table + a summary line. Exit non-
//! zero on any failure.

use std::path::Path;

use estante_types::{Placement, Source};

use crate::config::Config;
use crate::lockfile_io;
use crate::manifest_io;

use super::attest;
use super::verify;

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

pub async fn run(manifest_path: &Path, lockfile_path: &Path, cfg: &Config) -> anyhow::Result<()> {
    let mut results: Vec<CheckResult> = Vec::new();

    // 1. Manifest parses.
    let manifest = match manifest_io::read(manifest_path) {
        Ok(m) => {
            results.push(CheckResult {
                name: "manifest:parse",
                passed: true,
                detail: format!("{} package(s)", m.packages.len()),
            });
            Some(m)
        }
        Err(e) => {
            results.push(CheckResult {
                name: "manifest:parse",
                passed: false,
                detail: e.to_string(),
            });
            None
        }
    };

    // 2. Lockfile parses.
    let lock = match lockfile_io::read(lockfile_path) {
        Ok(l) => {
            results.push(CheckResult {
                name: "lockfile:parse",
                passed: true,
                detail: format!("{} entr(y/ies)", l.entries.len()),
            });
            Some(l)
        }
        Err(e) => {
            results.push(CheckResult {
                name: "lockfile:parse",
                passed: false,
                detail: e.to_string(),
            });
            None
        }
    };

    // 3. Manifest sources all parse to valid Source variants.
    if let Some(m) = &manifest {
        let mut malformed = Vec::new();
        for pkg in &m.packages {
            if Source::parse(&pkg.source).is_err() {
                malformed.push(pkg.name.clone());
            }
        }
        results.push(CheckResult {
            name: "manifest:sources-parse",
            passed: malformed.is_empty(),
            detail: if malformed.is_empty() {
                "all clean".into()
            } else {
                format!("malformed: {}", malformed.join(", "))
            },
        });
    }

    // 4. Lockfile placements are all recognized.
    if let Some(l) = &lock {
        let mut unknown = Vec::new();
        for e in &l.entries {
            // Placement::from_str maps unknown to Cache, but we want
            // to detect genuinely-unrecognized strings that aren't
            // "cache" / "nix" / "both" / "".
            let raw = e.placement.trim().to_ascii_lowercase();
            if !matches!(raw.as_str(), "" | "cache" | "nix" | "both") {
                unknown.push(format!("{}={}", e.name, e.placement));
            }
        }
        results.push(CheckResult {
            name: "lockfile:placement-known",
            passed: unknown.is_empty(),
            detail: if unknown.is_empty() {
                "all known".into()
            } else {
                format!("unknown: {}", unknown.join(", "))
            },
        });
    }

    // 5. Every lockfile entry's materialized path exists.
    if let Some(l) = &lock {
        let mut missing = Vec::new();
        for e in &l.entries {
            if !Path::new(&e.materialized_path).exists() {
                missing.push(e.name.clone());
            }
        }
        results.push(CheckResult {
            name: "lockfile:materialized-paths-exist",
            passed: missing.is_empty(),
            detail: if missing.is_empty() {
                format!("{}/{} present", l.entries.len(), l.entries.len())
            } else {
                format!("missing: {}", missing.join(", "))
            },
        });
    }

    // 6. BLAKE3 verification.
    if lock.is_some() {
        let report = verify::verify_at(lockfile_path, verify::Opts::default())?;
        results.push(CheckResult {
            name: "lockfile:blake3-verify",
            passed: report.is_ok(),
            detail: format!(
                "{} verified, {} drifted, {} missing, {} skipped",
                report.verified.len(),
                report.drifted.len(),
                report.missing.len(),
                report.skipped.len(),
            ),
        });
    }

    // 7. Nix-placed entries actually point at /nix/store paths.
    if let Some(l) = &lock {
        let mut inconsistent = Vec::new();
        for e in &l.entries {
            if Placement::from_str(&e.placement) == Placement::Nix
                && !e.materialized_path.starts_with("/nix/store/")
            {
                inconsistent.push(format!("{}={}", e.name, e.materialized_path));
            }
        }
        results.push(CheckResult {
            name: "placement:nix-paths-in-store",
            passed: inconsistent.is_empty(),
            detail: if inconsistent.is_empty() {
                "all consistent".into()
            } else {
                format!("inconsistent: {}", inconsistent.join(", "))
            },
        });
    }

    // 8. Cache directory exists.
    results.push(CheckResult {
        name: "cache:exists",
        passed: cfg.cache_dir.exists(),
        detail: cfg.cache_dir.display().to_string(),
    });

    // 9. Receipt sidecar matches. The receipt is a published
    // attestation receipt that some external party (CI, signer,
    // operator) emitted. If a `shellpkg.receipt.json` sits next to
    // the manifest, doctor confirms the current state still agrees
    // with that receipt — otherwise published attestations have
    // gone stale silently.
    let receipt_path = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("shellpkg.receipt.json");
    if receipt_path.exists() {
        let check_result = match std::fs::read(&receipt_path)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                serde_json::from_slice::<attest::Receipt>(&bytes).map_err(anyhow::Error::from)
            })
            .and_then(|claimed| {
                let actual = attest::build_receipt(manifest_path, lockfile_path)?;
                Ok(attest::diff_receipts(&claimed, &actual))
            }) {
            Ok(diffs) if diffs.is_empty() => CheckResult {
                name: "receipt:matches",
                passed: true,
                detail: format!("{} matches", receipt_path.display()),
            },
            Ok(diffs) => CheckResult {
                name: "receipt:matches",
                passed: false,
                detail: format!(
                    "{} mismatch(es): {}",
                    diffs.len(),
                    diffs
                        .iter()
                        .map(|d| d.field.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            },
            Err(e) => CheckResult {
                name: "receipt:matches",
                passed: false,
                detail: e.to_string(),
            },
        };
        results.push(check_result);
    }

    // ─── Render + exit. ──────────────────────────────────────────────

    let mut failed = 0_usize;
    for r in &results {
        let mark = if r.passed { "\x1b[32m✓\x1b[0m" } else {
            failed += 1;
            "\x1b[31m✗\x1b[0m"
        };
        println!("{mark} {:32}  {}", r.name, r.detail);
    }
    println!();
    if failed == 0 {
        println!("\x1b[32mAll {} checks passed.\x1b[0m", results.len());
        Ok(())
    } else {
        anyhow::bail!("{failed} of {} checks failed", results.len())
    }
}
