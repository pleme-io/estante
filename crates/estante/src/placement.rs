//! Placement resolver — moves a package's bytes into the chosen substrate.
//!
//! Two substrates today:
//!   - `cache` — `$XDG_CACHE_HOME/estante/store/<name>-<rev>/`. The
//!     resolver's default. Fast, mutable, user-local.
//!   - `nix`   — `/nix/store/<hash>-<name>-<rev>/`. Immutable,
//!     content-addressed via `nix store add-path`. Required for
//!     home-manager / fleet deploys.
//!
//! Shift logic: `migrate_in_place` reads a `LockedPkgSpec` and a
//! desired target, returns a new spec with the right
//! `materialized_path` + `nar_hash` + `placement` fields. The
//! caller (CLI / config-driven migration) persists the result via
//! the standard lockfile_io path.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use estante_types::{LockedPkgSpec, Placement};

/// Promote a cache-placed package into the Nix store. Idempotent: if
/// the package is already nix-placed (or both), this is a no-op.
pub async fn place_in_nix(entry: &LockedPkgSpec) -> anyhow::Result<LockedPkgSpec> {
    let current = Placement::from_str(&entry.placement);
    if matches!(current, Placement::Nix) {
        return Ok(entry.clone());
    }

    let cache_path = PathBuf::from(&entry.materialized_path);
    if !cache_path.exists() {
        bail!(
            "cannot move {} into nix: source path missing — run `estante install` first",
            entry.name
        );
    }

    let (store_path, nar_hash) = nix_add_path(&cache_path).await?;

    Ok(LockedPkgSpec {
        materialized_path: store_path,
        nar_hash,
        placement: Placement::Nix.as_str().to_owned(),
        ..entry.clone()
    })
}

/// Promote a nix-placed package into the user-local cache. The
/// resolver re-fetches via the standard pipeline (caller is
/// responsible for that part); this helper just rewrites the
/// placement field.
pub fn mark_as_cache(entry: &LockedPkgSpec, cache_path: &Path) -> LockedPkgSpec {
    LockedPkgSpec {
        materialized_path: cache_path.to_string_lossy().into_owned(),
        // narHash is empty for cache placement — only meaningful in nix.
        nar_hash: String::new(),
        placement: Placement::Cache.as_str().to_owned(),
        ..entry.clone()
    }
}

/// Mark an entry as present in BOTH stores. `materialized_path`
/// stays pointing at the primary (currently set) location; the
/// other location is recoverable from the rev + cache layout.
pub fn mark_as_both(entry: &LockedPkgSpec) -> LockedPkgSpec {
    LockedPkgSpec {
        placement: Placement::Both.as_str().to_owned(),
        ..entry.clone()
    }
}

/// Add a directory to the local Nix store and return `(store_path,
/// nar_hash)`. Requires `nix` on PATH.
async fn nix_add_path(dir: &Path) -> anyhow::Result<(String, String)> {
    if !nix_available().await {
        bail!("nix not on PATH — cannot promote to nix placement. Install nix or use --placement cache");
    }

    let add_output = tokio::process::Command::new("nix")
        .args(["store", "add-path"])
        .arg(dir)
        .output()
        .await
        .context("invoking `nix store add-path`")?;
    if !add_output.status.success() {
        bail!(
            "nix store add-path failed (exit {}): {}",
            add_output.status,
            String::from_utf8_lossy(&add_output.stderr),
        );
    }
    let store_path = String::from_utf8(add_output.stdout)?.trim().to_owned();
    if store_path.is_empty() {
        bail!("nix store add-path returned empty stdout");
    }

    // Query the NAR hash. nix-store has been the stable interface
    // for decades; the newer `nix store` namespace works too but
    // we use the legacy form for broadest compatibility.
    let hash_output = tokio::process::Command::new("nix-store")
        .args(["--query", "--hash", &store_path])
        .output()
        .await
        .context("invoking `nix-store --query --hash`")?;
    if !hash_output.status.success() {
        return Err(anyhow!(
            "nix-store --query --hash failed for {store_path}: {}",
            String::from_utf8_lossy(&hash_output.stderr)
        ));
    }
    let nar_hash = String::from_utf8(hash_output.stdout)?.trim().to_owned();

    Ok((store_path, nar_hash))
}

async fn nix_available() -> bool {
    tokio::process::Command::new("nix")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_as_cache_clears_nar_hash() {
        let entry = LockedPkgSpec {
            name: "foo".into(),
            source: "github:x/foo".into(),
            rev: "abc".into(),
            nar_hash: "sha256-old".into(),
            blake3: "blake3-yyy".into(),
            materialized_path: "/nix/store/abc-foo".into(),
            placement: "nix".into(),
        };
        let cached = mark_as_cache(&entry, std::path::Path::new("/tmp/cache/foo"));
        assert_eq!(cached.placement, "cache");
        assert_eq!(cached.materialized_path, "/tmp/cache/foo");
        assert!(cached.nar_hash.is_empty());
        // Unchanged metadata.
        assert_eq!(cached.rev, "abc");
        assert_eq!(cached.blake3, "blake3-yyy");
    }

    #[test]
    fn mark_as_both_only_flips_placement() {
        let entry = LockedPkgSpec {
            name: "foo".into(),
            source: "github:x/foo".into(),
            rev: "abc".into(),
            nar_hash: "sha256-x".into(),
            blake3: "blake3-y".into(),
            materialized_path: "/nix/store/abc-foo".into(),
            placement: "nix".into(),
        };
        let both = mark_as_both(&entry);
        assert_eq!(both.placement, "both");
        assert_eq!(both.materialized_path, entry.materialized_path);
        assert_eq!(both.nar_hash, entry.nar_hash);
    }

    #[test]
    fn mark_as_cache_preserves_metadata() {
        let entry = LockedPkgSpec {
            name: "alpha".into(),
            source: "github:x/alpha@v1".into(),
            rev: "abc123".into(),
            nar_hash: "sha256-original".into(),
            blake3: "blake3-content".into(),
            materialized_path: "/nix/store/old".into(),
            placement: "nix".into(),
        };
        let cached = mark_as_cache(&entry, std::path::Path::new("/new/path"));
        assert_eq!(cached.name, entry.name);
        assert_eq!(cached.source, entry.source);
        assert_eq!(cached.rev, entry.rev);
        assert_eq!(cached.blake3, entry.blake3);
    }

    #[tokio::test]
    async fn place_in_nix_no_op_when_already_nix() {
        let entry = LockedPkgSpec {
            name: "foo".into(),
            source: "github:x/foo".into(),
            rev: "abc".into(),
            nar_hash: "sha256-x".into(),
            blake3: "b3".into(),
            materialized_path: "/nix/store/abc-foo".into(),
            placement: "nix".into(),
        };
        // No-op path: must not invoke nix.
        let result = place_in_nix(&entry).await.unwrap();
        assert_eq!(result, entry);
    }

    #[tokio::test]
    async fn place_in_nix_errors_when_source_path_missing() {
        let entry = LockedPkgSpec {
            name: "foo".into(),
            source: "github:x/foo".into(),
            rev: "abc".into(),
            nar_hash: String::new(),
            blake3: "b3".into(),
            materialized_path: "/this/path/does/not/exist".into(),
            placement: "cache".into(),
        };
        let err = place_in_nix(&entry).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("source path missing") || msg.contains("run `estante install`"),
            "unexpected error: {msg}"
        );
    }
}
