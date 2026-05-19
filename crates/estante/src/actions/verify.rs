//! `estante verify` — re-hash every materialized package and diff
//! against the lockfile's recorded BLAKE3.
//!
//! This is estante's **verifiability primitive**. The lockfile's
//! `:blake3` column is the tameshi-chain attestation receipt:
//! anyone with the lockfile + the materialized tree can re-derive
//! the digest and prove the bytes haven't drifted. `estante verify`
//! is the one-command form of that check.
//!
//! Exit codes (matches `cargo verify`-style tooling):
//!   0   — every entry verified.
//!   1   — at least one drift; details printed to stderr.
//!   2   — lockfile or io error before we could verify anything.
//!
//! Flags:
//!   --strict   — empty BLAKE3 entries (cache-placement, pre-hash)
//!                count as failures. Default: skipped with a note.
//!   --report   — emit a JSON report on stdout (suitable for CI).

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::json;

use crate::hash;
use crate::lockfile_io;

#[derive(Debug, Clone, Copy, Default)]
pub struct Opts {
    pub strict: bool,
    pub json: bool,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub verified: Vec<String>,
    pub drifted: Vec<DriftedEntry>,
    pub skipped: Vec<SkippedEntry>,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DriftedEntry {
    pub name: String,
    pub expected_blake3: String,
    pub actual_blake3: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct SkippedEntry {
    pub name: String,
    pub reason: String,
}

impl Report {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.drifted.is_empty() && self.missing.is_empty()
    }

    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "verified": self.verified,
            "drifted": self.drifted.iter().map(|d| json!({
                "name": d.name,
                "expected": d.expected_blake3,
                "actual": d.actual_blake3,
                "path": d.path,
            })).collect::<Vec<_>>(),
            "missing": self.missing,
            "skipped": self.skipped.iter().map(|s| json!({
                "name": s.name,
                "reason": s.reason,
            })).collect::<Vec<_>>(),
        })
    }
}

pub fn verify_at(lockfile_path: &Path, opts: Opts) -> anyhow::Result<Report> {
    let lock = lockfile_io::read(lockfile_path)
        .with_context(|| format!("reading lockfile {}", lockfile_path.display()))?;
    if lock.entries.is_empty() {
        anyhow::bail!("lockfile {} is empty", lockfile_path.display());
    }

    let mut report = Report {
        verified: Vec::new(),
        drifted: Vec::new(),
        skipped: Vec::new(),
        missing: Vec::new(),
    };

    for entry in &lock.entries {
        let path = PathBuf::from(&entry.materialized_path);
        if !path.exists() {
            report.missing.push(entry.name.clone());
            continue;
        }
        if entry.blake3.is_empty() {
            if opts.strict {
                report.drifted.push(DriftedEntry {
                    name: entry.name.clone(),
                    expected_blake3: "<empty>".to_owned(),
                    actual_blake3: hash::blake3_tree(&path).unwrap_or_default(),
                    path: entry.materialized_path.clone(),
                });
            } else {
                report.skipped.push(SkippedEntry {
                    name: entry.name.clone(),
                    reason: "lockfile entry has no BLAKE3 (pre-hash). Re-run `estante install`.".to_owned(),
                });
            }
            continue;
        }
        let actual = hash::blake3_tree(&path)
            .with_context(|| format!("hashing {}", entry.materialized_path))?;
        if actual == entry.blake3 {
            report.verified.push(entry.name.clone());
        } else {
            report.drifted.push(DriftedEntry {
                name: entry.name.clone(),
                expected_blake3: entry.blake3.clone(),
                actual_blake3: actual,
                path: entry.materialized_path.clone(),
            });
        }
    }

    Ok(report)
}

