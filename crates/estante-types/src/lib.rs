//! Typed primitives for `estante` — the pleme-io shell-package manager.
//!
//! Re-exports frost-lisp's `defshellpkg` / `defload` / `deflockedpkg`
//! typed forms so estante is a *consumer* of the canonical primitives,
//! not a duplicate source of truth (Pillar 12: solve once). Adds the
//! resolver-facing wrappers a CLI needs:
//!
//! * [`Manifest`] — the parsed contents of one `shellpkg.lisp` file.
//! * [`Lockfile`] — the parsed contents of one `shellpkg.lock.lisp` file.
//! * [`Source`] — the source-URL ADT (Github / Git / Gist / Local),
//!   parsed from the string carried by [`PkgSpec::source`].
//! * [`EstanteError`] — the error variant for every estante operation
//!   that *doesn't* touch I/O. (I/O errors live in the bin's own
//!   error enum so this crate stays free of `std::io` for serde.)
//!
//! No I/O, no network, no randomness — that's the bin's job.
//!
//! ## Authoring discipline
//!
//! `Manifest` and `Lockfile` implement `Display` so emitting Lisp
//! source from a typed value goes through `write!()` calls inside a
//! `Display` impl. This is the *only* allowed string-emission surface
//! per `theory/TYPED-EMISSION.md` (the `format!()` ban). Hand-rolled
//! string concatenation of Lisp syntax is forbidden — extend [`render`]
//! instead.

#![forbid(unsafe_code)]

use std::fmt;
use std::fmt::Write as _;

pub mod nix_export;

pub use frost_lisp::{LoadSpec, LockedPkgSpec, PkgSpec, split_source_scheme};

// ─── Placement ────────────────────────────────────────────────────────

/// Where the bytes of a locked package physically live. Carried in
/// `LockedPkgSpec::placement` as a lowercase string.
///
/// | Variant | Path shape | Mutability | Reproducibility | Use case |
/// |---|---|---|---|---|
/// | [`Placement::Cache`] | `$XDG_CACHE_HOME/estante/store/<name>-<rev>/` | mutable | local-only | dev, ad-hoc `estante run`, fast iteration |
/// | [`Placement::Nix`] | `/nix/store/<hash>-<name>-<rev>/` | immutable | fleet-reproducible | home-manager, NixOS, substrate deploys |
/// | [`Placement::Both`] | — | mixed | — | transition state during `estante place` migration |
///
/// `estante place <pkg> --to nix` shifts a single entry; `estante
/// install --placement nix` makes nix the default for new entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    /// User-local cache. Default for `estante install` / `estante run`.
    #[default]
    Cache,
    /// `/nix/store/…`. Materialized via `nix store add-path` at
    /// install time; consumers (home-manager modules, flakes) pin
    /// to the resulting derivation hash.
    Nix,
    /// Both stores carry the bytes — typical mid-migration state.
    Both,
}

impl Placement {
    /// Parse from the lockfile string (lowercase). Empty / unknown
    /// → `Cache` for backward compatibility.
    #[must_use]
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "nix" => Self::Nix,
            "both" => Self::Both,
            _ => Self::Cache,
        }
    }

    /// Canonical lowercase string written to the lockfile.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cache => "cache",
            Self::Nix => "nix",
            Self::Both => "both",
        }
    }

    /// True if this placement requires nix tooling on PATH.
    #[must_use]
    pub fn needs_nix(self) -> bool {
        matches!(self, Self::Nix | Self::Both)
    }
}

impl fmt::Display for Placement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ─── Errors ───────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum EstanteError {
    #[error("tatara-lisp parse error in {context}: {message}")]
    Parse { context: String, message: String },
    #[error("unrecognized source scheme: {0} (known: github, git+https, git+ssh, gist, local)")]
    UnknownScheme(String),
    #[error("malformed source `{raw}`: {message}")]
    MalformedSource { raw: String, message: String },
    #[error("duplicate package name `{0}` in manifest")]
    DuplicateName(String),
    #[error("lockfile entry for `{0}` missing materialized-path")]
    MissingMaterializedPath(String),
    #[error("formatter error: {0}")]
    Fmt(#[from] fmt::Error),
}

pub type EstanteResult<T> = Result<T, EstanteError>;

// ─── Source ADT ───────────────────────────────────────────────────────

/// Parsed source URL. Constructed from the string carried by
/// [`PkgSpec::source`]; round-trips back to the same string via
/// [`Source::to_source_string`].
///
/// ```
/// use estante_types::Source;
/// let s = Source::parse("github:org/repo@v1.0").unwrap();
/// assert!(matches!(s, Source::Github { .. }));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `github:owner/repo[@ref]`. Ref defaults to `"HEAD"` if absent.
    Github {
        owner: String,
        repo: String,
        reference: String,
    },
    /// `git+https://host/path.git[#ref]`. Ref defaults to `"HEAD"`.
    GitHttps { url: String, reference: String },
    /// `git+ssh://host/path.git[#ref]`. Ref defaults to `"HEAD"`.
    GitSsh { url: String, reference: String },
    /// `gist:gist-id[@ref]`. Convenience for GitHub Gists.
    Gist { id: String, reference: String },
    /// `local:./relative/path`. Used in tests + monorepo demos.
    Local { path: String },
}

