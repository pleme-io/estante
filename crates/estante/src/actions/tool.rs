//! `estante tool` — uv-like persistent install.
//!
//! Wraps a tatara-lisp script (or a single named package) as a
//! installable binary by emitting a Nix derivation expression that
//! consumes `substrate/lib/build/estante/mk-script-binary.nix`. The
//! emitted derivation is the contract; how the operator installs it
//! (nix profile install, home-manager component, fleet rebuild) is
//! orthogonal.
//!
//! v0.1 emits the Nix derivation to a path under the cache and prints
//! the activation command. A future enhancement would shell out to
//! `nix profile install` directly — out of scope for the first cut.

use std::path::Path;

use anyhow::Context;
use estante_types::{Manifest, PkgSpec, Source, nix_export};

use crate::config::Config;
use crate::inline_metadata;
use crate::lockfile_io;
use crate::manifest_io;
use crate::resolver::Resolver;

pub async fn install(
    script_path: &Path,
    cfg: &Config,
    name_override: Option<String>,
) -> anyhow::Result<()> {
    let src = std::fs::read_to_string(script_path)
        .with_context(|| format!("reading script {}", script_path.display()))?;
    let (metadata, _) = inline_metadata::parse(&src)?;
    let tool_name = name_override
        .or_else(|| metadata.provides.clone())
        .or_else(|| {
            script_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .ok_or_else(|| anyhow::anyhow!("could not derive tool name; pass --name"))?;

    // Build the manifest from inline metadata.
    let mut manifest = Manifest::default();
    for s in &metadata.dependencies {
        let parsed = Source::parse(s)?;
        let derived = derive_name(&parsed);
        manifest.upsert(PkgSpec {
            name: derived,
            version: "0.0.0".into(),
            source: parsed.to_source_string(),
            exports: vec![],
            deps: vec![],
            lazy: false,
        });
    }

    // Persistent location: $XDG_CACHE_HOME/estante/tools/<name>/
    let tool_dir = cfg.cache_dir.join("tools").join(&tool_name);
    std::fs::create_dir_all(&tool_dir).context("creating tool dir")?;
    let manifest_path = tool_dir.join("shellpkg.lisp");
    let lockfile_path = tool_dir.join("shellpkg.lock.lisp");
    let lockfile_nix_path = tool_dir.join("shellpkg.lock.nix");
    let derivation_path = tool_dir.join("default.nix");
    let script_copy_path = tool_dir.join("script.lisp");

    manifest_io::write(&manifest_path, &manifest)?;

    // Resolve + materialize.
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
    std::fs::write(&lockfile_nix_path, nix_export::lockfile_to_nix(&lock))?;
    std::fs::copy(script_path, &script_copy_path)?;

    // Emit the Nix derivation that wraps everything.
    let derivation = render_tool_derivation(&tool_name);
    std::fs::write(&derivation_path, &derivation)?;

    println!("Installed tool `{tool_name}` to {}", tool_dir.display());
    println!();
    println!("To install system-wide:");
    println!("  nix profile install path:{}", tool_dir.display());
    println!();
    println!("To consume via home-manager:");
    println!(
        "  imports = [ {{ home.packages = [ (import {}/default.nix {{ pkgs = pkgs; substrate = inputs.substrate; }}) ]; }} ];",
        tool_dir.display()
    );
    Ok(())
}

pub async fn uninstall(name: &str, cfg: &Config) -> anyhow::Result<()> {
    let tool_dir = cfg.cache_dir.join("tools").join(name);
    if !tool_dir.exists() {
        anyhow::bail!("tool `{name}` is not installed");
    }
    std::fs::remove_dir_all(&tool_dir)?;
    println!("Removed tool `{name}` ({})", tool_dir.display());
    Ok(())
}

pub async fn list(cfg: &Config) -> anyhow::Result<()> {
    let tools_dir = cfg.cache_dir.join("tools");
    if !tools_dir.exists() {
        println!("No tools installed.");
        return Ok(());
    }
    let mut names: Vec<String> = std::fs::read_dir(&tools_dir)?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .collect();
    names.sort();
    if names.is_empty() {
        println!("No tools installed.");
    } else {
        for n in names {
            println!("{n}");
        }
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

fn render_tool_derivation(tool_name: &str) -> String {
    // The derivation consumes substrate's mkScriptBinary. The
    // consumer flake passes `substrate` as an input and `pkgs` as the
    // platform-resolved nixpkgs.
    format!(
        r#"# default.nix — `estante tool install` generated wrapper for `{tool_name}`.
# Consumer:
#   nix profile install path:.
# or via a HM module:
#   home.packages = [ (import ./default.nix {{ pkgs = pkgs; substrate = inputs.substrate; }}) ];
{{ pkgs, substrate, frost ? null, ... }}:
let
  estante = import "${{substrate}}/lib/build/estante" {{ inherit pkgs; }};
in estante.mkScriptBinary {{
  name = "{tool_name}";
  script = ./script.lisp;
  lockfile = ./shellpkg.lock.nix;
  inherit frost;
}}
"#
    )
}
