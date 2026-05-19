//! `shellpkg.lisp` read/write.

use std::path::Path;

use estante_types::Manifest;

/// Read a manifest from disk. Empty/missing → empty manifest.
pub fn read(path: &Path) -> anyhow::Result<Manifest> {
    if !path.exists() {
        return Ok(Manifest::default());
    }
    let src = std::fs::read_to_string(path)?;
    Ok(Manifest::parse(&src)?)
}

/// Atomic write: write to a sibling temp file, then rename. A crash
/// during emit leaves either the old file intact or the new one
/// fully-formed — never a half-written truncation.
pub fn write(path: &Path, m: &Manifest) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("manifest path has no parent directory: {}", path.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    // Manifest's Display impl is the typed-emission surface for the
    // shellpkg.lisp grammar — write through the Display block, never
    // hand-rolled string concatenation.
    use std::io::Write;
    write!(tmp, "{m}")?;
    tmp.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use estante_types::PkgSpec;

    #[test]
    fn read_missing_file_returns_empty() {
        let p = std::env::temp_dir().join(format!("estante-manifest-no-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let m = read(&p).unwrap();
        assert!(m.packages.is_empty());
    }

    #[test]
    fn write_atomic_does_not_leave_partial_file_on_existing_path() {
        // Pre-populate the path with one manifest; then write a
        // different manifest. The result is the SECOND manifest's
        // contents (atomic replacement), never a half-merged blob.
        let tmp = std::env::temp_dir().join(format!("estante-manifest-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("shellpkg.lisp");
        let mut m1 = Manifest::default();
        m1.upsert(PkgSpec {
            name: "first".into(),
            version: "1".into(),
            source: "github:x/first".into(),
            exports: vec![],
            deps: vec![],
            lazy: false,
        });
        write(&path, &m1).unwrap();
        let mut m2 = Manifest::default();
        m2.upsert(PkgSpec {
            name: "second".into(),
            version: "2".into(),
            source: "github:x/second".into(),
            exports: vec![],
            deps: vec![],
            lazy: false,
        });
        write(&path, &m2).unwrap();
        let re = read(&path).unwrap();
        assert_eq!(re, m2, "second write should replace first cleanly");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn write_then_read_round_trips() {
        let tmp = std::env::temp_dir().join(format!("estante-manifest-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("shellpkg.lisp");
        let mut m = Manifest::default();
        m.upsert(PkgSpec {
            name: "foo".into(),
            version: "1.0".into(),
            source: "github:org/foo".into(),
            exports: vec!["alias".into()],
            deps: vec![],
            lazy: false,
        });
        write(&path, &m).unwrap();
        let read_back = read(&path).unwrap();
        assert_eq!(read_back, m);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
