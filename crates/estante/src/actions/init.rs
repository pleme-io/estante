//! `estante init` — write a starter `shellpkg.lisp` in the working
//! directory.

use std::path::Path;

use estante_types::{Manifest, PkgSpec};

use crate::manifest_io;

pub async fn run(manifest_path: &Path, name: Option<String>) -> anyhow::Result<()> {
    if manifest_path.exists() {
        anyhow::bail!(
            "manifest already exists at {} — refusing to overwrite",
            manifest_path.display()
        );
    }
    let derived_name = name
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        })
        .unwrap_or_else(|| "my-shellpkg".to_owned());
    let mut m = Manifest::default();
    m.upsert(PkgSpec {
        name: derived_name.clone(),
        version: "0.1.0".to_owned(),
        // A placeholder source — the operator overwrites this with
        // their real GitHub URL before `lock`.
        source: format!("github:YOUR_ORG/{derived_name}"),
        exports: vec![],
        deps: vec![],
        lazy: false,
    });
    manifest_io::write(manifest_path, &m)?;
    tracing::info!(
        path = %manifest_path.display(),
        name = %derived_name,
        "wrote starter manifest"
    );
    println!("Wrote {}", manifest_path.display());
    Ok(())
}
