//! `estante expand` — print the materialized rc.lisp source frost-lisp
//! would see for a given lockfile entry. Useful for debugging
//! `defload` failures.

use std::path::Path;

use crate::lockfile_io;

pub async fn run(lockfile_path: &Path) -> anyhow::Result<()> {
    let lock = lockfile_io::read(lockfile_path)?;
    if lock.entries.is_empty() {
        anyhow::bail!("lockfile at {} is empty", lockfile_path.display());
    }
    for entry in &lock.entries {
        let rc = std::path::Path::new(&entry.materialized_path).join("rc.lisp");
        println!("─── {} ({})", entry.name, entry.rev);
        println!("─── {}\n", rc.display());
        if !rc.is_file() {
            println!(";; (rc.lisp not present — run `estante install`)");
            continue;
        }
        let src = std::fs::read_to_string(&rc)?;
        println!("{src}");
    }
    Ok(())
}
