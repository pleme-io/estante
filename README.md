# estante

**Cargo for shell.** Nix-native, git-as-registry, Rust + Tatara-Lisp.

`estante` manages typed shell-behavior libraries — aliases, prompts,
hooks, completions, keybindings — declared as `(defshellpkg …)` in
a git repo and consumed via `(defload …)` from `frost`'s tatara-lisp
rc files. No central registry. Git refs are the version surface;
GitHub topic search (`topic:estante-pkg`) is the discovery surface.

## Status: framework-ready. Resolver / lockfile / receipt / verify / doctor / Nix integration all shipped. M2 (caixa-frost renderer) next — see CLAUDE.md.

## Quick start

```bash
# In a directory you want to consume packages from:
estante init                                       # write shellpkg.lisp
estante add github:MichaelAquilina/zsh-you-should-use@v1.7.4
estante lock --emit-receipt                        # resolve + emit lockfile + attestation receipt
estante install                                    # fetch + materialize
# Then in ~/.frostrc.lisp:
#   (defsource :path "./shellpkg.lock.lisp")
#   (defload   :pkg "zsh-you-should-use")
```

## CI gates

estante ships three CI-grade verifiability primitives:

```bash
estante lock --check        # "the committed lockfile reflects the manifest"
estante attest --check      # "the committed receipt matches current state"
estante verify --strict     # "materialized trees still hash to the locked BLAKE3"
estante doctor              # 9-check health audit composing the above + more
```

All four accept `--json` for machine-readable output. Substrate also ships
`mkReceiptVerifier` — a Nix derivation that runs `attest --check` at build
time, so `nix flake check` gates on receipt freshness.

## The three forms

```lisp
;; shellpkg.lisp — author / consumer's manifest.
(defshellpkg
  :name    "you-should-use"
  :version "1.7.4"
  :source  "github:MichaelAquilina/zsh-you-should-use@v1.7.4"
  :exports ("alias" "hook"))

;; shellpkg.lock.lisp — machine-emitted by `estante install`.
(deflockedpkg
  :name              "you-should-use"
  :source            "github:MichaelAquilina/zsh-you-should-use"
  :rev               "aa489f1d0bef818c4ec7d09b87a44d5cabaa9b6f"
  :nar-hash          "sha256-…"
  :blake3            "blake3-…"
  :materialized-path "/nix/store/abc-you-should-use-1.7.4/")

;; ~/.frostrc.lisp — consumer side.
(defsource :path "./shellpkg.lock.lisp")
(defload   :pkg "you-should-use")
```

## Substrate fit

| Concern | Pleme-io primitive |
|---|---|
| Typed forms | `frost-lisp::pkg::{PkgSpec, LoadSpec, LockedPkgSpec}` (re-exported from estante-types) |
| Authoring surface | Tatara-Lisp via `#[derive(DeriveTataraDomain)]` |
| Workspace shape | `rust-workspace-release-flake.nix` (substrate) — same as `cofre` |
| Token discipline | `cofre::SecretRef` (M1d) |
| Rate-limit budget | `samba` typed broker (M1d) |
| Caixa kind | `Biblioteca` with a new `caixa-frost` renderer (M2) |
| Lockfile attestation | BLAKE3 + Nix narHash dual hash |
| Nix-side loader | `substrate/lib/build/estante` — `loadLockfile` / `loadReceipt` / `mkShellPackage` / `mkShellEnv` / `mkScriptBinary` / `mkReceiptVerifier` |

## Architecture

```
manifest (shellpkg.lisp)         author authors `(defshellpkg :name … :source …)`
    │
    ▼
Resolver  ── octocrab(search) ── samba ── GitHub API
    │  ── gix(fetch) ─────────────────── git protocol
    │  ── unpack (tar/flate2) ────────── local cache
    ▼
lockfile (shellpkg.lock.lisp)    `(deflockedpkg :name … :rev … :hashes …)`
    │
    ▼
frost-lisp::apply_source         `(defload :pkg "foo")` → `(defsource :path "<materialized>/rc.lisp")`
    │
    ▼
ShellEnv (aliases, hooks, …)     same code path as if you'd authored them in your `.frostrc.lisp`
```

## License

MIT OR Apache-2.0.
