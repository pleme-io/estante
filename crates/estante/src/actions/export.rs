//! `estante export` — emit the lockfile in alternate formats for
//! cross-tool consumption.
//!
//! Three formats today:
//!
//!   `lisp`   (default) — the canonical `shellpkg.lock.lisp` form.
//!                        Re-emits to stdout from the on-disk file —
//!                        useful for piping into review tools.
//!   `nix`              — pure-data Nix attrset. Consumed by
//!                        `substrate/lib/build/estante/` to materialize
//!                        the whole env at Nix build time.
//!   `json`             — machine-friendly debug shape. Maps 1:1 to
//!                        `LockedPkgSpec` via serde.

use std::path::Path;

use estante_types::nix_export;
use serde_json;

use crate::lockfile_io;

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Format {
    Lisp,
    Nix,
    Json,
}

pub async fn run(
    lockfile_path: &Path,
    format: Format,
    output: Option<&Path>,
) -> anyhow::Result<()> {
    let lock = lockfile_io::read(lockfile_path)?;
    let rendered = match format {
        Format::Lisp => lock.to_string(),
        Format::Nix => nix_export::lockfile_to_nix(&lock),
        Format::Json => serde_json::to_string_pretty(&serde_json::json!({
            "schemaVersion": 1,
            "packages": lock.entries.iter().map(|e| serde_json::json!({
                "name": e.name,
                "source": e.source,
                "rev": e.rev,
                "narHash": e.nar_hash,
                "blake3": e.blake3,
                "materializedPath": e.materialized_path,
            })).collect::<Vec<_>>(),
        }))?,
    };

    if let Some(out_path) = output {
        std::fs::write(out_path, &rendered)?;
        println!(
            "Wrote {} entries to {}",
            lock.entries.len(),
            out_path.display()
        );
    } else {
        print!("{rendered}");
    }
    Ok(())
}
