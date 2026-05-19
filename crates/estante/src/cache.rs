//! Cache-directory helpers.
//!
//! Layout (relative to `Config::cache_dir`):
//!
//! ```text
//! $XDG_CACHE_HOME/estante/
//!   store/                       — unpacked package trees
//!     <name>-<short-rev>/        — one per (name, rev)
//! ```

use std::path::Path;

use crate::config::Config;

/// Ensure the cache layout exists. Idempotent.
pub fn ensure_layout(cfg: &Config) -> anyhow::Result<()> {
    std::fs::create_dir_all(cfg.cache_dir.join("store"))?;
    Ok(())
}

/// True if a path looks like an unpacked package (i.e. it exists and
/// contains a `rc.lisp` at the root). Used by the resolver's
/// "skip-already-unpacked" path.
#[must_use]
pub fn is_unpacked_pkg(path: &Path) -> bool {
    path.is_dir() && path.join("rc.lisp").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ensure_layout_creates_dirs() {
        let tmp = std::env::temp_dir().join(format!("estante-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let cfg = Config {
            cache_dir: tmp.clone(),
            github_token: None,
        };
        ensure_layout(&cfg).unwrap();
        assert!(tmp.join("store").is_dir());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn unpacked_predicate_requires_rc_lisp() {
        let tmp = std::env::temp_dir().join(format!("estante-unpacked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(!is_unpacked_pkg(&tmp));
        std::fs::write(tmp.join("rc.lisp"), "").unwrap();
        assert!(is_unpacked_pkg(&tmp));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn cache_path_buf_is_unused_warning_proof() {
        // Touch PathBuf import so the symbol is referenced when tests
        // are excluded from the build (silences unused-import warning
        // on the test module's `use std::path::PathBuf;`).
        let _: PathBuf = PathBuf::from("/dev/null");
    }

    #[test]
    fn unpacked_predicate_false_when_rc_lisp_is_directory() {
        // Edge case: `rc.lisp` exists but as a directory, not a file.
        // Should NOT be treated as a valid unpacked package.
        let tmp = std::env::temp_dir().join(format!("estante-unpacked-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("rc.lisp")).unwrap();
        assert!(!is_unpacked_pkg(&tmp));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn unpacked_predicate_false_for_nonexistent_path() {
        let p = std::env::temp_dir().join(format!(
            "estante-unpacked-noexist-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        assert!(!is_unpacked_pkg(&p));
    }

    #[test]
    fn ensure_layout_idempotent() {
        let tmp = std::env::temp_dir().join(format!(
            "estante-cache-idem-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let cfg = Config {
            cache_dir: tmp.clone(),
            github_token: None,
        };
        ensure_layout(&cfg).unwrap();
        ensure_layout(&cfg).unwrap();
        ensure_layout(&cfg).unwrap();
        assert!(tmp.join("store").is_dir());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
