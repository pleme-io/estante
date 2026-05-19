//! `estante place <pkg> --to <cache|nix|both>` — shift a single
//! package's placement substrate.
//!
//! Reads the lockfile, locates the entry, calls the appropriate
//! placement helper, writes the lockfile back atomically. Idempotent:
//! shifting `nix → nix` is a no-op.
//!
//! Also handles bulk: `estante place --all --to nix` walks every
//! entry; partial failures are reported but don't abort the rest.

use std::path::Path;

use clap::ValueEnum;
use estante_types::Placement;

use crate::lockfile_io;
use crate::placement;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Target {
    Cache,
    Nix,
    Both,
}

impl Target {
    fn to_placement(self) -> Placement {
        match self {
            Self::Cache => Placement::Cache,
            Self::Nix => Placement::Nix,
            Self::Both => Placement::Both,
        }
    }
}

pub async fn run(
    lockfile_path: &Path,
    name: Option<String>,
    all: bool,
    target: Target,
) -> anyhow::Result<()> {
    if name.is_none() && !all {
        anyhow::bail!("must supply either <name> or --all");
    }
    let mut lock = lockfile_io::read(lockfile_path)?;
    if lock.entries.is_empty() {
        anyhow::bail!("lockfile {} is empty", lockfile_path.display());
    }

    let mut shifted = 0_usize;
    let mut skipped = 0_usize;
    let mut errored = 0_usize;
    let target_placement = target.to_placement();

    let entries_to_shift: Vec<usize> = if all {
        (0..lock.entries.len()).collect()
    } else {
        let n = name.as_deref().unwrap();
        let idx = lock
            .entries
            .iter()
            .position(|e| e.name == n)
            .ok_or_else(|| anyhow::anyhow!("no lockfile entry for `{n}`"))?;
        vec![idx]
    };

    for idx in entries_to_shift {
        let entry = &lock.entries[idx];
        let current = Placement::parse_lockfile(&entry.placement);
        if current == target_placement {
            skipped += 1;
            tracing::info!(name = %entry.name, placement = %current, "already at target placement");
            continue;
        }
        match target_placement {
            Placement::Nix => match placement::place_in_nix(entry).await {
                Ok(new) => {
                    lock.entries[idx] = new;
                    shifted += 1;
                    tracing::info!(name = %lock.entries[idx].name, "→ nix");
                }
                Err(e) => {
                    errored += 1;
                    tracing::error!(name = %entry.name, error = %e, "failed to promote to nix");
                }
            },
            Placement::Cache => {
                // The user must re-resolve via `estante install` for
                // cache placement; we can't fabricate a tarball from
                // a Nix store path without a full re-fetch. Mark the
                // intent + leave the resolver to act.
                tracing::warn!(
                    name = %entry.name,
                    "cache placement requires re-running `estante install` after this — flagging intent only"
                );
                lock.entries[idx] = placement::mark_as_cache(
                    entry,
                    Path::new(""), // resolver fills this in
                );
                shifted += 1;
            }
            Placement::Both => {
                lock.entries[idx] = placement::mark_as_both(entry);
                shifted += 1;
            }
        }
    }

    lockfile_io::write(lockfile_path, &lock).map_err(|e| {
        if e.to_string().contains("materialized-path") {
            anyhow::anyhow!("{e}\n\nHint: cache-placement entries need a real path. Run `estante install` after `estante place ... --to cache`.")
        } else {
            e
        }
    })?;

    println!(
        "{} shifted, {} already at target, {} errored.",
        shifted, skipped, errored,
    );
    if errored > 0 {
        anyhow::bail!("{errored} entr(y/ies) failed to migrate");
    }
    Ok(())
}
