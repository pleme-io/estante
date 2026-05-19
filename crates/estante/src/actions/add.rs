//! `estante add <source>` — append a `(defshellpkg …)` entry to the
//! manifest.

use std::path::Path;

use estante_types::{PkgSpec, Source};

use crate::manifest_io;

pub async fn run(
    manifest_path: &Path,
    source: &str,
    name_override: Option<String>,
    version_override: Option<String>,
) -> anyhow::Result<()> {
    let parsed = Source::parse(source)?;
    let derived_name = name_override.unwrap_or_else(|| derive_name(&parsed));
    let derived_version = version_override.unwrap_or_else(|| derive_version(&parsed));

    let mut m = manifest_io::read(manifest_path)?;
    m.upsert(PkgSpec {
        name: derived_name.clone(),
        version: derived_version.clone(),
        source: parsed.to_source_string(),
        exports: vec![],
        deps: vec![],
        lazy: false,
    });
    manifest_io::write(manifest_path, &m)?;

    tracing::info!(
        name = %derived_name,
        version = %derived_version,
        source = %parsed,
        "added package"
    );
    println!(
        "Added {derived_name}@{derived_version} ({parsed}) to {}",
        manifest_path.display()
    );
    Ok(())
}

fn derive_name(s: &Source) -> String {
    match s {
        Source::Github { repo, .. } => repo.clone(),
        Source::Gist { id, .. } => id.clone(),
        Source::Local { path } => Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "local-pkg".to_owned()),
        Source::GitHttps { url, .. } | Source::GitSsh { url, .. } => {
            url.rsplit('/').next().unwrap_or("git-pkg").to_owned()
        }
    }
}

fn derive_version(s: &Source) -> String {
    let r = match s {
        Source::Github { reference, .. }
        | Source::Gist { reference, .. }
        | Source::GitHttps { reference, .. }
        | Source::GitSsh { reference, .. } => reference.as_str(),
        Source::Local { .. } => "local",
    };
    if r == "HEAD" {
        "0.1.0".to_owned()
    } else {
        r.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_name_from_github_uses_repo() {
        let s = Source::parse("github:foo/zsh-bar@v1").unwrap();
        assert_eq!(derive_name(&s), "zsh-bar");
    }

    #[test]
    fn derive_version_from_ref() {
        let s = Source::parse("github:foo/bar@v1.7.4").unwrap();
        assert_eq!(derive_version(&s), "v1.7.4");
    }

    #[test]
    fn derive_version_head_falls_back() {
        let s = Source::parse("github:foo/bar").unwrap();
        assert_eq!(derive_version(&s), "0.1.0");
    }
}