pub async fn run(lockfile_path: &Path, opts: Opts) -> anyhow::Result<()> {
    let report = verify_at(lockfile_path, opts)?;

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&report.to_json())?);
    } else {
        for name in &report.verified {
            println!("\x1b[32m✓\x1b[0m {name}");
        }
        for entry in &report.drifted {
            eprintln!(
                "\x1b[31m✗\x1b[0m {} — BLAKE3 drift\n     expected: {}\n     actual:   {}\n     path:     {}",
                entry.name, entry.expected_blake3, entry.actual_blake3, entry.path,
            );
        }
        for name in &report.missing {
            eprintln!("\x1b[33m?\x1b[0m {name} — materialized path missing (run `estante install`)");
        }
        for entry in &report.skipped {
            eprintln!("\x1b[2m·\x1b[0m {} — skipped ({})", entry.name, entry.reason);
        }
        println!(
            "\n{} verified, {} drifted, {} missing, {} skipped",
            report.verified.len(),
            report.drifted.len(),
            report.missing.len(),
            report.skipped.len(),
        );
    }

    if !report.is_ok() {
        anyhow::bail!(
            "verify failed: {} drifted, {} missing",
            report.drifted.len(),
            report.missing.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use estante_types::{Lockfile, LockedPkgSpec};

    #[test]
    fn report_is_ok_when_only_skipped() {
        let r = Report {
            verified: vec!["a".into()],
            drifted: vec![],
            skipped: vec![SkippedEntry {
                name: "b".into(),
                reason: "empty".into(),
            }],
            missing: vec![],
        };
        assert!(r.is_ok());
    }

    #[test]
    fn report_is_not_ok_when_drifted() {
        let r = Report {
            verified: vec![],
            drifted: vec![DriftedEntry {
                name: "a".into(),
                expected_blake3: "x".into(),
                actual_blake3: "y".into(),
                path: "/p".into(),
            }],
            skipped: vec![],
            missing: vec![],
        };
        assert!(!r.is_ok());
    }

    #[test]
    fn report_is_not_ok_when_missing() {
        let r = Report {
            verified: vec![],
            drifted: vec![],
            skipped: vec![],
            missing: vec!["gone".into()],
        };
        assert!(!r.is_ok());
    }

    #[test]
    fn verify_at_missing_path_lands_in_missing() {
        let tmp = std::env::temp_dir().join(format!("estante-verify-miss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let lock_path = tmp.join("shellpkg.lock.lisp");
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "ghost".into(),
            source: "github:o/r".into(),
            rev: "abc".into(),
            nar_hash: String::new(),
            blake3: "expected-blake3".into(),
            materialized_path: "/this/does/not/exist".into(),
            placement: "cache".into(),
        });
        std::fs::write(&lock_path, l.to_string()).unwrap();
        let report = verify_at(&lock_path, Opts::default()).unwrap();
        assert_eq!(report.missing, vec!["ghost".to_string()]);
        assert!(!report.is_ok());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn verify_at_matching_blake3_lands_in_verified() {
        let tmp =
            std::env::temp_dir().join(format!("estante-verify-match-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let pkg_dir = tmp.join("pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("rc.lisp"), "(defalias :name \"x\" :value \"y\")").unwrap();
        // Compute the real BLAKE3 first.
        let expected = hash::blake3_tree(&pkg_dir).unwrap();

        let lock_path = tmp.join("shellpkg.lock.lisp");
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "good".into(),
            source: "local:./pkg".into(),
            rev: "abc".into(),
            nar_hash: String::new(),
            blake3: expected.clone(),
            materialized_path: pkg_dir.to_string_lossy().into_owned(),
            placement: "cache".into(),
        });
        std::fs::write(&lock_path, l.to_string()).unwrap();
        let report = verify_at(&lock_path, Opts::default()).unwrap();
        assert_eq!(report.verified, vec!["good".to_string()]);
        assert!(report.is_ok());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn verify_at_drifted_blake3_lands_in_drifted() {
        let tmp =
            std::env::temp_dir().join(format!("estante-verify-drift-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let pkg_dir = tmp.join("pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("rc.lisp"), "actual content").unwrap();
        let lock_path = tmp.join("shellpkg.lock.lisp");
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "drifted".into(),
            source: "local:./pkg".into(),
            rev: "abc".into(),
            nar_hash: String::new(),
            blake3: "wrong-recorded-blake3-value".into(),
            materialized_path: pkg_dir.to_string_lossy().into_owned(),
            placement: "cache".into(),
        });
        std::fs::write(&lock_path, l.to_string()).unwrap();
        let report = verify_at(&lock_path, Opts::default()).unwrap();
        assert_eq!(report.drifted.len(), 1);
        assert_eq!(report.drifted[0].name, "drifted");
        assert_eq!(report.drifted[0].expected_blake3, "wrong-recorded-blake3-value");
        assert!(!report.is_ok());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn verify_at_skips_empty_blake3_in_non_strict_mode() {
        let tmp =
            std::env::temp_dir().join(format!("estante-verify-skip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let pkg_dir = tmp.join("pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("rc.lisp"), "x").unwrap();
        let lock_path = tmp.join("shellpkg.lock.lisp");
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "no-hash".into(),
            source: "local:./pkg".into(),
            rev: "abc".into(),
            nar_hash: String::new(),
            blake3: String::new(),
            materialized_path: pkg_dir.to_string_lossy().into_owned(),
            placement: "cache".into(),
        });
        std::fs::write(&lock_path, l.to_string()).unwrap();
        let report = verify_at(&lock_path, Opts::default()).unwrap();
        assert_eq!(report.skipped.len(), 1);
        assert!(report.is_ok(), "empty BLAKE3 is not a failure in non-strict mode");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn verify_at_strict_treats_empty_blake3_as_drift() {
        let tmp =
            std::env::temp_dir().join(format!("estante-verify-strict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let pkg_dir = tmp.join("pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("rc.lisp"), "x").unwrap();
        let lock_path = tmp.join("shellpkg.lock.lisp");
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "no-hash".into(),
            source: "local:./pkg".into(),
            rev: "abc".into(),
            nar_hash: String::new(),
            blake3: String::new(),
            materialized_path: pkg_dir.to_string_lossy().into_owned(),
            placement: "cache".into(),
        });
        std::fs::write(&lock_path, l.to_string()).unwrap();
        let report = verify_at(&lock_path, Opts { strict: true, json: false }).unwrap();
        assert_eq!(report.drifted.len(), 1);
        assert!(!report.is_ok());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn report_to_json_includes_all_categories() {
        let r = Report {
            verified: vec!["v1".into()],
            drifted: vec![DriftedEntry {
                name: "d1".into(),
                expected_blake3: "e".into(),
                actual_blake3: "a".into(),
                path: "/p".into(),
            }],
            skipped: vec![SkippedEntry {
                name: "s1".into(),
                reason: "r".into(),
            }],
            missing: vec!["m1".into()],
        };
        let j = r.to_json();
        assert_eq!(j["verified"].as_array().unwrap().len(), 1);
        assert_eq!(j["drifted"].as_array().unwrap().len(), 1);
        assert_eq!(j["skipped"].as_array().unwrap().len(), 1);
        assert_eq!(j["missing"].as_array().unwrap().len(), 1);
    }
}