impl Source {
    /// Parse a `source:` string into its ADT form.
    pub fn parse(s: &str) -> EstanteResult<Self> {
        let Some((scheme, rest)) = split_source_scheme(s) else {
            return Err(EstanteError::UnknownScheme(s.to_owned()));
        };
        match scheme {
            "github" => parse_github(rest, s),
            "git+https" => parse_git_with_fragment(rest, s, true),
            "git+ssh" => parse_git_with_fragment(rest, s, false),
            "gist" => parse_gist(rest, s),
            "local" => Ok(Self::Local { path: rest.to_owned() }),
            // `RECOGNIZED_SCHEMES` is the closed enumeration; any
            // unknown scheme is caught at the `split_source_scheme`
            // step above. Reaching this branch means we added a new
            // scheme without wiring its parser — fail loud.
            other => Err(EstanteError::UnknownScheme(other.to_owned())),
        }
    }

    /// Reverse of [`Source::parse`] — emit the canonical `source:`
    /// string. Round-trip property: `Source::parse(s.to_source_string()) == Ok(s)`.
    #[must_use]
    pub fn to_source_string(&self) -> String {
        let mut out = String::new();
        // The `Source` Display impl writes the canonical
        // `scheme:rest[@ref]` form via `write!` — typed-emission via
        // the Display block. Unwrap is sound because writing to a
        // `String` cannot fail.
        let _ = write!(out, "{self}");
        out
    }

    /// True if this source is resolvable via the GitHub REST API
    /// (search + tarball download). The resolver uses this to pick
    /// between octocrab and the plain git transport.
    #[must_use]
    pub fn is_github(&self) -> bool {
        matches!(self, Self::Github { .. } | Self::Gist { .. })
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Github {
                owner,
                repo,
                reference,
            } if reference == "HEAD" => write!(f, "github:{owner}/{repo}"),
            Self::Github {
                owner,
                repo,
                reference,
            } => write!(f, "github:{owner}/{repo}@{reference}"),
            Self::GitHttps { url, reference } if reference == "HEAD" => {
                write!(f, "git+https:{url}")
            }
            Self::GitHttps { url, reference } => write!(f, "git+https:{url}#{reference}"),
            Self::GitSsh { url, reference } if reference == "HEAD" => {
                write!(f, "git+ssh:{url}")
            }
            Self::GitSsh { url, reference } => write!(f, "git+ssh:{url}#{reference}"),
            Self::Gist { id, reference } if reference == "HEAD" => write!(f, "gist:{id}"),
            Self::Gist { id, reference } => write!(f, "gist:{id}@{reference}"),
            Self::Local { path } => write!(f, "local:{path}"),
        }
    }
}

fn parse_github(rest: &str, original: &str) -> EstanteResult<Source> {
    let (slug, reference) = rest
        .split_once('@')
        .map_or((rest, "HEAD"), |(s, r)| (s, r));
    let (owner, repo) = slug.split_once('/').ok_or_else(|| EstanteError::MalformedSource {
        raw: original.to_owned(),
        message: "expected `owner/repo` after `github:`".to_owned(),
    })?;
    if owner.is_empty() || repo.is_empty() {
        return Err(EstanteError::MalformedSource {
            raw: original.to_owned(),
            message: "owner and repo must be non-empty".to_owned(),
        });
    }
    Ok(Source::Github {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        reference: reference.to_owned(),
    })
}

