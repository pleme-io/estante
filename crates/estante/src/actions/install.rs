//! `estante install` — read an existing lockfile, ensure every entry
//! is materialized in the cache, refresh missing hashes.

use std::path::Path;

use estante_types::Source;

use crate::cache;
use crate::config::Config;
use crate::fetch;
use crate::lockfile_io;

pub async fn run(lockfile_path: &Path, cfg: &Config) -> anyhow::Result<()> {
    let mut lock = lockfile_io::read(lockfile_path)?;
    if lock.entries.is_empty() {
        anyhow::bail!(
            "lockfile at {} is empty — run `estante lock` first",
            lockfile_path.display()
        );
    }
    cache::ensure_layout(cfg)?;
    let client = fetch::build_client(cfg.github_token_str())?;
    let mut refreshed = 0_usize;
    for entry in &mut lock.entries {
        let dest = cfg.store_path(&entry.name, &entry.rev);
        if cache::is_unpacked_pkg(&dest) {
            entry.materialized_path = dest.to_string_lossy().into_owned();
            continue;
        }
        let source = Source::parse(&entry.source)?;
        let report = fetch::download_and_unpack(&client, &source, &entry.rev, &dest).await?;
        entry.materialized_path = dest.to_string_lossy().into_owned();
        if entry.blake3.is_empty() {
            entry.blake3 = report.blake3;
        }
        refreshed += 1;
    }
    lockfile_io::write(lockfile_path, &lock)?;
    println!(
        "Installed {} package(s) ({} fetched, {} cache hit)",
        lock.entries.len(),
        refreshed,
        lock.entries.len() - refreshed
    );
    Ok(())
}
