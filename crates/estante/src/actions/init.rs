//! `estante init` — scaffold a new shell-package repo.
//!
//! Three kinds × three compat targets. Every combination produces a
//! self-contained source tree consumable by both estante and the
//! substrate Nix builders.
//!
//! ## Kinds
//!
//! | Kind     | Files generated                                 |
//! |----------|-------------------------------------------------|
//! | Library  | shellpkg.lisp + entrypoint(s) + flake.nix       |
//! | Binary   | shellpkg.lisp + main.{lisp,bash,zsh} + flake.nix |
//! | Daemon   | shellpkg.lisp + service.{lisp,bash} + service-unit templates + flake.nix |
//!
//! ## Compat targets
//!
//! | Compat   | Entrypoints scaffolded                          |
//! |----------|-------------------------------------------------|
//! | Frost    | rc.lisp (tatara-lisp def-forms)                 |
//! | Vanilla  | init.bash + init.zsh (POSIX shell, frost-free)  |
//! | Both     | rc.lisp + init.bash + init.zsh                  |
//!
//! ## Hard rule — vanilla support is first-class
//!
//! The estante substrate consumes packages WITHOUT requiring frost-lisp
//! at runtime. A vanilla-compat package's `init.bash` is a plain POSIX
//! file that consumers `source` from bash/zsh; the materialized path
//! lives in the same Nix store derivation as frost-targeted packages.
//! The two paths share `shellpkg.lisp` + flake.nix; the entrypoint
//! file shape is the only difference.

use std::path::Path;

use clap::ValueEnum;
use estante_types::{Manifest, PkgSpec};

use crate::manifest_io;

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum Kind {
    /// Set of reusable behavior forms (aliases, hooks, completions).
    /// Consumed via `(defload :pkg "<name>")` in a consumer's rc.lisp,
    /// or `source $matpath/init.<shell>` in a vanilla shell.
    Library,
    /// One-shot wrapped script. The materialized `<name>` binary lives
    /// in $PATH after `estante tool install`.
    Binary,
    /// Long-running service. Ships launchd.plist + systemd.service
    /// templates alongside the script.
    Daemon,
}