fn parse_git_with_fragment(rest: &str, _original: &str, https: bool) -> EstanteResult<Source> {
    let (url, reference) = rest
        .split_once('#')
        .map_or((rest, "HEAD"), |(u, r)| (u, r));
    if https {
        Ok(Source::GitHttps {
            url: url.to_owned(),
            reference: reference.to_owned(),
        })
    } else {
        Ok(Source::GitSsh {
            url: url.to_owned(),
            reference: reference.to_owned(),
        })
    }
}

fn parse_gist(rest: &str, _original: &str) -> EstanteResult<Source> {
    let (id, reference) = rest
        .split_once('@')
        .map_or((rest, "HEAD"), |(i, r)| (i, r));
    Ok(Source::Gist {
        id: id.to_owned(),
        reference: reference.to_owned(),
    })
}

// ─── Manifest ─────────────────────────────────────────────────────────

/// The parsed contents of one `shellpkg.lisp` file — the package
/// author's manifest. Carries one or more [`PkgSpec`] declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub packages: Vec<PkgSpec>,
}

impl Manifest {
    /// Parse a manifest from raw Lisp source.
    pub fn parse(src: &str) -> EstanteResult<Self> {
        let packages: Vec<PkgSpec> = tatara_lisp::compile_typed(src).map_err(|e| {
            EstanteError::Parse {
                context: "manifest".to_owned(),
                message: e.to_string(),
            }
        })?;
        let mut seen = std::collections::HashSet::new();
        for p in &packages {
            if !seen.insert(&p.name) {
                return Err(EstanteError::DuplicateName(p.name.clone()));
            }
        }
        Ok(Self { packages })
    }

    /// Find a package by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&PkgSpec> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// Add a package; replaces any existing entry with the same name.
    pub fn upsert(&mut self, pkg: PkgSpec) {
        if let Some(existing) = self.packages.iter_mut().find(|p| p.name == pkg.name) {
            *existing = pkg;
        } else {
            self.packages.push(pkg);
        }
    }
}

impl fmt::Display for Manifest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, ";; shellpkg.lisp — estante manifest.")?;
        writeln!(f, ";; Authored by hand or via `estante add`.")?;
        writeln!(f, ";; Lockfile: shellpkg.lock.lisp (emitted by `estante lock`).")?;
        writeln!(f)?;
        for pkg in &self.packages {
            write!(f, "{}", PkgSpecDisplay(pkg))?;
            writeln!(f)?;
        }
        Ok(())
    }
}

struct PkgSpecDisplay<'a>(&'a PkgSpec);

impl fmt::Display for PkgSpecDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = self.0;
        writeln!(f, "(defshellpkg")?;
        writeln!(f, "  :name    {}", LispString(&p.name))?;
        writeln!(f, "  :version {}", LispString(&p.version))?;
        writeln!(f, "  :source  {}", LispString(&p.source))?;
        if !p.exports.is_empty() {
            write!(f, "  :exports ")?;
            writeln!(f, "{}", LispStringList(&p.exports))?;
        }
        if !p.deps.is_empty() {
            write!(f, "  :deps    ")?;
            writeln!(f, "{}", LispStringList(&p.deps))?;
        }
        if p.lazy {
            writeln!(f, "  :lazy    #t")?;
        }
        writeln!(f, "  )")
    }
}

// ─── Lockfile ─────────────────────────────────────────────────────────

/// Parsed contents of one `shellpkg.lock.lisp` file — `estante install`
/// emits, `frost-lisp` reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lockfile {
    pub entries: Vec<LockedPkgSpec>,
}

impl Lockfile {
    pub fn parse(src: &str) -> EstanteResult<Self> {
        let raw: Vec<LockedPkgSpec> = tatara_lisp::compile_typed(src).map_err(|e| {
            EstanteError::Parse {
                context: "lockfile".to_owned(),
                message: e.to_string(),
            }
        })?;
        let entries = raw
            .into_iter()
            .map(|mut e| {
                // Normalize placement — older lockfiles + serde
                // defaults give empty string; canonical value is
                // "cache".
                if e.placement.is_empty() {
                    e.placement = Placement::Cache.as_str().to_owned();
                }
                e
            })
            .collect();
        Ok(Self { entries })
    }

