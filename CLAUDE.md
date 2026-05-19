# estante — Claude Orientation

> **★★★ CSE / Knowable Construction.** This repo operates under
> **Constructive Substrate Engineering** — canonical specification at
> [`pleme-io/theory/CONSTRUCTIVE-SUBSTRATE-ENGINEERING.md`](https://github.com/pleme-io/theory/blob/main/CONSTRUCTIVE-SUBSTRATE-ENGINEERING.md).
> The Compounding Directive (operational rules: solve once, load-bearing
> fixes only, idiom-first, models stay current, direction beats velocity)
> is in the org-level pleme-io/CLAUDE.md ★★★ section. Read both before
> non-trivial changes.

One-sentence purpose: cargo for shell — Nix-native, git-as-registry,
Rust + Tatara-Lisp shell-package manager. Resolves `(defshellpkg …)`
manifests from git repos, emits typed `(deflockedpkg …)` lockfiles,
and materializes content for `frost-lisp::defload` to consume.

## Classification

- **Archetype:** rust-workspace (types lib + bin)
- **Workspace members:** `crates/estante-types` (lib) + `crates/estante` (bin)
- **Substrate flake:** `rust-workspace-release-flake.nix` — same shape as cofre
- **Repo visibility:** private at first; open-source under MIT-OR-Apache-2.0
  when promoted via the pleme-io-github-posture flow.

## Where to look

| Intent | File |
|--------|------|
| Typed primitives — `PkgSpec`, `LoadSpec`, `LockedPkgSpec` | `frost/crates/frost-lisp/src/pkg.rs` (re-exported here) |
| Resolver-facing wrappers — `Manifest`, `Lockfile`, `Source` | `crates/estante-types/src/lib.rs` |
| Subcommand dispatch | `crates/estante/src/main.rs` |
| Per-subcommand impl | `crates/estante/src/actions/*.rs` |
| Resolver — git fetch + hashing | `crates/estante/src/resolver.rs`, `fetch.rs` |
| Cache + manifest/lockfile I/O | `crates/estante/src/{cache,manifest_io,lockfile_io}.rs` |
| Estante config (cache dir, token) | `crates/estante/src/config.rs` |
| Architecture overview | `README.md` |

## Three layers, mapped

```
       estante-types  (lib)         — typed primitives + Lisp formatters, no I/O
            │
            ▼
       estante         (bin)        — fetch, unpack, hashing, octocrab, CLI
            │
            ▼
   frost-lisp::apply_source         — consumes the lockfile via `defload`
```

## Hard rules

1. **Typed forms live ONCE.** `PkgSpec` / `LoadSpec` / `LockedPkgSpec`
   are defined in `frost/crates/frost-lisp/src/pkg.rs`. estante
   re-exports — never duplicates. Pillar 12: solve once.
2. **`format!()` is banned.** Lisp emission goes through `Display`
   impls inside `estante-types`. Hand-rolled string concatenation of
   Lisp/Nix syntax is forbidden. Extend `Manifest::Display` /
   `Lockfile::Display` / `LispString` instead.
3. **No shell.** Subprocess invocations are forbidden — `tar` and
   `flate2` unpack the GitHub tarball directly; gix (when added)
   speaks the git protocol from inside the Rust process. Pleme-io's
   NO SHELL directive.
4. **Git is the registry.** No central package server. Discovery is
   `octocrab.search.repositories("topic:estante-pkg <q>")`; fetch is
   `octocrab.repos.download_tarball(rev)`; private repos auth via a
   `cofre::SecretRef` GitHub PAT.
5. **Rate-limit through samba (M1d).** Every octocrab call routes
   through the samba broker at `quotaPct = 0.02` so estante shares
   the fleet GitHub budget with `tend`.
6. **Lockfile is content-addressed.** Two hashes per entry: BLAKE3
   (tameshi-chain) and Nix narHash. Lockfile MUST validate before
   write — `Lockfile::validate_materialized()` gates emit.
7. **Tests sandboxed.** Unit tests in `estante-types` are
   fully-sandboxed (no fs, no net). Integration tests in `estante`
   that touch git/HTTP land under `#[ignore]` with a per-run setup
   helper or hit a tiny in-tree fixture repo.

## What NOT to do

- Don't duplicate the typed forms locally. If you want a field
  that doesn't exist, add it to `frost/crates/frost-lisp/src/pkg.rs`
  upstream (frost-lisp owns the canonical types).
- Don't shell out to `git`, `curl`, `tar`, `nix-prefetch-url`, or any
  other binary. Use gix / reqwest / octocrab / tar+flate2 inside the
  Rust process. NO SHELL.
- Don't write a `format!("(deflockedpkg :name {} …)")` call. Use the
  `LispString` / `LispStringList` newtype display surfaces.
- Don't bypass `samba` for octocrab once it's wired (M1d). Even
  unauthenticated GitHub has a 60 req/h ceiling — sharing the budget
  with `tend` is what keeps both alive in CI.
- Don't write secrets to argv. GitHub tokens flow through cofre's
  `SecretRef`.

## Companion typescapes / consumers

- `pleme-io/frost` — owns the typed forms in `crates/frost-lisp/src/pkg.rs`.
- `pleme-io/frostmourne` — the canonical curated distribution; first
  consumer that migrates from Nix-concatenation to estante (M2).
- `pleme-io/cofre` — typed secret materialization (`SecretRef` for
  GitHub tokens). M1d integration.
- `pleme-io/samba` — typed rate-limited consumer broker. M1d
  integration.
- `pleme-io/caixa-frost` — a new caixa renderer that turns a
  `Biblioteca`-kind caixa into an estante-installable package
  (M2).

## Tests

```
crates/estante-types  →  Manifest / Lockfile / Source round-trips,
                          LispString escaping, duplicate detection
crates/estante        →  CLI smoke + per-action unit tests (M1f)
```
