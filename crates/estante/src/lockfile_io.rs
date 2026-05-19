//! `shellpkg.lock.lisp` read/write.

use std::path::Path;

use estante_types::Lockfile;

pub fn read(path: &Path) -> anyhow::Result<Lockfile> {
    if !path.exists() {
        return Ok(Lockfile::default());
    }
    let src = std::fs::read_to_string(path)?;
    Ok(Lockfile::parse(&src)?)
}

/// Atomic write — same rationale as `manifest_io::write`. Validates
/// materialization before emit so a half-resolved lockfile never
/// lands on disk.
pub fn write(path: &Path, l: &Lockfile) -> anyhow::Result<()> {
    l.validate_materialized()?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("lockfile path has no parent directory: {}", path.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    write!(tmp, "{l}")?;
    tmp.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use estante_types::LockedPkgSpec;

    #[test]
    fn read_missing_lockfile_returns_empty() {
        let p = std::env::temp_dir().join(format!("estante-lock-no-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let l = read(&p).unwrap();
        assert!(l.entries.is_empty());
    }

    #[test]
    fn write_validates_materialized_path() {
        let tmp = std::env::temp_dir().join(format!("estante-lock-val-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("shellpkg.lock.lisp");
        let mut l = Lockfile::default();
        l.upsert(LockedPkgSpec {
            name: "foo".into(),
            source: "github:org/foo".into(),
            rev: "abc".into(),
            nar_hash: "sha256-x".into(),
            blake3: "blake3-y".into(),
            materialized_path: String::new(), // ← empty — write should reject
        });
        let err = write(&path, &l).unwrap_err();
        assert!(err.to_string().contains("materialized-path"));
        // Now fix the path and write succeeds.
        l.entries[0].materialized_path = "/nix/store/abc-foo/".into();
        write(&path, &l).unwrap();
        let re = read(&path).unwrap();
        assert_eq!(re, l);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