    /// Look up the locked entry for a package name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&LockedPkgSpec> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Upsert a locked entry by name.
    pub fn upsert(&mut self, entry: LockedPkgSpec) {
        if let Some(existing) = self.entries.iter_mut().find(|e| e.name == entry.name) {
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
    }

    /// Validate every entry carries a materialized path. Run by the
    /// resolver after install before writing the lockfile to disk —
    /// catches the bug where a fetch was attempted but never stored.
    pub fn validate_materialized(&self) -> EstanteResult<()> {
        for e in &self.entries {
            if e.materialized_path.is_empty() {
                return Err(EstanteError::MissingMaterializedPath(e.name.clone()));
            }
        }
        Ok(())
    }
}

impl fmt::Display for Lockfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, ";; shellpkg.lock.lisp — machine-emitted by `estante install`.")?;
        writeln!(f, ";; DO NOT EDIT BY HAND. Re-run `estante lock` instead.")?;
        writeln!(f, ";; Content-addressed by (rev, nar-hash, blake3).")?;
        writeln!(f)?;
        for e in &self.entries {
            write!(f, "{}", LockedPkgSpecDisplay(e))?;
            writeln!(f)?;
        }
        Ok(())
    }
}

struct LockedPkgSpecDisplay<'a>(&'a LockedPkgSpec);

impl fmt::Display for LockedPkgSpecDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let l = self.0;
        writeln!(f, "(deflockedpkg")?;
        writeln!(f, "  :name              {}", LispString(&l.name))?;
        writeln!(f, "  :source            {}", LispString(&l.source))?;
        writeln!(f, "  :rev               {}", LispString(&l.rev))?;
        writeln!(f, "  :nar-hash          {}", LispString(&l.nar_hash))?;
        writeln!(f, "  :blake3            {}", LispString(&l.blake3))?;
        writeln!(
            f,
            "  :materialized-path {}",
            LispString(&l.materialized_path)
        )?;
        // Placement always emitted (normalized to "cache" for empty
        // serde defaults) — keeps Display/parse a pure round-trip.
        let placement = if l.placement.is_empty() { "cache" } else { l.placement.as_str() };
        writeln!(f, "  :placement         {}", LispString(placement))?;
        writeln!(f, "  )")
    }
}

// ─── Lisp value formatters (the typed-emission surface) ──────────────

/// Newtype wrapper that emits a Rust string as a Lisp string literal
/// (`"…"`) with double-quote and backslash escaped. The `Display`
/// impl is the typed write-surface — direct `format!()` on a String
/// is forbidden by the typed-emission rule, but `LispString` exists
/// so callers say `write!(f, "{}", LispString(name))` and get correct
/// escaping for free.
pub struct LispString<'a>(pub &'a str);

impl fmt::Display for LispString<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"")?;
        for c in self.0.chars() {
            match c {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                _ => f.write_char(c)?,
            }
        }
        f.write_str("\"")
    }
}

/// Emit a Lisp list of strings — `("a" "b" "c")`. Empty list emits `()`.
pub struct LispStringList<'a>(pub &'a [String]);

impl fmt::Display for LispStringList<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(")?;
        for (i, s) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{}", LispString(s))?;
        }
        f.write_str(")")
    }
}

// ─── Render helpers ──────────────────────────────────────────────────

/// Render a [`Manifest`] back to Lisp source. Equivalent to
/// `manifest.to_string()` but takes a `Write` so callers can stream to
/// a file without allocating the full source.
pub mod render {
    use super::Manifest;
    use super::Lockfile;
    use std::fmt::Write;

    /// Render a manifest into an existing buffer.
    pub fn manifest(out: &mut String, m: &Manifest) -> std::fmt::Result {
        write!(out, "{m}")
    }

