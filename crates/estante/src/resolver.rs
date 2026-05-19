//! The resolver — manifest in, lockfile out.
//!
//! v0.1 is GitHub-only (per `fetch.rs`); transitive dep walking is a
//! follow-up (the typed surface is `PkgSpec::deps` which carries
//! `"name@constraint"` strings — wire-format ready, semantics in v0.2).

use std::path::{Path, PathBuf};

use anyhow::Context;
use estante_types::{LockedPkgSpec, Lockfile, Manifest, PkgSpec, Source};

use crate::cache;
use crate::config::Config;
use crate::fetch;
use crate::hash;

/// The resolution pipeline. Stateless; constructed per CLI invocation.
pub struct Resolver<'a> {
    cfg: &'a Config,
    client: octocrab::Octocrab,
    /// Directory `local:` sources resolve against. Mirrors how
    /// `defsource :path "relative.lisp"` resolves against the
    /// sourcing rc-file's directory in frost-lisp — same mental
    /// model. When `None`, relative `local:` paths fall through to
    /// CWD-relative (the behavior pre-`with_base_dir`).
    base_dir: Option<PathBuf>,
}

impl<'a> Resolver<'a> {
    pub fn new(cfg: &'a Config) -> anyhow::Result<Self> {
        let client = fetch::build_client(cfg.github_token.as_deref())?;
        Ok(Self {
            cfg,
            client,
            base_dir: None,
        })
    }

    /// Anchor `local:` relative sources at `dir`. Typically called
    /// with `manifest_path.parent()` so the lockfile resolves the
    /// same source paths the author saw at write-time.
    #[must_use]
    pub fn with_base_dir(mut self, dir: PathBuf) -> Self {
        self.base_dir = Some(dir);
        self
    }

    /// Resolve every package in `manifest` against the network +
    /// cache and return a fully-materialized `Lockfile`. Each entry
    /// has its rev pinned to a 40-char SHA and its package tree
    /// already unpacked under `cfg.cache_dir`.
    pub async fn resolve(&self, manifest: &Manifest) -> anyhow::Result<Lockfile> {
        cache::ensure_layout(self.cfg)?;
        let mut lock = Lockfile::default();
        for pkg in &manifest.packages {
            let entry = self.resolve_one(pkg).await?;
            lock.upsert(entry);
        }
        lock.validate_materialized()?;
        Ok(lock)
    }

    /// Resolve a single package — used by `add` after appending to
    /// the manifest, by `lock` for the full set, and by `install` to
    /// fill any missing entries.
    pub async fn resolve_one(&self, pkg: &PkgSpec) -> anyhow::Result<LockedPkgSpec> {
        let parsed = Source::parse(&pkg.source)
            .with_context(|| format!("parsing source for package {}", pkg.name))?;
        // Normalize `local:relative` paths against the resolver's
        // base_dir so the LockedPkgSpec carries an absolute path. The
        // lockfile is then self-contained — `install` on a different
        // host with the same fixture reproduces the same lock entry.
        let source = self.normalize_source(parsed);
        let rev = fetch::resolve_rev(&self.client, &source)
            .await
            .with_context(|| format!("resolving rev for {}", pkg.name))?;
        let dest = self.cfg.store_path(&pkg.name, &rev.sha);
        if cache::is_unpacked_pkg(&dest) {
            tracing::info!(name = %pkg.name, rev = %rev.sha, "cache hit");
        } else {
            let r = fetch::download_and_unpack(&self.client, &source, &rev.sha, &dest)
                .await
                .with_context(|| format!("fetching {} @ {}", pkg.name, rev.sha))?;
            tracing::info!(
                name = %pkg.name,
                rev = %rev.sha,
                files = r.file_count,
                "fetched"
            );
        }
        // The canonical content-address: BLAKE3 of the *unpacked*
        // tree, walked in deterministic sorted order. Identical across
        // cache hits and fresh fetches; identical across machines and
        // tar/gzip implementations; one-line attestation receipt of
        // "what frost-lisp is going to see when it defloads me."
        let blake3 = hash::blake3_tree(&dest)
            .with_context(|| format!("hashing materialized tree for {}", pkg.name))?;
        Ok(LockedPkgSpec {
            name: pkg.name.clone(),
            source: source.to_source_string(),
            rev: rev.sha,
            // narHash stays empty for v0.1 cache placement. The
            // `estante place ... --to nix` migration fills it via
            // `nix-store --query --hash` at promotion time.
            nar_hash: String::new(),
            blake3,
            materialized_path: path_to_string(&dest),
            // The resolver defaults to cache placement; explicit
            // nix placement happens via `estante install
            // --placement nix` or `estante place ... --to nix`.
            placement: estante_types::Placement::Cache.as_str().to_owned(),
        })
    }
}

