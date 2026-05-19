# Security Policy

## Supported Versions

The published `main` branch is the only supported version surface
during the M1 phase. Tagged releases (`v0.x.x`) will inherit the
support window once `feira publish` ships them.

## Reporting

Open a private security advisory at
[github.com/pleme-io/estante/security/advisories/new](https://github.com/pleme-io/estante/security/advisories/new)
or email `security@pleme.io` if the advisory surface is unavailable.
Do **not** file a public GitHub issue for security-sensitive reports.

We'll acknowledge within five business days and either ship a fix
under a new patch release or document a mitigation in
`docs/security.md`.

## Threat model — what's in scope

`estante` runs as an operator-facing CLI; its threat surface is:

1. **Manifest / lockfile parsing** — A malicious `shellpkg.lisp` from
   a fetched dependency must not be able to compromise the local
   process. Parser hardening for tatara-lisp's `compile_typed` is
   tracked upstream in `pleme-io/tatara`.
2. **Tarball unpack** — `tar` + `flate2` are invoked in-process
   (NO SHELL). Zip-slip / symlink-escape from a malicious tarball
   would land here. We rely on `tar`'s built-in safety; report any
   bypass that lands a file outside the cache `store/<name>-<rev>/`
   prefix.
3. **Github tokens** — The PAT used for octocrab discovery is fetched
   from `$GITHUB_TOKEN` (M1d will migrate to `cofre::SecretRef`).
   The token NEVER lands in argv, stdout, lockfile entries, or any
   emitted Lisp form.
4. **Content-address integrity** — The lockfile's `:blake3` is the
   tameshi-chain attestation receipt. A drift between the recorded
   digest and the on-disk tree should be reported.

## Out of scope (M1)

- Resolution of typosquat domains (the `:source` URL is taken as
  declared; consumer responsible for vetting before adding).
- Cross-process attacks on the local cache (filesystem ACLs assumed
  intact).
- Supply-chain attacks on the upstream tatara-lisp / frost-lisp
  crates (reported upstream).

## See also

- `pleme-io/cofre` — typed secret materialization.
- `pleme-io/tameshi` — content-address attestation chain.
