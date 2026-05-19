;; estante — the typed SDLC manifest. One file, one source of truth.
;;
;; Substrate-side renderers consume this and emit:
;;   caixa-publish        → release flow + git tag + GHCR push
;;   caixa-publish-tlisp  → tatara-lisp publishing path (N/A for Binario)
;;   caixa-validate       → CSE invariant check (layout, slot coverage)
;;   caixa-forge          → repo + workflow regeneration (compares to
;;                          the (defrepo estante …) in pleme-io/repo-forge/repos.lisp)
;;
;; Author edits HERE; everything downstream re-derives from these slots.

(defcaixa
  :nome        "estante"
  :versao      "0.1.0"
  :kind        Binario
  :edicao      "2026"
  :descricao   "Cargo for shell — Nix-native, git-as-registry, Rust + Tatara-Lisp shell-package manager. Resolves `(defshellpkg …)` manifests from git, emits typed `(deflockedpkg …)` lockfiles, materializes package trees for `frost-lisp::defload` to consume."
  :repositorio "github:pleme-io/estante"
  :licenca     "MIT OR Apache-2.0"
  :autores     ("pleme-io")
  :etiquetas   ("rust" "tatara-lisp" "shell-package-manager"
                "git-as-registry" "frost" "frostmourne"
                "deterministic" "content-addressed" "caixa-binario")

  ;; Workspace deps that need to vendor at build time.
  :deps        ()
  :deps-dev    ()

  ;; The wired-up binary is what `feira publish` releases.
  :binarios    ("crates/estante")

  ;; Bibliotecas vendored alongside the bin. estante-types is the
  ;; downstream-consumable typed surface — third-party packages or
  ;; alternative resolvers depend on it without pulling the bin.
  :bibliotecas ("crates/estante-types")
  )
