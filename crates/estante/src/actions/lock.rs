//! `estante lock` — resolve the manifest into a deterministic
//! lockfile. v0.1 also fetches + unpacks each package; `install` is
//! an alias today and may diverge later (fetch-only vs. resolve-only).

use std::path::Path;

use crate::config::Config;
use crate::lockfile_io;
use crate::manifest_io;
use crate::resolver::Resolver;

pub async fn run(manifest_path: &Path, lockfile_path: &Path, cfg: &Config) -> anyhow::Result<()> {
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
    lockfile_io::write(lockfile_path, &lock)?;
    println!(
        "Locked {} package(s) → {}",
        lock.entries.len(),
        lockfile_path.display()
    );
    Ok(())
}
