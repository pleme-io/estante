//! Network fetch — GitHub tarballs via octocrab.
//!
//! For v0.1 we accept only GitHub-hosted sources (Github + Gist
//! variants of [`estante_types::Source`]). gix-backed git+https /
//! git+ssh transports land in v0.2.
//!
//! All octocrab calls SHOULD be routed through the samba broker so
//! estante shares the GitHub rate-limit budget with `tend`. samba
//! integration is M1d; the function signatures here are stable so the
//! switch is purely the constructor of the underlying client.

use std::path::Path;

use anyhow::{Context, anyhow};
use estante_types::Source;
use http_body_util::BodyExt;
use octocrab::Octocrab;

/// One resolved GitHub-side fact about a package — what we learned by
/// asking octocrab. Returned by [`resolve_rev`] and consumed by the
/// resolver to populate `LockedPkgSpec::rev`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRev {
    /// 40-character commit SHA.
    pub sha: String,
}

/// Build an octocrab client honoring the optional PAT. Unauthenticated
/// clients still work — useful for CI on public repos until a token is
/// available.
pub fn build_client(token: Option<&str>) -> anyhow::Result<Octocrab> {
    let mut builder = Octocrab::builder();
    if let Some(t) = token {
        builder = builder.personal_token(t.to_owned());
    }
    builder.build().context("octocrab build failed")
}

/// Resolve a source's reference (tag/branch/SHA) to a 40-char commit
/// SHA. Returns the SHA verbatim if the source's reference is already
/// 40 hex chars.
pub async fn resolve_rev(client: &Octocrab, source: &Source) -> anyhow::Result<ResolvedRev> {
    match source {
        Source::Github {
            owner,
            repo,
            reference,
        } => {
            if is_full_sha(reference) {
                return Ok(ResolvedRev {
                    sha: reference.clone(),
                });
            }
            // GitHub's commits endpoint accepts a ref (tag/branch/SHA)
            // and returns the canonical commit. Octocrab's typed
            // wrapper exposes this as `repos(owner, repo).list_commits()
            // .sha(ref)`. We just want the first match.
            let commits = client
                .repos(owner, repo)
                .list_commits()
                .sha(reference.as_str())
                .per_page(1)
                .send()
                .await
                .with_context(|| {
                    format!("github API: list commits for {owner}/{repo}@{reference}")
                })?;
            let commit = commits
                .items
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("no commit found for {owner}/{repo}@{reference}"))?;
            Ok(ResolvedRev { sha: commit.sha })
        }
        Source::Gist { id, reference } => {
            // Gists don't have a list-commits endpoint with a ref
            // filter; octocrab exposes the gist directly. v0.1 simply
            // pins the gist by ID; reference is preserved as-is unless
            // it's a full SHA.
            Ok(ResolvedRev {
                sha: if is_full_sha(reference) {
                    reference.clone()
                } else {
                    id.clone()
                },
            })
        }
        Source::Local { path } => {
            // BLAKE3 the absolute path so the rev is stable, 16
            // hex chars, and filesystem-safe — slots straight into
            // store_path without the `:` / `/` mangling a literal
            // path produces. Two local sources at the same path hash
            // to the same rev (intentional — cache hits work).
            let digest = blake3::hash(path.as_bytes()).to_hex();
            let short = digest.as_str()[..16].to_owned();
            Ok(ResolvedRev { sha: short })
        }
        Source::GitHttps { .. } | Source::GitSsh { .. } => Err(anyhow!(
            "git+https / git+ssh sources are v0.2 (gix integration); use github: for v0.1"
        )),
    }
}