impl Kind {
    fn export_label(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Binary => "binary",
            Self::Daemon => "daemon",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum Compat {
    /// frost-lisp entrypoint (rc.lisp).
    Frost,
    /// POSIX shell entrypoints (init.bash + init.zsh). Frost-free.
    Vanilla,
    /// Ship both — consumers pick which to source at runtime.
    Both,
}

impl Compat {
    fn includes_frost(self) -> bool {
        matches!(self, Self::Frost | Self::Both)
    }
    fn includes_vanilla(self) -> bool {
        matches!(self, Self::Vanilla | Self::Both)
    }
}

pub async fn run(
    manifest_path: &Path,
    name: Option<String>,
    kind: Kind,
    compat: Compat,
) -> anyhow::Result<()> {
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
    let repo_dir = manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    std::fs::create_dir_all(&repo_dir)?;

    // 1. shellpkg.lisp — the manifest. Always written.
    let manifest = author_manifest(&derived_name, kind);
    manifest_io::write(manifest_path, &manifest)?;

    // 2. Entrypoint files — depends on kind + compat.
    let writes = scaffold_entrypoints(&repo_dir, &derived_name, kind, compat)?;

    // 3. README, flake.nix — always.
    write_if_missing(
        &repo_dir.join("README.md"),
        &render_readme(&derived_name, kind, compat),
    )?;
    write_if_missing(
        &repo_dir.join("flake.nix"),
        &render_flake_nix(&derived_name, kind),
    )?;

    tracing::info!(
        name = %derived_name,
        kind = ?kind,
        compat = ?compat,
        files = writes.len(),
        "scaffolded estante package"
    );
    println!("Scaffolded `{derived_name}` ({kind:?}, compat: {compat:?})");
    println!("  manifest:  {}", manifest_path.display());
    for w in writes {
        println!("  +         {}", w.display());
    }
    println!("  README:    {}", repo_dir.join("README.md").display());
    println!("  flake.nix: {}", repo_dir.join("flake.nix").display());
    println!();
    println!("Next:");
    println!("  estante validate    # syntax-check the scaffold");
    println!("  estante lock        # if you add deps to shellpkg.lisp");
    Ok(())
}

fn author_manifest(name: &str, kind: Kind) -> Manifest {
    let mut m = Manifest::default();
    m.upsert(PkgSpec {
        name: name.to_owned(),
        version: "0.1.0".into(),
        source: format!("github:YOUR_ORG/{name}"),
        exports: vec![format!("kind:{}", kind.export_label())],
        deps: vec![],
        lazy: false,
    });
    m
}

fn scaffold_entrypoints(
    repo_dir: &Path,
    name: &str,
    kind: Kind,
    compat: Compat,
) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut written = Vec::new();
    match kind {
        Kind::Library => {
            if compat.includes_frost() {
                let p = repo_dir.join("rc.lisp");
                write_if_missing(&p, &template_library_rc_lisp(name))?;
                written.push(p);
            }
            if compat.includes_vanilla() {
                let p_bash = repo_dir.join("init.bash");
                write_if_missing(&p_bash, &template_library_init_bash(name))?;
                written.push(p_bash);
                let p_zsh = repo_dir.join("init.zsh");
                write_if_missing(&p_zsh, &template_library_init_zsh(name))?;
                written.push(p_zsh);
            }
        }
        Kind::Binary => {
            if compat.includes_frost() {
                let p = repo_dir.join("main.lisp");
                write_if_missing(&p, &template_binary_main_lisp(name))?;
                written.push(p);
                // The rc.lisp is what frost loads via defload — for a Binary
                // it just sources main.lisp.
                let rc = repo_dir.join("rc.lisp");
                write_if_missing(&rc, &template_binary_rc_lisp())?;
                written.push(rc);
            }
            if compat.includes_vanilla() {
                let p = repo_dir.join("main.bash");
                write_if_missing(&p, &template_binary_main_bash(name))?;
                #[cfg(unix)]
                make_executable(&p)?;
                written.push(p);
            }
        }
        Kind::Daemon => {
            if compat.includes_frost() {
                let p = repo_dir.join("service.lisp");
                write_if_missing(&p, &template_daemon_service_lisp(name))?;
                written.push(p);
                let rc = repo_dir.join("rc.lisp");
                write_if_missing(&rc, &template_daemon_rc_lisp())?;
                written.push(rc);
            }
            if compat.includes_vanilla() {
                let p = repo_dir.join("service.bash");
                write_if_missing(&p, &template_daemon_service_bash(name))?;
                #[cfg(unix)]
                make_executable(&p)?;
                written.push(p);
            }
            // Daemon-specific unit files. Ship templates regardless of compat;
            // operators wire to whichever entrypoint they use.
            let launchd_dir = repo_dir.join("dist");
            std::fs::create_dir_all(&launchd_dir)?;
            let launchd = launchd_dir.join(format!("io.pleme.{name}.plist"));
            write_if_missing(&launchd, &template_daemon_launchd_plist(name))?;
            written.push(launchd);
            let systemd = launchd_dir.join(format!("{name}.service"));
            write_if_missing(&systemd, &template_daemon_systemd_unit(name))?;
            written.push(systemd);
        }
    }
    Ok(written)
}

fn write_if_missing(path: &Path, content: &str) -> std::io::Result<()> {
    if path.exists() {
        Ok(())
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
}

// ─── Template renderers ───────────────────────────────────────────────

fn template_library_rc_lisp(name: &str) -> String {
    format!(
        r#";; {name} :: rc.lisp
;; frost-lisp entrypoint. Consumed by `(defload :pkg "{name}")`.

(defalias :name "{name}-status" :value "echo {name} loaded")
"#
    )
}

fn template_library_init_bash(name: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
# {name} :: init.bash
# Vanilla-shell entrypoint. Consume via:
#   source "$(estante info --print-store)/store/{name}/init.bash"
# or (after `home-manager switch`):
#   source "${{XDG_DATA_HOME:-$HOME/.local/share}}/estante/{name}/init.bash"

# Estante exposes this guard so multiple sourcings are idempotent.
if [ -n "${{__ESTANTE_PKG_{name_upper}_LOADED:-}}" ]; then
  return 0 2>/dev/null || true
fi
__ESTANTE_PKG_{name_upper}_LOADED=1

alias {name}-status='echo "{name} loaded"'
"#,
        name_upper = name.to_uppercase().replace('-', "_"),
    )
}

fn template_library_init_zsh(name: &str) -> String {
    format!(
        r#"# {name} :: init.zsh — zsh-flavored entrypoint.
# Mirrors init.bash; consumed via `source $matpath/init.zsh`.

if [[ -n "${{__ESTANTE_PKG_{name_upper}_LOADED:-}}" ]]; then
  return 0
fi
__ESTANTE_PKG_{name_upper}_LOADED=1

alias {name}-status='echo "{name} loaded"'
"#,
        name_upper = name.to_uppercase().replace('-', "_"),
    )
}

fn template_binary_main_lisp(name: &str) -> String {
    format!(
        r#";;; --- estante
;;; dependencies: []
;;; provides: {name}
;;; ---
;; {name} :: main.lisp
;; Entry script. `estante tool install ./main.lisp` wraps this as a
;; system binary via `mkScriptBinary`. `estante run ./main.lisp` runs
;; it ad-hoc.

(defalias :name "{name}-go" :value "echo running {name}")
(defun :name "{name}-main"
       :body "echo {name} main fired")
"#
    )
}

fn template_binary_rc_lisp() -> String {
    // For a Binary kind, the package's rc.lisp just defsources main.lisp.
    r#";; rc.lisp — re-exports main.lisp for `defload` consumers.
(defsource :path "main.lisp")
"#
    .to_owned()
}

fn template_binary_main_bash(name: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
# {name} :: main.bash
# Vanilla-shell binary. `estante tool install ./main.bash` wraps this
# as a system binary. Estante reads inline metadata between the
# `# === estante` markers so the same deps machinery works for shell
# scripts.

# === estante
# dependencies: []
# provides: {name}
# ===

set -euo pipefail
echo "{name} running ($*)"
"#
    )
}

