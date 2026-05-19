//! Deterministic content-address for an unpacked package tree.
//!
//! `blake3_tree(root)` walks `root` in **sorted order** and returns
//! a hex BLAKE3 digest. The hash is canonical: two trees with the
//! same file paths + contents produce the same digest, regardless of
//! filesystem ordering, mtime, inode number, or whether the tree
//! arrived via tarball or local copy.
//!
//! This is the verifiability anchor for the lockfile — anyone with
//! the same source can re-derive the same digest, and any drift in a
//! single byte produces a different digest.
//!
//! The encoding folds three pieces per file into the hash:
//!
//! ```text
//! H ← BLAKE3-init
//! for each file in sorted-relative-path order:
//!     H.update(relative-path-bytes)
//!     H.update(b"\0")                  // domain separator
//!     H.update(content-length as u64 LE)
//!     H.update(file-bytes)
//! ```
//!
//! Length-prefixing prevents collisions between `["foo", "bar"]` and
//! `["foobar"]`-shaped concatenations. Symbolic links are followed;
//! empty directories are invisible (deliberate — frost-lisp doesn't
//! care about empty dirs and ignoring them keeps the digest stable
//! across tar implementations that vary in directory-entry handling).

use std::path::{Path, PathBuf};

/// Compute the canonical content-address of `root`.
pub fn blake3_tree(root: &Path) -> std::io::Result<String> {
    let mut entries: Vec<PathBuf> = Vec::new();
    collect_files(root, root, &mut entries)?;
    entries.sort();

    let mut hasher = blake3::Hasher::new();
    for relative in &entries {
        // Path bytes — use a stable encoding (UTF-8 lossy) so paths
        // hash identically across filesystems with weird encoding.
        let relative_str = relative.to_string_lossy();
        hasher.update(relative_str.as_bytes());
        hasher.update(b"\0");

        let absolute = root.join(relative);
        let content = std::fs::read(&absolute)?;
        let len = u64::try_from(content.len()).unwrap_or(u64::MAX);
        hasher.update(&len.to_le_bytes());
        hasher.update(&content);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn collect_files(root: &Path, cur: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(cur)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            // Symlinks fall through to the file branch — we want the
            // target's contents in the hash, not the link metadata.
            let relative = path
                .strip_prefix(root)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            out.push(relative.to_path_buf());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tree(root: &Path, files: &[(&str, &[u8])]) {
        for (rel, content) in files {
            let p = root.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&p, content).unwrap();
        }
    }

    #[test]
    fn same_tree_same_hash() {
        let tmp = std::env::temp_dir().join(format!("estante-hash-eq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let a = tmp.join("a");
        let b = tmp.join("b");
        make_tree(&a, &[("rc.lisp", b"alpha"), ("sub/x.lisp", b"beta")]);
        make_tree(&b, &[("rc.lisp", b"alpha"), ("sub/x.lisp", b"beta")]);
        let h1 = blake3_tree(&a).unwrap();
        let h2 = blake3_tree(&b).unwrap();
        assert_eq!(h1, h2);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn one_byte_difference_changes_hash() {
        let tmp = std::env::temp_dir().join(format!("estante-hash-diff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let a = tmp.join("a");
        let b = tmp.join("b");
        make_tree(&a, &[("rc.lisp", b"alpha")]);
        make_tree(&b, &[("rc.lisp", b"alpha!")]);
        let h1 = blake3_tree(&a).unwrap();
        let h2 = blake3_tree(&b).unwrap();
        assert_ne!(h1, h2);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn path_relocation_changes_hash() {
        // foo/bar vs foo-bar should produce different digests — the
        // path encoding participates in the hash.
        let tmp = std::env::temp_dir().join(format!("estante-hash-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let a = tmp.join("a");
        let b = tmp.join("b");
        make_tree(&a, &[("foo/bar.lisp", b"x")]);
        make_tree(&b, &[("foo-bar.lisp", b"x")]);
        let h1 = blake3_tree(&a).unwrap();
        let h2 = blake3_tree(&b).unwrap();
        assert_ne!(h1, h2);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn hash_idempotent_across_repeated_calls() {
        let tmp = std::env::temp_dir().join(format!("estante-hash-idem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        make_tree(&tmp, &[("rc.lisp", b"data"), ("a/b/c.lisp", b"more")]);
        let h1 = blake3_tree(&tmp).unwrap();
        let h2 = blake3_tree(&tmp).unwrap();
        let h3 = blake3_tree(&tmp).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h2, h3);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn binary_content_hashes_consistently() {
        // Non-UTF-8 bytes (a tarball, an image, anything) must hash
        // the same as their text equivalent. blake3 doesn't care about
        // encoding; this test just guards against an accidental
        // String-based path in the hasher.
        let tmp = std::env::temp_dir().join(format!("estante-hash-bin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let binary: Vec<u8> = (0..=255u8).collect(); // every byte value
        make_tree(&tmp, &[("blob.bin", &binary)]);
        let h1 = blake3_tree(&tmp).unwrap();
        let h2 = blake3_tree(&tmp).unwrap();
        assert_eq!(h1, h2);
        // Modify ONE byte → digest changes.
        let mut binary2 = binary;
        binary2[127] ^= 1;
        make_tree(&tmp, &[("blob.bin", &binary2)]);
        let h3 = blake3_tree(&tmp).unwrap();
        assert_ne!(h1, h3);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn deeply_nested_tree_hashes_deterministically() {
        let tmp = std::env::temp_dir().join(format!("estante-hash-deep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        make_tree(
            &tmp,
            &[
                ("a/b/c/d/e/leaf.lisp", b"deep-content"),
                ("a/b/c/sibling.lisp", b"sibling"),
                ("top.lisp", b"shallow"),
            ],
        );
        let h1 = blake3_tree(&tmp).unwrap();
        let h2 = blake3_tree(&tmp).unwrap();
        assert_eq!(h1, h2);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn file_order_doesnt_matter_for_hash() {
        // Two equivalent trees written in different fs-creation order
        // hash identically — the sort step in blake3_tree neutralizes
        // any directory-listing entropy.
        let tmp = std::env::temp_dir().join(format!("estante-hash-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let a = tmp.join("a");
        let b = tmp.join("b");
        // Create in different orders.
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(a.join("z.lisp"), b"z").unwrap();
        std::fs::write(a.join("a.lisp"), b"a").unwrap();
        std::fs::write(a.join("m.lisp"), b"m").unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("a.lisp"), b"a").unwrap();
        std::fs::write(b.join("m.lisp"), b"m").unwrap();
        std::fs::write(b.join("z.lisp"), b"z").unwrap();
        let h1 = blake3_tree(&a).unwrap();
        let h2 = blake3_tree(&b).unwrap();
        assert_eq!(h1, h2);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn empty_tree_has_a_stable_hash() {
        // An empty directory should still hash deterministically —
        // the all-zero start state of BLAKE3 produces a known value.
        let tmp = std::env::temp_dir().join(format!("estante-hash-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let h1 = blake3_tree(&tmp).unwrap();
        let h2 = blake3_tree(&tmp).unwrap();
        assert_eq!(h1, h2);
        // The empty-tree digest is BLAKE3 of zero bytes — its hex
        // form is a stable constant; spot-check the prefix.
        assert!(h1.starts_with("af1349"));
        std::fs::remove_dir_all(&tmp).ok();
    }
}