    /// Render a lockfile into an existing buffer.
    pub fn lockfile(out: &mut String, l: &Lockfile) -> std::fmt::Result {
        write!(out, "{l}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_parse_github_with_ref() {
        let s = Source::parse("github:MichaelAquilina/zsh-you-should-use@v1.7.4").unwrap();
        assert_eq!(
            s,
            Source::Github {
                owner: "MichaelAquilina".into(),
                repo: "zsh-you-should-use".into(),
                reference: "v1.7.4".into(),
            }
        );
        assert!(s.is_github());
    }

    #[test]
    fn source_parse_github_no_ref_uses_head() {
        let s = Source::parse("github:org/repo").unwrap();
        let Source::Github { reference, .. } = s else {
            panic!("expected Github");
        };
        assert_eq!(reference, "HEAD");
    }

    #[test]
    fn source_parse_local() {
        let s = Source::parse("local:./pkgs/foo").unwrap();
        assert_eq!(
            s,
            Source::Local {
                path: "./pkgs/foo".into()
            }
        );
        assert!(!s.is_github());
    }

    #[test]
    fn source_parse_unknown_scheme_errors() {
        let err = Source::parse("ftp:nope").unwrap_err();
        assert!(matches!(err, EstanteError::UnknownScheme(_)));
    }

    #[test]
    fn source_parse_malformed_github_errors() {
        let err = Source::parse("github:no-slash").unwrap_err();
        assert!(matches!(err, EstanteError::MalformedSource { .. }));
    }

    #[test]
    fn source_round_trip() {
        for raw in [
            "github:org/repo",
            "github:org/repo@v1.0.0",
            "git+https:example.org/foo.git#abc",
            "git+ssh:git@example.org:foo.git#abc",
            "gist:abc123",
            "gist:abc123@v1",
            "local:./pkgs/foo",
        ] {
            let parsed = Source::parse(raw).unwrap();
            let rendered = parsed.to_source_string();
            assert_eq!(
                rendered, raw,
                "round-trip failed for {raw}: rendered as {rendered}"
            );
        }
    }

    #[test]
    fn manifest_parse_then_render_round_trips() {
        let src = r#"
            (defshellpkg :name "foo" :version "1.0.0" :source "github:org/foo")
            (defshellpkg :name "bar" :version "0.2.0" :source "github:org/bar"
                         :exports ("alias" "hook") :lazy #t)
        "#;
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.packages.len(), 2);
        assert_eq!(m.get("foo").map(|p| p.version.as_str()), Some("1.0.0"));
        assert_eq!(m.get("bar").map(|p| p.lazy), Some(true));

        // Render then re-parse — same packages emerge.
        let rendered = m.to_string();
        let re = Manifest::parse(&rendered).unwrap();
        assert_eq!(re, m);
    }

    #[test]
    fn manifest_duplicate_name_rejected() {
        let src = r#"
            (defshellpkg :name "dup" :version "1" :source "github:x/y")
            (defshellpkg :name "dup" :version "2" :source "github:x/z")
        "#;
        let err = Manifest::parse(src).unwrap_err();
        assert!(matches!(err, EstanteError::DuplicateName(n) if n == "dup"));
    }

    #[test]
    fn manifest_upsert_replaces_existing() {
        let mut m = Manifest::default();
        m.upsert(PkgSpec {
            name: "foo".into(),
            version: "1.0".into(),
            source: "github:x/y".into(),
            exports: vec![],
            deps: vec![],
            lazy: false,
        });
        m.upsert(PkgSpec {
            name: "foo".into(),
            version: "2.0".into(),
            source: "github:x/y".into(),
            exports: vec![],
            deps: vec![],
            lazy: false,
        });
        assert_eq!(m.packages.len(), 1);
        assert_eq!(m.get("foo").unwrap().version, "2.0");
    }

    #[test]
    fn lockfile_round_trip() {
        let src = r#"
            (deflockedpkg :name "foo" :source "github:org/foo"
                          :rev "abc123" :nar-hash "sha256-aa" :blake3 "blake3-bb"
                          :materialized-path "/nix/store/abc-foo/")
        "#;
        let l = Lockfile::parse(src).unwrap();
        assert_eq!(l.entries.len(), 1);
        let rendered = l.to_string();
        let re = Lockfile::parse(&rendered).unwrap();
        assert_eq!(re, l);
        l.validate_materialized().unwrap();
    }

    #[test]
    fn lockfile_validate_catches_missing_path() {
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "foo".into(),
            source: "github:org/foo".into(),
            rev: "abc".into(),
            nar_hash: "sha256-aa".into(),
            blake3: "blake3-bb".into(),
            materialized_path: String::new(),
            placement: "cache".into(),
        });
        let err = l.validate_materialized().unwrap_err();
        assert!(matches!(err, EstanteError::MissingMaterializedPath(_)));
    }

    #[test]
    fn lisp_string_escapes_special_chars() {
        let mut buf = String::new();
        write!(buf, "{}", LispString("he said \"hi\\there\"")).unwrap();
        assert_eq!(buf, r#""he said \"hi\\there\"""#);
    }

    #[test]
    fn lisp_string_list_empty_is_parens() {
        let empty: Vec<String> = vec![];
        let mut buf = String::new();
        write!(buf, "{}", LispStringList(&empty)).unwrap();
        assert_eq!(buf, "()");
    }

    #[test]
    fn lisp_string_list_singletons_and_pairs() {
        let one = vec!["a".to_owned()];
        let mut buf = String::new();
        write!(buf, "{}", LispStringList(&one)).unwrap();
        assert_eq!(buf, r#"("a")"#);

        let two = vec!["a".to_owned(), "b".to_owned()];
        let mut buf = String::new();
        write!(buf, "{}", LispStringList(&two)).unwrap();
        assert_eq!(buf, r#"("a" "b")"#);
    }

    // ─── Extensive coverage — added by the testing-sweep pass. ─────────

    #[test]
    fn empty_manifest_round_trips() {
        let m = Manifest::default();
        let rendered = m.to_string();
        let reparsed = Manifest::parse(&rendered).unwrap();
        assert_eq!(reparsed, m);
        assert_eq!(reparsed.packages.len(), 0);
    }

    #[test]
    fn empty_lockfile_round_trips() {
        let l = Lockfile::default();
        let rendered = l.to_string();
        let reparsed = Lockfile::parse(&rendered).unwrap();
        assert_eq!(reparsed, l);
    }

    #[test]
    fn multi_package_manifest_round_trips_with_mixed_optional_fields() {
        // One bare-bones pkg, one with exports only, one with deps + lazy,
        // one with everything. Exercises all the conditional emit branches
        // in PkgSpecDisplay.
        let src = r#"
            (defshellpkg :name "bare"  :version "0.1" :source "github:o/b")
            (defshellpkg :name "exp"   :version "0.2" :source "github:o/e"
                         :exports ("alias"))
            (defshellpkg :name "deps"  :version "0.3" :source "github:o/d"
                         :deps ("first@^1" "second@^2") :lazy #t)
            (defshellpkg :name "everything" :version "1.0" :source "github:o/a"
                         :exports ("alias" "hook" "completion")
                         :deps ("dep-one@^1.0")
                         :lazy #t)
        "#;
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.packages.len(), 4);
        let rendered = m.to_string();
        let re = Manifest::parse(&rendered).unwrap();
        assert_eq!(re, m);
        // Spot-check the optional fields survived round-trip.
        assert!(re.get("bare").unwrap().exports.is_empty());
        assert_eq!(re.get("exp").unwrap().exports, vec!["alias"]);
        assert!(re.get("deps").unwrap().lazy);
        assert_eq!(re.get("deps").unwrap().deps.len(), 2);
        assert_eq!(re.get("everything").unwrap().exports.len(), 3);
    }

    #[test]
    fn manifest_get_returns_none_for_missing() {
        let m = Manifest::default();
        assert!(m.get("nonexistent").is_none());
    }

    #[test]
    fn lockfile_upsert_replaces_existing_entry() {
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "foo".into(),
            source: "github:x/y".into(),
            rev: "abc".into(),
            nar_hash: "sha256-a".into(),
            blake3: "blake3-a".into(),
            materialized_path: "/p/a".into(),
            placement: "cache".into(),
        });
        l.upsert(LockedPkgSpec {
            name: "foo".into(),
            source: "github:x/y".into(),
            rev: "def".into(),
            nar_hash: "sha256-b".into(),
            blake3: "blake3-b".into(),
            materialized_path: "/p/b".into(),
            placement: "cache".into(),
        });
        assert_eq!(l.entries.len(), 1);
        assert_eq!(l.get("foo").unwrap().rev, "def");
        assert_eq!(l.get("foo").unwrap().materialized_path, "/p/b");
    }

    #[test]
    fn lockfile_get_returns_none_for_missing() {
        let l = Lockfile::default();
        assert!(l.get("ghost").is_none());
    }

    #[test]
    fn source_git_https_with_no_fragment_uses_head() {
        let s = Source::parse("git+https:example.org/foo.git").unwrap();
        match &s {
            Source::GitHttps { url, reference } => {
                assert_eq!(url, "example.org/foo.git");
                assert_eq!(reference, "HEAD");
            }
            _ => panic!("expected GitHttps"),
        }
        assert!(!s.is_github());
    }

    #[test]
    fn source_git_ssh_round_trip() {
        let s = Source::parse("git+ssh:git@example.org:foo.git#abc").unwrap();
        match &s {
            Source::GitSsh { url, reference } => {
                assert_eq!(url, "git@example.org:foo.git");
                assert_eq!(reference, "abc");
            }
            _ => panic!("expected GitSsh"),
        }
        assert_eq!(s.to_source_string(), "git+ssh:git@example.org:foo.git#abc");
    }

    #[test]
    fn source_gist_with_ref_round_trip() {
        let s = Source::parse("gist:abc123@v1").unwrap();
        match &s {
            Source::Gist { id, reference } => {
                assert_eq!(id, "abc123");
                assert_eq!(reference, "v1");
            }
            _ => panic!("expected Gist"),
        }
        assert!(s.is_github(), "gists count as github-resolvable");
    }

    #[test]
    fn source_empty_string_unknown_scheme() {
        let err = Source::parse("").unwrap_err();
        assert!(matches!(err, EstanteError::UnknownScheme(_)));
    }

    #[test]
    fn source_github_empty_owner_rejected() {
        let err = Source::parse("github:/repo").unwrap_err();
        assert!(matches!(err, EstanteError::MalformedSource { .. }));
    }

    #[test]
    fn source_github_empty_repo_rejected() {
        let err = Source::parse("github:owner/").unwrap_err();
        assert!(matches!(err, EstanteError::MalformedSource { .. }));
    }

    #[test]
    fn source_is_github_classifies_all_variants() {
        assert!(Source::parse("github:o/r").unwrap().is_github());
        assert!(Source::parse("gist:abc").unwrap().is_github());
        assert!(!Source::parse("git+https:x.org/y.git").unwrap().is_github());
        assert!(!Source::parse("git+ssh:git@x.org:y.git").unwrap().is_github());
        assert!(!Source::parse("local:./foo").unwrap().is_github());
    }

    #[test]
    fn lisp_string_escape_round_trips_via_compile_typed() {
        // Author a PkgSpec with payloads in name/version/source that
        // exercise every escape branch, render it via Manifest::Display,
        // then re-parse via tatara-lisp. If LispString and tatara-lisp
        // disagree on escaping, this assertion catches it before users
        // do.
        let m = {
            let mut m = Manifest::default();
            m.upsert(PkgSpec {
                name: r#"has-"quote""#.to_owned(),
                version: r#"v\back"#.to_owned(),
                source: r#"github:org/repo with spaces"#.to_owned(),
                exports: vec![r#"alias-with-"inner"-quote"#.to_owned()],
                deps: vec![],
                lazy: false,
            });
            m
        };
        let rendered = m.to_string();
        let re = Manifest::parse(&rendered).unwrap();
        assert_eq!(re, m);
    }

    #[test]
    fn lisp_string_handles_unicode() {
        let mut buf = String::new();
        write!(buf, "{}", LispString("café 🌸 日本語")).unwrap();
        assert_eq!(buf, r#""café 🌸 日本語""#);
    }

    #[test]
    fn lisp_string_doesnt_escape_safe_chars() {
        let mut buf = String::new();
        write!(buf, "{}", LispString("a/b:c-d_e.f@g123")).unwrap();
        assert_eq!(buf, r#""a/b:c-d_e.f@g123""#);
    }

    #[test]
    fn pkg_spec_round_trip_with_lazy_off_omits_field() {
        let m = {
            let mut m = Manifest::default();
            m.upsert(PkgSpec {
                name: "x".into(),
                version: "1".into(),
                source: "github:o/r".into(),
                exports: vec![],
                deps: vec![],
                lazy: false,
            });
            m
        };
        let rendered = m.to_string();
        assert!(!rendered.contains(":lazy"), "lazy=false must not emit the slot: {rendered}");
        let re = Manifest::parse(&rendered).unwrap();
        assert_eq!(re, m);
    }

    // ─── Placement ─────────────────────────────────────────────────

    #[test]
    fn placement_from_str_canonical() {
        assert_eq!(Placement::from_str("cache"), Placement::Cache);
        assert_eq!(Placement::from_str("nix"), Placement::Nix);
        assert_eq!(Placement::from_str("both"), Placement::Both);
    }

    #[test]
    fn placement_from_str_empty_defaults_to_cache() {
        assert_eq!(Placement::from_str(""), Placement::Cache);
        assert_eq!(Placement::from_str("   "), Placement::Cache);
    }

    #[test]
    fn placement_from_str_case_insensitive() {
        assert_eq!(Placement::from_str("NIX"), Placement::Nix);
        assert_eq!(Placement::from_str("Nix"), Placement::Nix);
        assert_eq!(Placement::from_str("BOTH"), Placement::Both);
    }

    #[test]
    fn placement_from_str_unknown_defaults_to_cache() {
        assert_eq!(Placement::from_str("xyz"), Placement::Cache);
        assert_eq!(Placement::from_str("local"), Placement::Cache);
    }

    #[test]
    fn placement_as_str_round_trips() {
        for p in [Placement::Cache, Placement::Nix, Placement::Both] {
            assert_eq!(Placement::from_str(p.as_str()), p);
        }
    }

    #[test]
    fn placement_needs_nix() {
        assert!(!Placement::Cache.needs_nix());
        assert!(Placement::Nix.needs_nix());
        assert!(Placement::Both.needs_nix());
    }

    #[test]
    fn placement_default_is_cache() {
        assert_eq!(Placement::default(), Placement::Cache);
    }

    #[test]
    fn placement_display_matches_as_str() {
        assert_eq!(format!("{}", Placement::Cache), "cache");
        assert_eq!(format!("{}", Placement::Nix), "nix");
        assert_eq!(format!("{}", Placement::Both), "both");
    }

    #[test]
    fn lockfile_emits_placement_unconditionally() {
        // Cache placement (default) STILL emits — keeps Display/parse
        // a round-trip invariant.
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "foo".into(),
            source: "github:o/r".into(),
            rev: "abc".into(),
            nar_hash: String::new(),
            blake3: "b3".into(),
            materialized_path: "/p".into(),
            placement: "cache".into(),
        });
        let rendered = l.to_string();
        assert!(
            rendered.contains(":placement         \"cache\""),
            "placement must always emit: {rendered}"
        );
    }

    #[test]
    fn lockfile_empty_placement_canonicalizes_to_cache() {
        // An older-format entry with empty placement gets normalized
        // on parse — the Lockfile in memory has "cache".
        let src = r#"
            (deflockedpkg :name "foo" :source "github:o/r" :rev "abc"
                          :nar-hash "" :blake3 "b3"
                          :materialized-path "/p"
                          :placement "")
        "#;
        let l = Lockfile::parse(src).unwrap();
        assert_eq!(l.entries[0].placement, "cache");
    }

    #[test]
    fn lockfile_missing_placement_field_canonicalizes_to_cache() {
        // No `:placement` slot at all — older lockfiles. Same normalization.
        let src = r#"
            (deflockedpkg :name "foo" :source "github:o/r" :rev "abc"
                          :nar-hash "" :blake3 "b3"
                          :materialized-path "/p")
        "#;
        let l = Lockfile::parse(src).unwrap();
        assert_eq!(l.entries[0].placement, "cache");
    }

    #[test]
    fn lockfile_nix_placement_round_trips() {
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "foo".into(),
            source: "github:o/r".into(),
            rev: "abc".into(),
            nar_hash: "sha256-x".into(),
            blake3: "b3".into(),
            materialized_path: "/nix/store/abc-foo/".into(),
            placement: "nix".into(),
        });
        let rendered = l.to_string();
        assert!(rendered.contains(":placement         \"nix\""));
        let re = Lockfile::parse(&rendered).unwrap();
        assert_eq!(re, l);
    }

    #[test]
    fn lockfile_both_placement_round_trips() {
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
        let rendered = l.to_string();
        assert!(rendered.contains(":placement         \"both\""));
        let re = Lockfile::parse(&rendered).unwrap();
        assert_eq!(re, l);
    }

    #[test]
    fn lockfile_with_mixed_placements_round_trips() {
        let mut l = Lockfile::default();
        for (name, p) in [("cached", "cache"), ("nixed", "nix"), ("dual", "both")] {
            l.upsert(LockedPkgSpec {
                name: name.into(),
                source: format!("github:o/{name}"),
                rev: "abc".into(),
                nar_hash: "sha256-x".into(),
                blake3: "b3".into(),
                materialized_path: format!("/p/{name}"),
                placement: p.into(),
            });
        }
        let rendered = l.to_string();
        let re = Lockfile::parse(&rendered).unwrap();
        assert_eq!(re, l);
        assert_eq!(re.get("cached").unwrap().placement, "cache");
        assert_eq!(re.get("nixed").unwrap().placement, "nix");
        assert_eq!(re.get("dual").unwrap().placement, "both");
    }

    #[test]
    fn pkg_spec_lazy_true_round_trips() {
        let m = {
            let mut m = Manifest::default();
            m.upsert(PkgSpec {
                name: "x".into(),
                version: "1".into(),
                source: "github:o/r".into(),
                exports: vec![],
                deps: vec![],
                lazy: true,
            });
            m
        };
        let rendered = m.to_string();
        assert!(rendered.contains(":lazy    #t"), "lazy=true must emit slot: {rendered}");
        let re = Manifest::parse(&rendered).unwrap();
        assert!(re.get("x").unwrap().lazy);
    }
}