fn path_to_string(p: &PathBuf) -> String {
    p.to_string_lossy().into_owned()
}

impl Resolver<'_> {
    /// Promote `local:relative` paths to `local:/absolute/path` if a
    /// base_dir is configured. Other Source variants pass through.
    fn normalize_source(&self, source: Source) -> Source {
        if let Source::Local { path } = &source {
            let p = Path::new(path);
            if p.is_absolute() {
                return source;
            }
            if let Some(base) = &self.base_dir {
                // Don't require the path to exist yet — the caller's
                // fetcher emits a clean error if it doesn't. Just
                // canonicalize what we can; absent path = pass the
                // joined form through unchanged.
                let joined = base.join(p);
                let canonical = std::fs::canonicalize(&joined).unwrap_or(joined);
                return Source::Local {
                    path: canonical.to_string_lossy().into_owned(),
                };
            }
        }
        source
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_to_string_round_trips_simple_paths() {
        let p = PathBuf::from("/nix/store/abc-foo");
        assert_eq!(path_to_string(&p), "/nix/store/abc-foo");
    }

    #[tokio::test]
    async fn normalize_source_promotes_relative_local_against_base_dir() {
        let tmp = std::env::temp_dir().join(format!("estante-normalize-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let pkg_dir = tmp.join("packages").join("foo");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        let cfg = Config {
            cache_dir: tmp.join("cache"),
            github_token: None,
        };
        let resolver = Resolver::new(&cfg).unwrap().with_base_dir(tmp.clone());
        let source = Source::Local {
            path: "packages/foo".to_owned(),
        };
        let normalized = resolver.normalize_source(source);
        match normalized {
            Source::Local { path } => {
                let canonical = std::fs::canonicalize(&pkg_dir).unwrap();
                assert_eq!(path, canonical.to_string_lossy());
            }
            other => panic!("expected Source::Local, got {other:?}"),
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[tokio::test]
    async fn normalize_source_leaves_absolute_local_unchanged() {
        let cfg = Config {
            cache_dir: PathBuf::from("/tmp/cache"),
            github_token: None,
        };
        let resolver = Resolver::new(&cfg).unwrap();
        let source = Source::Local {
            path: "/already/absolute".to_owned(),
        };
        let normalized = resolver.normalize_source(source.clone());
        assert_eq!(normalized, source);
    }

    // ─── Normalize_source coverage for every Source variant. ──────────

    #[tokio::test]
    async fn normalize_source_leaves_github_unchanged() {
        let cfg = Config {
            cache_dir: PathBuf::from("/tmp"),
            github_token: None,
        };
        let resolver = Resolver::new(&cfg)
            .unwrap()
            .with_base_dir(PathBuf::from("/wherever"));
        let source = Source::Github {
            owner: "o".into(),
            repo: "r".into(),
            reference: "v1".into(),
        };
        assert_eq!(resolver.normalize_source(source.clone()), source);
    }

    #[tokio::test]
    async fn normalize_source_leaves_gist_unchanged() {
        let cfg = Config {
            cache_dir: PathBuf::from("/tmp"),
            github_token: None,
        };
        let resolver = Resolver::new(&cfg)
            .unwrap()
            .with_base_dir(PathBuf::from("/wherever"));
        let source = Source::Gist {
            id: "abc".into(),
            reference: "HEAD".into(),
        };
        assert_eq!(resolver.normalize_source(source.clone()), source);
    }

    #[tokio::test]
    async fn normalize_source_leaves_git_https_unchanged() {
        let cfg = Config {
            cache_dir: PathBuf::from("/tmp"),
            github_token: None,
        };
        let resolver = Resolver::new(&cfg)
            .unwrap()
            .with_base_dir(PathBuf::from("/wherever"));
        let source = Source::GitHttps {
            url: "example.org/x.git".into(),
            reference: "HEAD".into(),
        };
        assert_eq!(resolver.normalize_source(source.clone()), source);
    }

    #[tokio::test]
    async fn normalize_source_local_no_base_dir_passes_through() {
        // When no base_dir is configured, even relative local paths
        // pass through unchanged (caller's CWD becomes the resolver).
        let cfg = Config {
            cache_dir: PathBuf::from("/tmp"),
            github_token: None,
        };
        let resolver = Resolver::new(&cfg).unwrap();
        let source = Source::Local {
            path: "relative/path".to_owned(),
        };
        assert_eq!(resolver.normalize_source(source.clone()), source);
    }
}