/// Download + extract a tarball for the given source at the given SHA
/// into `dest_dir`. `dest_dir` is created if missing; existing
/// contents are NOT cleared (caller is expected to use a fresh path
/// per (name, rev)).
pub async fn download_and_unpack(
    client: &Octocrab,
    source: &Source,
    sha: &str,
    dest_dir: &Path,
) -> anyhow::Result<UnpackReport> {
    match source {
        Source::Github { owner, repo, .. } => {
            let resp = client
                ._get(format!(
                    "https://api.github.com/repos/{owner}/{repo}/tarball/{sha}"
                ))
                .await
                .with_context(|| format!("github API: tarball for {owner}/{repo}@{sha}"))?;
            let bytes = resp
                .into_body()
                .collect()
                .await
                .context("read tarball body")?
                .to_bytes();
            unpack_tarball(&bytes, dest_dir)
        }
        Source::Gist { id, .. } => {
            let resp = client
                ._get(format!("https://api.github.com/gists/{id}"))
                .await
                .context("github API: gist fetch")?;
            let bytes = resp
                .into_body()
                .collect()
                .await
                .context("read gist body")?
                .to_bytes();
            // Gists return JSON, not a tarball; v0.1 just dumps the
            // raw bytes to a single file. Real gist support lands in
            // v0.2 when we crack open the files map.
            std::fs::create_dir_all(dest_dir)?;
            std::fs::write(dest_dir.join("gist.json"), &bytes)?;
            Ok(UnpackReport {
                blake3: blake3::hash(&bytes).to_hex().to_string(),
                file_count: 1,
            })
        }
        Source::Local { path } => {
            // For local sources, recursively copy. Treats the source
            // path as the tree contents themselves — no archive
            // intermediary.
            let src = std::path::Path::new(path);
            if !src.exists() {
                return Err(anyhow!("local source path does not exist: {path}"));
            }
            let report = copy_dir_recursive(src, dest_dir)?;
            Ok(report)
        }
        Source::GitHttps { .. } | Source::GitSsh { .. } => Err(anyhow!(
            "git+https / git+ssh fetch is v0.2 (gix integration)"
        )),
    }
}

/// Result of an unpack operation. Feeds back into the lockfile.
#[derive(Debug, Clone)]
pub struct UnpackReport {
    /// BLAKE3 of the (gzipped) tarball bytes. Stable across machines
    /// for the same input. Empty string for the cache-hit short-circuit
    /// in `resolver.rs` (re-hash from disk is M1d).
    pub blake3: String,
    /// Number of files extracted. Logged at info level after each
    /// unpack so debug output shows progress.
    pub file_count: usize,
}

fn unpack_tarball(bytes: &[u8], dest_dir: &Path) -> anyhow::Result<UnpackReport> {
    std::fs::create_dir_all(dest_dir)?;
    // Hash the raw gzipped bytes once (this becomes the lockfile's
    // blake3 entry).
    let blake3_digest = blake3::hash(bytes).to_hex().to_string();

    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);

    // GitHub tarballs come wrapped in one top-level directory like
    // `owner-repo-<sha>/`. We strip that wrapper so `dest_dir/rc.lisp`
    // lives at the root of the unpacked tree (which is what
    // frost-lisp's `defload` expects).
    let mut file_count = 0_usize;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let mut components = path.components();
        let _wrapper = components.next(); // strip first component
        let rest: std::path::PathBuf = components.collect();
        if rest.as_os_str().is_empty() {
            continue;
        }
        let unpack_to = dest_dir.join(&rest);
        if let Some(parent) = unpack_to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry.unpack(&unpack_to)?;
        file_count += 1;
    }

    Ok(UnpackReport {
        blake3: blake3_digest,
        file_count,
    })
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<UnpackReport> {
    std::fs::create_dir_all(dst)?;
    let mut hasher = blake3::Hasher::new();
    let mut file_count = 0_usize;
    walk_and_copy(src, src, dst, &mut hasher, &mut file_count)?;
    Ok(UnpackReport {
        blake3: hasher.finalize().to_hex().to_string(),
        file_count,
    })
}

fn walk_and_copy(
    root: &Path,
    cur: &Path,
    dst: &Path,
    hasher: &mut blake3::Hasher,
    file_count: &mut usize,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(cur)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?.to_path_buf();
        let out = dst.join(&rel);
        if path.is_dir() {
            std::fs::create_dir_all(&out)?;
            walk_and_copy(root, &path, dst, hasher, file_count)?;
        } else {
            let bytes = std::fs::read(&path)?;
            hasher.update(&bytes);
            std::fs::write(&out, &bytes)?;
            *file_count += 1;
        }
    }
    Ok(())
}

fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_sha_detection() {
        assert!(is_full_sha("aa489f1d0bef818c4ec7d09b87a44d5cabaa9b6f"));
        assert!(!is_full_sha("aa489f1"));
        assert!(!is_full_sha("v1.7.4"));
        assert!(!is_full_sha("main"));
    }

    #[test]
    fn local_source_copy_round_trips() {
        let tmp = std::env::temp_dir().join(format!("estante-fetch-local-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        std::fs::create_dir_all(src.join("subdir")).unwrap();
        std::fs::write(src.join("rc.lisp"), "alpha").unwrap();
        std::fs::write(src.join("subdir/nested.lisp"), "beta").unwrap();
        let report = copy_dir_recursive(&src, &dst).unwrap();
        assert_eq!(report.file_count, 2);
        assert_eq!(
            std::fs::read_to_string(dst.join("rc.lisp")).unwrap(),
            "alpha"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("subdir/nested.lisp")).unwrap(),
            "beta"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }
}
