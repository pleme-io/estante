//! Estante CLI config — cache dir, GitHub token, default mirror.
//!
//! v0.1 is a thin POJO loaded from env vars + XDG paths. A full
//! shikumi-loaded YAML config + cofre `SecretRef` lookup lands in
//! M1d; the [`Config::resolve`] surface stays the same.

use std::path::PathBuf;

/// Resolved CLI configuration. Cheap to clone; held by every async
/// action that needs access to the GitHub client + cache root.
#[derive(Debug, Clone)]
pub struct Config {
    /// `$XDG_CACHE_HOME/estante` (or `$HOME/.cache/estante`). Stores
    /// unpacked tarballs and the octocrab response cache.
    pub cache_dir: PathBuf,
    /// GitHub PAT for authenticated API + tarball requests. `None`
    /// falls back to public, unauthenticated access (60 req/h limit).
    pub github_token: Option<String>,
}

impl Config {
    /// Resolve config from env vars + XDG paths. Takes the CLI-provided
    /// token override so `--github-token` wins over `$GITHUB_TOKEN`.
    pub fn resolve(token_override: Option<String>) -> anyhow::Result<Self> {
        let cache_dir = dirs::cache_dir()
            .ok_or_else(|| anyhow::anyhow!("cannot resolve $XDG_CACHE_HOME"))?
            .join("estante");
        let github_token =
            token_override.or_else(|| std::env::var("GITHUB_TOKEN").ok().filter(|s| !s.is_empty()));
        Ok(Self {
            cache_dir,
            github_token,
        })
    }

    /// Path where a fetched package gets unpacked. The naming
    /// (`<name>-<rev>`) is the content-addressed shape downstream
    /// frost-lisp expects.
    #[must_use]
    pub fn store_path(&self, name: &str, rev: &str) -> PathBuf {
        let short_rev = if rev.len() > 16 { &rev[..16] } else { rev };
        self.cache_dir
            .join("store")
            .join(format!("{name}-{short_rev}"))
    }

    /// True if the config has a usable GitHub PAT. Used by the
    /// search/fetch paths to short-circuit early when private repos
    /// are requested without auth.
    #[must_use]
    pub fn has_token(&self) -> bool {
        self.github_token.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_path_truncates_long_rev() {
        let c = Config {
            cache_dir: PathBuf::from("/tmp/cache"),
            github_token: None,
        };
        let p = c.store_path("foo", "abcdef0123456789deadbeef");
        assert_eq!(p, PathBuf::from("/tmp/cache/store/foo-abcdef0123456789"));
    }

    #[test]
    fn store_path_keeps_short_rev() {
        let c = Config {
            cache_dir: PathBuf::from("/tmp/cache"),
            github_token: None,
        };
        let p = c.store_path("foo", "abc");
        assert_eq!(p, PathBuf::from("/tmp/cache/store/foo-abc"));
    }

    #[test]
    fn explicit_token_override_carries_through() {
        // The unsafe-free corollary: when the caller passes a token,
        // Config::resolve uses it verbatim (env lookup only kicks in
        // when the override is None). This proves precedence-of-CLI
        // without touching the process env.
        let cfg = Config::resolve(Some("override".into())).unwrap();
        assert_eq!(cfg.github_token.as_deref(), Some("override"));
        assert!(cfg.has_token());
    }

    #[test]
    fn missing_token_yields_unauthenticated() {
        // If neither override nor env supplies a token, Config still
        // resolves — public-rate-limited GitHub access is the v0.1
        // fallback.
        let cfg = Config {
            cache_dir: PathBuf::from("/tmp"),
            github_token: None,
        };
        assert!(!cfg.has_token());
    }
}
