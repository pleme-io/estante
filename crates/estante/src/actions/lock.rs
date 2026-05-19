//! `estante lock` — resolve the manifest into a deterministic
//! lockfile. v0.1 also fetches + unpacks each package; `install` is
//! an alias today and may diverge later (fetch-only vs. resolve-only).
//!
//! `--check` mode: re-resolve in memory and compare against the
//! existing on-disk lockfile, exiting non-zero with a diff if they
//! disagree. This is the CI gate that enforces "the committed
//! lockfile reflects the committed manifest" — drift here means a
//! manifest edit was merged without rerunning `estante lock`.

use std::path::Path;

use crate::config::Config;
use crate::lockfile_io;
use crate::manifest_io;
use crate::resolver::Resolver;

pub async fn run(manifest_path: &Path, lockfile_path: &Path, cfg: &Config) -> anyhow::Result<()> {
    run_with_opts(manifest_path, lockfile_path, cfg, false).await
}

pub async fn run_with_opts(
    manifest_path: &Path,
    lockfile_path: &Path,
    cfg: &Config,
    check: bool,
) -> anyhow::Result<()> {
    let manifest = manifest_io::read(manifest_path)?;
    if manifest.packages.is_empty() {
        anyhow::bail!(
            "manifest at {} has no packages — run `estante add <source>` first",
            manifest_path.display()
        );
    }
    let mut resolver = Resolver::new(cfg)?;
    if let Some(parent) = manifest_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        // Canonicalize so symlinked / `./` paths resolve consistently.
        let base = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        resolver = resolver.with_base_dir(base);
    }
    let lock = resolver.resolve(&manifest).await?;

    if check {
        let new_bytes = lock.to_string();
        let old_bytes = std::fs::read_to_string(lockfile_path).unwrap_or_default();
        if new_bytes == old_bytes {
            println!(
                "lockfile up to date ({} package(s), {} bytes)",
                lock.entries.len(),
                new_bytes.len(),
            );
            return Ok(());
        }
        eprintln!(
            "\x1b[31m✗\x1b[0m lockfile out of date — `estante lock` would rewrite {}",
            lockfile_path.display(),
        );
        eprintln!("  on-disk : {} bytes", old_bytes.len());
        eprintln!("  resolved: {} bytes", new_bytes.len());
        eprintln!(
            "  blake3 on-disk : {}",
            blake3::hash(old_bytes.as_bytes()).to_hex(),
        );
        eprintln!(
            "  blake3 resolved: {}",
            blake3::hash(new_bytes.as_bytes()).to_hex(),
        );
        anyhow::bail!("lockfile drift detected — run `estante lock` and commit the change");
    }

    lockfile_io::write(lockfile_path, &lock)?;
    println!(
        "Locked {} package(s) → {}",
        lock.entries.len(),
        lockfile_path.display()
    );
    Ok(())
}
