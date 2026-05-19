//! Nix-shape lockfile emission.
//!
//! Bridges estante to the substrate Nix builders. A `Lockfile` renders
//! to a pure-data Nix attrset that `substrate/lib/build/estante/`
//! consumes via `import ./shellpkg.lock.nix`. No estante binary needed
//! at Nix evaluation time — the lockfile is the contract.
//!
//! Canonical schema (v1):
//!
//! ```nix
//! {
//!   schemaVersion = 1;
//!   packages = [
//!     {
//!       name = "you-should-use";
//!       source = "github:MichaelAquilina/zsh-you-should-use";
//!       rev = "aa489f1d…";
//!       narHash = "sha256-…";
//!       blake3 = "…";
//!       exports = [ "alias" "hook" ];
//!       entrypoint = "rc.lisp";
//!     }
//!     # …
//!   ];
//! }
//! ```
//!
//! Like the Lisp emitter, this lives under the typed-emission ban —
//! every escaping decision is local to one `Display` impl. No
//! `format!()` for value composition.

use std::fmt;

use crate::Lockfile;

/// Render a [`Lockfile`] to a Nix attrset string. Output is
/// deterministic + reparsable by Nix's `import`.
#[must_use]
pub fn lockfile_to_nix(lockfile: &Lockfile) -> String {
    NixLockfileFormatter(lockfile).to_string()
}

struct NixLockfileFormatter<'a>(&'a Lockfile);

impl fmt::Display for NixLockfileFormatter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "# shellpkg.lock.nix — machine-emitted by `estante export --format nix`."
        )?;
        writeln!(
            f,
            "# DO NOT EDIT BY HAND. Re-run `estante lock` then `estante export`."
        )?;
        writeln!(
            f,
            "# Consumed by `substrate/lib/build/estante/` via `import ./shellpkg.lock.nix`."
        )?;
        writeln!(f)?;
        writeln!(f, "{{")?;
        writeln!(f, "  schemaVersion = 1;")?;
        writeln!(f, "  packages = [")?;
        for entry in &self.0.entries {
            writeln!(f, "    {{")?;
            writeln!(f, "      name = {};", NixString(&entry.name))?;
            writeln!(f, "      source = {};", NixString(&entry.source))?;
            writeln!(f, "      rev = {};", NixString(&entry.rev))?;
            if !entry.nar_hash.is_empty() {
                writeln!(f, "      narHash = {};", NixString(&entry.nar_hash))?;
            }
            writeln!(f, "      blake3 = {};", NixString(&entry.blake3))?;
            writeln!(
                f,
                "      materializedPath = {};",
                NixString(&entry.materialized_path)
            )?;
            writeln!(f, "      entrypoint = \"rc.lisp\";")?;
            let placement = if entry.placement.is_empty() {
                "cache"
            } else {
                entry.placement.as_str()
            };
            writeln!(f, "      placement = {};", NixString(placement))?;
            writeln!(f, "    }}")?;
        }
        writeln!(f, "  ];")?;
        writeln!(f, "}}")
    }
}

/// Newtype wrapper that emits a Rust string as a Nix string literal.
/// Escapes the four characters Nix string syntax cares about: `"`,
/// `\`, `$` (interpolation start) and the dollar-brace `${` pattern.
struct NixString<'a>(pub &'a str);

impl fmt::Display for NixString<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"")?;
        let mut chars = self.0.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                '$' if chars.peek() == Some(&'{') => {
                    // Escape only the interpolation form `${` — bare $
                    // is fine in Nix strings.
                    f.write_str("\\${")?;
                    let _ = chars.next();
                }
                _ => f.write_char(c)?,
            }
        }
        f.write_str("\"")
    }
}