fn template_daemon_service_lisp(name: &str) -> String {
    format!(
        r#";;; --- estante
;;; dependencies: []
;;; provides: {name}
;;; ---
;; {name} :: service.lisp — long-running tatara-lisp entrypoint.

(defalias :name "{name}-status" :value "echo {name} alive")

;; The launch host (launchd / systemd) supervises this entrypoint;
;; restart on failure is handled by the unit file in dist/.
"#
    )
}

fn template_daemon_rc_lisp() -> String {
    r#";; rc.lisp — defload-side wrapper for the daemon's service.lisp.
(defsource :path "service.lisp")
"#
    .to_owned()
}

fn template_daemon_service_bash(name: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
# {name} :: service.bash — long-running entrypoint.
# Process supervision lives in dist/io.pleme.{name}.plist (macOS) +
# dist/{name}.service (Linux). estante does not own that lifecycle —
# it just ships the unit-file templates.

set -euo pipefail
echo "{name} starting (pid $$)"
while true; do
  sleep 60
  echo "{name} heartbeat $(date -u +%FT%TZ)"
done
"#
    )
}

fn template_daemon_launchd_plist(name: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>io.pleme.{name}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/env</string>
    <string>bash</string>
    <string>/REPLACE_WITH_MATERIALIZED_PATH/service.bash</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>/tmp/{name}.log</string>
  <key>StandardErrorPath</key><string>/tmp/{name}.err</string>
</dict>
</plist>
"#
    )
}

fn template_daemon_systemd_unit(name: &str) -> String {
    format!(
        r#"[Unit]
Description={name} (estante daemon)
After=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/env bash /REPLACE_WITH_MATERIALIZED_PATH/service.bash
Restart=on-failure
RestartSec=5s
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=default.target
"#
    )
}

fn render_readme(name: &str, kind: Kind, compat: Compat) -> String {
    format!(
        r#"# {name}

An estante shell-package — kind `{kind_label}`, compat `{compat:?}`.

## Build

```bash
nix build .#default
```

## Install via estante (consumer side)

```bash
# As a consumer of this package's published GitHub repo:
estante add github:YOUR_ORG/{name}@v0.1.0
estante lock && estante install
```

Then in your `~/.frostrc.lisp`:

```lisp
(defsource :path "./shellpkg.lock.lisp")
(defload   :pkg "{name}")
```

## Vanilla shell consumption (frost-free)

This package ships POSIX shell entrypoints — `source` the materialized
init.bash / init.zsh directly:

```bash
source "$(estante expand | head -1 | awk '{{print $NF}}')/init.bash"
```

(Run `estante info` for the cache directory layout.)

## License

MIT.
"#,
        kind_label = kind.export_label(),
    )
}

fn render_flake_nix(name: &str, _kind: Kind) -> String {
    format!(
        r#"# flake.nix — emits the materialized derivation consumed by estante.
{{
  description = "estante shell-package: {name}";

  inputs = {{
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    flake-utils.url = "github:numtide/flake-utils";
    substrate = {{
      url = "github:pleme-io/substrate";
      inputs.nixpkgs.follows = "nixpkgs";
    }};
  }};

  outputs = inputs: (import "${{inputs.substrate}}/lib/build/estante/flake.nix" {{
    inherit (inputs) nixpkgs flake-utils;
  }}) {{
    name = "{name}";
    version = "0.1.0";
    src = inputs.self;
    description = "estante shell-package: {name}";
    exports = [];
  }};
}}
"#
    )
}
