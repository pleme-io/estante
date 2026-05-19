# estante examples

Two self-contained walkthroughs that mirror the production flow.

## `frostmourne-style/`

Shows the canonical estante consumer pattern — the exact shape a
distribution like `frostmourne` adopts to load packages from git
rather than concatenating Lisp files at Nix build time.

```
frostmourne-style/
  packages/                       # consumer-local fixture packages
    example-pkg/
      rc.lisp                     # ← a (defalias …) the package exports
  consumer/
    shellpkg.lisp                 # ← author manifest (estante reads this)
    frostrc.lisp                  # ← consumer rc (frost reads this)
```

### Driving the demo

From the repo root:

```bash
# 1. Manifest + lockfile resolution.
cargo run --bin estante -- \
  --manifest examples/frostmourne-style/consumer/shellpkg.lisp \
  --lockfile examples/frostmourne-style/consumer/shellpkg.lock.lisp \
  lock

# 2. Materialize into the local cache.
cargo run --bin estante -- \
  --manifest examples/frostmourne-style/consumer/shellpkg.lisp \
  --lockfile examples/frostmourne-style/consumer/shellpkg.lock.lisp \
  install

# 3. Inspect what frost-lisp will see.
cargo run --bin estante -- \
  --lockfile examples/frostmourne-style/consumer/shellpkg.lock.lisp \
  expand
```

Step 3 prints the rc.lisp of every locked package. If you point
`$FROSTRC` at `examples/frostmourne-style/consumer/frostrc.lisp` and
launch `frost`, the `(defalias :name "example" …)` from the package
lands in `env.aliases` and you can run `example` interactively.

### Migrating frostmourne itself

The same recipe — copy `frostmourne-style/consumer/shellpkg.lisp` and
`frostrc.lisp` into the frostmourne repo, register the packages you
want via `estante add github:…`, and commit the lockfile. The current
`frostmourne/lisp/6X-tools-*.lisp` files become candidates for
"de-inlining" once their upstream maintainers ship `rc.lisp`
entrypoints (per the M3 roadmap).

### Why a fixture package?

The `local:` source makes the demo offline + deterministic — running
the same `estante lock` twice produces a byte-identical
`shellpkg.lock.lisp`, content-addressed via BLAKE3 of the unpacked
tree. Anyone with the same fixture content gets the same digest;
verification is one BLAKE3 invocation away.

The integration test
`crates/estante/tests/end_to_end.rs::local_pipeline_yields_loadable_lockfile`
exercises this exact flow and asserts the chain holds end-to-end:
manifest → resolver → lockfile → frost-lisp::load_rc → ShellEnv.
