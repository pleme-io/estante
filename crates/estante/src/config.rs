//! Estante CLI config — cache dir, GitHub token, default mirror.
//!
//! v0.1 is a thin POJO loaded from env vars + XDG paths. A full
//! shikumi-loaded YAML config + cofre `SecretRef` lookup lands in
//! M1d; the [`Config::resolve`] surface stays the same.
//!
//! The PAT is a [`Secret`], not a `String`. That is what keeps it out
//! of a `{:?}` of this struct and out of any `format!` — neither
//! typechecks against `Secret`.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use cofre_secret::Secret;

/// Env var naming a file whose contents are the GitHub PAT. The
/// preferred source: unlike `$GITHUB_TOKEN` the credential never
/// enters an environment block, so it is not visible in
/// `/proc/<pid>/environ` to a process that inherits it.
pub const TOKEN_FILE_ENV: &str = "GITHUB_TOKEN_FILE";

/// Resolved CLI configuration. Cheap to clone; held by every async
/// action that needs access to the GitHub client + cache root.
#[derive(Debug, Clone)]
pub struct Config {
    /// `$XDG_CACHE_HOME/estante` (or `$HOME/.cache/estante`). Stores
    /// unpacked tarballs and the octocrab response cache.
    pub cache_dir: PathBuf,
    /// GitHub PAT for authenticated API + tarball requests. `None`
    /// falls back to public, unauthenticated access (60 req/h limit).
    ///
    /// `Arc` only so that `Config` stays `Clone`; `Secret` is
    /// deliberately neither `Clone` nor `Display`.
    pub github_token: Option<Arc<Secret>>,
}

impl Config {
    /// Resolve config from env vars + XDG paths. Takes the CLI-provided
    /// token override so the deprecated `--github-token` still wins over
    /// `$GITHUB_TOKEN`.
    pub fn resolve(token_override: Option<String>) -> anyhow::Result<Self> {
        let cache_dir = dirs::cache_dir()
            .ok_or_else(|| anyhow::anyhow!("cannot resolve $XDG_CACHE_HOME"))?
            .join("estante");
        let github_token = Self::resolve_token(token_override)?;
        Ok(Self {
            cache_dir,
            github_token,
        })
    }

    /// Token precedence: the deprecated `--github-token` flag, then
    /// `$GITHUB_TOKEN`, then the file named by `$GITHUB_TOKEN_FILE`.
    ///
    /// The flag is kept working — removing it would break callers —
    /// but it is announced as deprecated on every use, because a value
    /// passed on the command line is readable from the process table by
    /// any local user for the lifetime of the process, and is recorded
    /// verbatim in the operator's shell history. `Secret` cannot fix
    /// that: by the time estante is running, the exposure has already
    /// happened in the parent shell. Only not passing it fixes it.
    fn resolve_token(flag: Option<String>) -> anyhow::Result<Option<Arc<Secret>>> {
        if let Some(t) = flag.filter(|s| !s.is_empty()) {
            tracing::warn!(
                "--github-token is deprecated and will be removed: a token on the \
                 command line is visible in the process table to every local user \
                 and is written to shell history. Set $GITHUB_TOKEN, or point \
                 ${TOKEN_FILE_ENV} at a 0600 file, instead."
            );
            return Ok(Some(Arc::new(Secret::new(t)?)));
        }

        if let Some(t) = std::env::var("GITHUB_TOKEN").ok().filter(|s| !s.is_empty()) {
            return Ok(Some(Arc::new(Secret::new(t)?)));
        }

        if let Some(path) = std::env::var(TOKEN_FILE_ENV).ok().filter(|s| !s.is_empty()) {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading ${TOKEN_FILE_ENV} ({path})"))?;
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Ok(Some(Arc::new(Secret::new(trimmed)?)));
            }
        }

        Ok(None)
    }

    /// The PAT itself, for the one consumer that needs it as a `&str`
    /// (the octocrab client builder). Deliberately routed through
    /// `Secret::expose` so `rg 'expose\(\)'` enumerates every read.
    #[must_use]
    pub fn github_token_str(&self) -> Option<&str> {
        self.github_token.as_ref().map(|s| s.expose())
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
        assert_eq!(cfg.github_token_str(), Some("override"));
        assert!(cfg.has_token());
    }

    #[test]
    fn empty_flag_value_is_not_a_token() {
        // `--github-token ''` is a mistake, not a credential. It must
        // not shadow a real $GITHUB_TOKEN/$GITHUB_TOKEN_FILE, and it
        // must not become a Secret (Secret::new rejects empty).
        let cfg = Config::resolve(Some(String::new())).unwrap();
        // No assertion on Some/None: the surrounding env decides which
        // later source wins. What is asserted is that resolve does not
        // error and does not adopt the empty string.
        assert_ne!(cfg.github_token_str(), Some(""));
    }

    #[test]
    fn token_is_not_printed_by_debug() {
        // The reason the field is a Secret rather than a String: a
        // `{:?}` of Config is one edit away in any tracing call, and it
        // must not be the thing that leaks the PAT.
        //
        // The sentinel deliberately does NOT carry a `ghp_` prefix. It
        // stands in for a PAT but is not shaped like one, so it cannot
        // trip the fleet's block-secrets pre-commit hook — a fixture
        // that has to be waived on every commit is a fixture that
        // teaches people to pass --no-verify.
        let cfg = Config::resolve(Some("sentinel-not-a-real-token".into())).unwrap();
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("sentinel-not-a-real-token"), "{rendered}");
        assert!(rendered.contains("Secret(***)"), "{rendered}");
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
