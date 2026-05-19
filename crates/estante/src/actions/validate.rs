//! `estante validate` — parse the manifest + lockfile without
//! touching the network. Catches typos in `:source` / `:exports` /
//! duplicate names and confirms every locked entry carries a
//! materialized path that exists on disk.

use std::path::Path;

use estante_types::Source;

use crate::lockfile_io;
use crate::manifest_io;

pub async fn run(manifest_path: &Path, lockfile_path: &Path) -> anyhow::Result<()> {
    let manifest = manifest_io::read(manifest_path)?;
    let lock = lockfile_io::read(lockfile_path)?;
    // Manifest parse already enforced duplicate-name rejection.
    for pkg in &manifest.packages {
        let _ = Source::parse(&pkg.source)?;
    }
    // Lockfile entries must each have a materialized path that exists.
    for entry in &lock.entries {
        let p = std::path::Path::new(&entry.materialized_path);
        if !p.exists() {
            anyhow::bail!(
                "{}: materialized-path {} does not exist (run `estante install`)",
                entry.name,
                entry.materialized_path
            );
        }
    }
    println!(
        "OK — {} package(s), {} lock entries.",
        manifest.packages.len(),
        lock.entries.len()
    );
    Ok(())
}
