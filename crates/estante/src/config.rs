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

/// Env var holding the GitHub PAT. Already the name this crate has
/// always honored, so no second spelling is invented for it.
pub const TOKEN_ENV: &str = "GITHUB_TOKEN";

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
    /// Resolve config from env vars + XDG paths. Takes the value of the
    /// deprecated `--github-token` flag, which is honored only when no
    /// safer source supplied one — see [`Config::select_token`].
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

    /// Read every token source, then hand them to [`Config::select_token`]
    /// for the precedence decision. Split this way so the precedence rule
    /// is testable without mutating the process environment — which this
    /// crate cannot do anyway, being `#![forbid(unsafe_code)]`.
    fn resolve_token(flag: Option<String>) -> anyhow::Result<Option<Arc<Secret>>> {
        // An empty `--github-token ''` is a mistake, not a credential:
        // `Secret::new` would reject it, and it must not be treated as
        // "the operator supplied a token" either.
        let flag = flag.filter(|s| !s.is_empty()).map(Secret::new).transpose()?;
        let env_token = Secret::from_env(TOKEN_ENV).ok();
        let file_token = Self::token_from_file()?;
        Ok(Self::select_token(flag, env_token, file_token))
    }

    /// Read the token out of the file named by `$GITHUB_TOKEN_FILE`.
    /// A missing/empty var means "not configured"; a named file that
    /// cannot be read is an error, because silently continuing
    /// unauthenticated would be indistinguishable from success.
    fn token_from_file() -> anyhow::Result<Option<Secret>> {
        let Some(path) = std::env::var(TOKEN_FILE_ENV).ok().filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading ${TOKEN_FILE_ENV} ({path})"))?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Ok(Some(Secret::new(trimmed)?))
    }

    /// Token precedence: `$GITHUB_TOKEN`, then the file named by
    /// `$GITHUB_TOKEN_FILE`, and only then the deprecated
    /// `--github-token` flag.
    ///
    /// The flag is kept working — removing it would break callers — but
    /// it is the LAST resort rather than the first, because a value
    /// passed on the command line is readable from the process table by
    /// any local user for the lifetime of the process, and is recorded
    /// verbatim in the operator's shell history. `Secret` cannot fix
    /// that: by the time estante is running, the exposure has already
    /// happened in the parent shell. Only not passing it fixes it.
    ///
    /// Putting the safe sources first is what makes migration
    /// one-directional: an operator who exports `$GITHUB_TOKEN` is not
    /// silently dragged back onto the flag by a stale wrapper script
    /// that still passes it.
    fn select_token(
        flag: Option<Secret>,
        env_token: Option<Secret>,
        file_token: Option<Secret>,
    ) -> Option<Arc<Secret>> {
        if flag.is_some() {
            if env_token.is_some() || file_token.is_some() {
                tracing::warn!(
                    "--github-token is deprecated and IGNORED here: it is visible in \
                     the process table to every local user and is written to shell \
                     history. Using ${TOKEN_ENV}/${TOKEN_FILE_ENV} instead; drop the flag."
                );
            } else {
                tracing::warn!(
                    "--github-token is deprecated: it is visible in the process table \
                     to every local user and is written to shell history. Set \
                     ${TOKEN_ENV}, or point ${TOKEN_FILE_ENV} at a 0600 file, instead."
                );
            }
        }
        env_token.or(file_token).or(flag).map(Arc::new)
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

    /// `select_token` is the whole precedence rule, over already-read
    /// sources, so these assertions do not depend on — and cannot be
    /// broken by — whatever the ambient environment happens to hold.
    fn picked(
        flag: Option<&str>,
        env_token: Option<&str>,
        file_token: Option<&str>,
    ) -> Option<String> {
        let mk = |v: Option<&str>| v.map(|v| Secret::new(v).unwrap());
        Config::select_token(mk(flag), mk(env_token), mk(file_token))
            .map(|s| s.expose().to_owned())
    }

    #[test]
    fn env_token_takes_precedence_over_flag() {
        // The point of the deprecation: an operator who has already
        // moved to $GITHUB_TOKEN is not dragged back onto the flag by a
        // stale wrapper script that still passes it.
        assert_eq!(
            picked(Some("from-flag"), Some("from-env"), None).as_deref(),
            Some("from-env")
        );
    }

    #[test]
    fn token_file_takes_precedence_over_flag() {
        assert_eq!(
            picked(Some("from-flag"), None, Some("from-file")).as_deref(),
            Some("from-file")
        );
    }

    #[test]
    fn env_token_takes_precedence_over_token_file() {
        assert_eq!(
            picked(None, Some("from-env"), Some("from-file")).as_deref(),
            Some("from-env")
        );
    }

    #[test]
    fn flag_still_works_when_it_is_the_only_source() {
        // Deprecate, do not remove: with no safer source configured the
        // flag is still honored exactly as it always was.
        assert_eq!(
            picked(Some("from-flag"), None, None).as_deref(),
            Some("from-flag")
        );
    }

    #[test]
    fn no_source_is_unauthenticated() {
        assert_eq!(picked(None, None, None), None);
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