use std::fmt::Write as _;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LockedPkgSpec;

    #[test]
    fn empty_lockfile_renders_with_empty_packages_list() {
        let l = Lockfile::default();
        let rendered = lockfile_to_nix(&l);
        assert!(rendered.contains("schemaVersion = 1"));
        assert!(rendered.contains("packages = ["));
        // No package entries between the brackets.
        let after_packages = rendered.split("packages = [").nth(1).unwrap();
        let before_close = after_packages.split("];").next().unwrap();
        assert!(before_close.trim().is_empty());
    }

    #[test]
    fn single_pkg_renders_all_required_fields() {
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "foo".into(),
            source: "github:org/foo".into(),
            rev: "abc123".into(),
            nar_hash: "sha256-zzz".into(),
            blake3: "blake3-yyy".into(),
            materialized_path: "/nix/store/abc-foo/".into(),
            placement: "nix".into(),
        });
        let rendered = lockfile_to_nix(&l);
        assert!(rendered.contains(r#"name = "foo""#));
        assert!(rendered.contains(r#"source = "github:org/foo""#));
        assert!(rendered.contains(r#"rev = "abc123""#));
        assert!(rendered.contains(r#"narHash = "sha256-zzz""#));
        assert!(rendered.contains(r#"blake3 = "blake3-yyy""#));
        assert!(rendered.contains(r#"materializedPath = "/nix/store/abc-foo/""#));
        assert!(rendered.contains(r#"placement = "nix""#));
    }

    #[test]
    fn empty_nar_hash_field_is_elided() {
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "foo".into(),
            source: "github:org/foo".into(),
            rev: "abc".into(),
            nar_hash: String::new(),
            placement: "cache".into(),
            blake3: "blake3-yyy".into(),
            materialized_path: "/nix/store/abc-foo/".into(),
        });
        let rendered = lockfile_to_nix(&l);
        assert!(!rendered.contains("narHash"), "empty narHash must not emit");
        assert!(rendered.contains("blake3"));
    }

    #[test]
    fn nix_string_escapes_backslash_and_quote() {
        let mut buf = String::new();
        write!(buf, "{}", NixString(r#"with "quote" and \back"#)).unwrap();
        assert_eq!(buf, r#""with \"quote\" and \\back""#);
    }

    #[test]
    fn nix_string_escapes_dollar_brace_interpolation() {
        let mut buf = String::new();
        write!(buf, "{}", NixString("path/${var}/file")).unwrap();
        assert_eq!(buf, r#""path/\${var}/file""#);
    }

    #[test]
    fn nix_string_passes_bare_dollar_unchanged() {
        let mut buf = String::new();
        write!(buf, "{}", NixString("$5 a pound")).unwrap();
        assert_eq!(buf, r#""$5 a pound""#);
    }

    #[test]
    fn placement_emitted_as_cache_for_empty() {
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "foo".into(),
            source: "github:o/r".into(),
            rev: "abc".into(),
            nar_hash: "sha256-x".into(),
            blake3: "b3".into(),
            materialized_path: "/p".into(),
            placement: String::new(),
        });
        let rendered = lockfile_to_nix(&l);
        assert!(
            rendered.contains(r#"placement = "cache""#),
            "empty placement must normalize to cache: {rendered}"
        );
    }

    #[test]
    fn placement_emitted_verbatim_for_nix() {
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "foo".into(),
            source: "github:o/r".into(),
            rev: "abc".into(),
            nar_hash: "sha256-x".into(),
            blake3: "b3".into(),
            materialized_path: "/nix/store/abc-foo".into(),
            placement: "nix".into(),
        });
        let rendered = lockfile_to_nix(&l);
        assert!(rendered.contains(r#"placement = "nix""#));
    }

    #[test]
    fn placement_emitted_verbatim_for_both() {
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "foo".into(),
            source: "github:o/r".into(),
            rev: "abc".into(),
            nar_hash: "sha256-x".into(),
            blake3: "b3".into(),
            materialized_path: "/p".into(),
            placement: "both".into(),
        });
        let rendered = lockfile_to_nix(&l);
        assert!(rendered.contains(r#"placement = "both""#));
    }

    #[test]
    fn multi_pkg_lockfile_each_emits_own_placement() {
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "cached".into(),
            source: "github:o/cached".into(),
            rev: "1".into(),
            nar_hash: String::new(),
            blake3: "a".into(),
            materialized_path: "/p/cached".into(),
            placement: "cache".into(),
        });
        l.upsert(LockedPkgSpec {
            name: "nixed".into(),
            source: "github:o/nixed".into(),
            rev: "2".into(),
            nar_hash: "sha256-x".into(),
            blake3: "b".into(),
            materialized_path: "/nix/store/abc-nixed".into(),
            placement: "nix".into(),
        });
        let rendered = lockfile_to_nix(&l);
        let cache_count = rendered.matches(r#"placement = "cache""#).count();
        let nix_count = rendered.matches(r#"placement = "nix""#).count();
        assert_eq!(cache_count, 1);
        assert_eq!(nix_count, 1);
    }
}
