//! `estante run <script>` — uv-like ad-hoc execution.
//!
//! Reads the script's inline metadata block, builds an ephemeral
//! manifest from its declared dependencies, resolves them via the
//! standard `Resolver` pipeline (network fetch + cache), and launches
//! frost with a generated rc.lisp that loads the deps + the script.
//!
//! Fast path — no Nix build, no `estante tool install`. The ephemeral
//! manifest never lands on disk in a discoverable location; it's
//! materialized inside the resolver's cache and the lockfile lives
//! in a `tempfile::TempDir` for the duration of the run.

use std::path::Path;

use anyhow::Context;
use estante_types::{Manifest, PkgSpec, Source};

use crate::config::Config;
use crate::inline_metadata::{self, CommentStyle};
use crate::lockfile_io;
use crate::manifest_io;
use crate::resolver::Resolver;

#[derive(Debug, Clone, Copy)]
enum Runtime {
    Frost,
    Bash,
    Zsh,
    Fish,
}

impl Runtime {
    fn from_path(p: &Path) -> Self {
        match p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "bash" | "sh" => Self::Bash,
            "zsh" => Self::Zsh,
            "fish" => Self::Fish,
            _ => Self::Frost,
        }
    }

    fn binary_name(self) -> &'static str {
        match self {
            Self::Frost => "frost",
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
        }
    }

    fn vanilla_entrypoint(self) -> &'static str {
        match self {
            Self::Bash | Self::Frost => "init.bash",
            Self::Zsh => "init.zsh",
            Self::Fish => "init.fish",
        }
    }
}

pub async fn run(
    script_path: &Path,
    cfg: &Config,
    shell_override: Option<String>,
) -> anyhow::Result<()> {
    let src = std::fs::read_to_string(script_path)
        .with_context(|| format!("reading script {}", script_path.display()))?;
    let (metadata, _body_line, comment_style) = inline_metadata::parse_with_style(&src)?;
    let runtime = Runtime::from_path(script_path);
    if metadata.dependencies.is_empty() {
        tracing::info!(
            kind = ?runtime,
            style = ?comment_style,
            "script has no inline metadata block — running with no estante deps"
        );
    } else {
        tracing::info!(
            kind = ?runtime,
            deps = metadata.dependencies.len(),
            "parsed inline metadata"
        );
    }

    // Build an ephemeral manifest. Each metadata dep becomes one
    // PkgSpec; names are derived from the source URL the same way
    // `actions::add` derives them.
    let mut manifest = Manifest::default();
    for src_str in &metadata.dependencies {
        let parsed = Source::parse(src_str)
            .with_context(|| format!("parsing inline-metadata dep `{src_str}`"))?;
        let derived_name = derive_name(&parsed);
        manifest.upsert(PkgSpec {
            name: derived_name,
            version: "0.0.0".into(),
            source: parsed.to_source_string(),
            exports: vec![],
            deps: vec![],
            lazy: false,
        });
    }

    let tmp = tempfile::tempdir().context("creating ephemeral manifest dir")?;
    let manifest_path = tmp.path().join("shellpkg.lisp");
    let lockfile_path = tmp.path().join("shellpkg.lock.lisp");
    manifest_io::write(&manifest_path, &manifest)?;

    // Resolve all deps. Base dir = script's parent dir so local:
    // relative paths resolve the same way they would for `estante lock`.
    let base_dir = script_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    let resolver = if let Some(base) = base_dir {
        Resolver::new(cfg)?.with_base_dir(base)
    } else {
        Resolver::new(cfg)?
    };
    let lock = resolver.resolve(&manifest).await?;
    lockfile_io::write(&lockfile_path, &lock)?;

    let shell_bin = shell_override.unwrap_or_else(|| runtime.binary_name().to_owned());
    match runtime {
        Runtime::Frost => {
            exec_frost(&tmp, &lockfile_path, &manifest, script_path, &shell_bin).await
        }
        Runtime::Bash | Runtime::Zsh | Runtime::Fish => {
            exec_vanilla(&tmp, &lock, script_path, &shell_bin, runtime).await
        }
    }
}

async fn exec_frost(
    tmp: &tempfile::TempDir,
    lockfile_path: &Path,
    manifest: &Manifest,
    script_path: &Path,
    frost_bin: &str,
) -> anyhow::Result<()> {
    let mut rc = String::new();
    rc.push_str(&format!(
        ";; estante run — ephemeral rc for {}\n",
        script_path.display()
    ));
    rc.push_str(&format!(
        "(defsource :path {:?})\n",
        lockfile_path.to_string_lossy()
    ));
    for pkg in &manifest.packages {
        rc.push_str(&format!("(defload :pkg {:?})\n", pkg.name));
    }
    rc.push_str(&format!(
        "(defsource :path {:?})\n",
        script_path.canonicalize()?.to_string_lossy()
    ));
    let rc_path = tmp.path().join("frostrc.lisp");
    std::fs::write(&rc_path, &rc)?;

    let status = tokio::process::Command::new(frost_bin)
        .arg("--rcfile")
        .arg(&rc_path)
        .status()
        .await
        .with_context(|| format!("exec frost (`{frost_bin}` not on PATH?)"))?;
    if !status.success() {
        anyhow::bail!("frost exited with status {status}");
    }
    Ok(())
}

async fn exec_vanilla(
    tmp: &tempfile::TempDir,
    lock: &estante_types::Lockfile,
    script_path: &Path,
    shell_bin: &str,
    runtime: Runtime,
) -> anyhow::Result<()> {
    // Vanilla shells consume each package's init.<shell> by sourcing
    // it. Generate a wrapper script that does exactly that, then exec
    // the chosen shell pointed at the wrapper.
    let mut wrapper = String::new();
    wrapper.push_str("#!/usr/bin/env ");
    wrapper.push_str(shell_bin);
    wrapper.push('\n');
    wrapper.push_str("# estante run — ephemeral wrapper. Sources every locked\n");
    wrapper.push_str("# package's vanilla entrypoint, then exec's the user script.\n\n");
    let entry = runtime.vanilla_entrypoint();
    for entry_lock in &lock.entries {
        wrapper.push_str(&format!(
            "if [ -f {path:?}/{entry} ]; then\n  . {path:?}/{entry}\nfi\n",
            path = entry_lock.materialized_path,
            entry = entry,
        ));
    }
    wrapper.push_str(&format!(
        "\n# user script\nexec {shell_bin} {script:?} \"$@\"\n",
        script = script_path.canonicalize()?.to_string_lossy(),
    ));
    let wrapper_path = tmp.path().join("wrapper.sh");
    std::fs::write(&wrapper_path, &wrapper)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&wrapper_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&wrapper_path, perms)?;
    }

    let status = tokio::process::Command::new(shell_bin)
        .arg(&wrapper_path)
        .status()
        .await
        .with_context(|| format!("exec {shell_bin} (`{shell_bin}` not on PATH?)"))?;
    if !status.success() {
        anyhow::bail!("{shell_bin} exited with status {status}");
    }
    Ok(())
}

fn derive_name(src: &Source) -> String {
    match src {
        Source::Github { repo, .. } => repo.clone(),
        Source::Gist { id, .. } => id.clone(),
        Source::Local { path } => Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "local-pkg".into()),
        Source::GitHttps { url, .. } | Source::GitSsh { url, .. } => {
            url.rsplit('/').next().unwrap_or("git-pkg").to_owned()
        }
    }
}
