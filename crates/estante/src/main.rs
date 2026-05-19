//! `estante` — cargo for shell.
//!
//! See repo README + CLAUDE.md for the full picture. This binary is
//! the thin CLI shell; everything load-bearing lives in `estante::*`
//! and `estante-types` so subcommand wiring is mechanical and the
//! tested surface stays in libraries.

#![forbid(unsafe_code)]

use clap::{Parser, Subcommand};
use estante::{actions, config};

#[derive(Parser, Debug)]
#[command(
    name = "estante",
    version,
    about = "Cargo for shell. Nix-native, git-as-registry, Rust + Tatara-Lisp.",
    long_about = "Manages typed shell-package libraries (aliases, prompts, hooks, completions, keybinds) declared via (defshellpkg …) and consumed via (defload …) in frost-lisp."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Path to the manifest file. Defaults to `./shellpkg.lisp`.
    #[arg(long, global = true, default_value = "shellpkg.lisp")]
    manifest: std::path::PathBuf,

    /// Path to the lockfile. Defaults to `./shellpkg.lock.lisp`.
    #[arg(long, global = true, default_value = "shellpkg.lock.lisp")]
    lockfile: std::path::PathBuf,

    /// GitHub access token. Falls back to `$GITHUB_TOKEN` then to
    /// unauthenticated access (subject to the 60 req/h public limit).
    #[arg(long, global = true, env = "GITHUB_TOKEN")]
    github_token: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Write a starter `shellpkg.lisp` in the current directory.
    Init {
        /// Package name. Defaults to the basename of cwd.
        #[arg(long)]
        name: Option<String>,
        /// Package kind. `library` (default) is the standard set of
        /// behavior forms. `binary` scaffolds a one-shot tool. `daemon`
        /// scaffolds a long-running service.
        #[arg(long, value_enum, default_value_t = estante::actions::init::Kind::Library)]
        kind: estante::actions::init::Kind,
        /// Compat target. `frost` (default) emits a tatara-lisp
        /// rc.lisp entrypoint. `vanilla` emits POSIX shell
        /// init.{bash,zsh} entrypoints — frost-free. `both` does both.
        #[arg(long, value_enum, default_value_t = estante::actions::init::Compat::Frost)]
        compat: estante::actions::init::Compat,
    },
    /// Add a `(defshellpkg …)` entry to the manifest.
    Add {
        /// Source URL: `github:owner/repo[@ref]` etc.
        source: String,
        /// Override the package name (default: derived from source).
        #[arg(long)]
        name: Option<String>,
        /// Version label. Defaults to the ref portion of `source` or
        /// `"HEAD"` if no ref was given.
        #[arg(long)]
        version: Option<String>,
    },
    /// Resolve the manifest's deps + emit a deterministic lockfile.
    Lock,
    /// Fetch + materialize every locked entry into the local cache.
    Install,
    /// Search for shell packages by `topic:estante-pkg` + query.
    Search {
        /// Search query (matches repo names + descriptions).
        query: String,
        /// Limit results.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Print the materialized rc.lisp source frost-lisp would see.
    Expand,
    /// Dry-run the resolver — no I/O side effects.
    Validate,
    /// Print the cache directory + auth status.
    Info,
    /// Emit the lockfile in an alternate format (lisp / nix / json).
    /// `--format nix` produces a `shellpkg.lock.nix` consumed by
    /// `substrate/lib/build/estante/`.
    Export {
        /// Output format.
        #[arg(long, value_enum, default_value_t = estante::actions::export::Format::Lisp)]
        format: estante::actions::export::Format,
        /// Write to this file instead of stdout.
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// uv-like ad-hoc run: parse the script's inline metadata,
    /// materialize its declared deps via the resolver, launch a shell
    /// that loads the deps + the script. Supports `.lisp` (frost),
    /// `.bash`/`.sh` (bash), `.zsh` (zsh).
    Run {
        /// Path to the script source.
        script: std::path::PathBuf,
        /// Override the shell to exec. Default: derived from the
        /// script's extension.
        #[arg(long)]
        shell: Option<String>,
    },
    /// uv-like persistent install: wrap a script (and its deps) as a
    /// installable binary via a generated Nix derivation. Operator
    /// installs via `nix profile install path:<cache>/tools/<name>`.
    #[command(subcommand)]
    Tool(ToolCommand),
    /// Run the package's test suite. Discovers `*_test.{bash,zsh,sh,lisp}`
    /// files under `--dir` (default: `./tests`) and executes each in a
    /// fresh shell. The test battery shipped with `estante-stdlib`
    /// provides describe/it/expect_* matchers and the summary line.
    Test {
        /// Directory containing test files. Default: ./tests.
        #[arg(long, default_value = "tests")]
        dir: std::path::PathBuf,
        /// Substring filter applied to file names.
        #[arg(long)]
        filter: Option<String>,
    },
    /// Shift a package's placement substrate — move bytes between
    /// estante's user-local cache and the Nix store. `--to nix`
    /// promotes via `nix store add-path` (requires nix on PATH);
    /// `--to cache` flags the entry for re-resolution on next
    /// `estante install`. Idempotent.
    Place {
        /// Package name to shift. Mutually exclusive with `--all`.
        name: Option<String>,
        /// Shift every entry in the lockfile.
        #[arg(long)]
        all: bool,
        /// Target placement.
        #[arg(long, value_enum)]
        to: estante::actions::place::Target,
    },
}

#[derive(Subcommand, Debug)]
enum ToolCommand {
    /// Install a script as a persistent binary.
    Install {
        /// Path to the script source.
        script: std::path::PathBuf,
        /// Override the installed binary name (default: script's
        /// `provides:` metadata or its file stem).
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove a previously installed tool from the cache.
    Uninstall {
        /// Tool name.
        name: String,
    },
    /// List installed tools.
    List,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("estante=info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let cfg = config::Config::resolve(cli.github_token.clone())?;

    match cli.command {
        Command::Init { name, kind, compat } => {
            actions::init::run(&cli.manifest, name, kind, compat).await
        }
        Command::Add {
            source,
            name,
            version,
        } => actions::add::run(&cli.manifest, &source, name, version).await,
        Command::Lock => actions::lock::run(&cli.manifest, &cli.lockfile, &cfg).await,
        Command::Install => actions::install::run(&cli.lockfile, &cfg).await,
        Command::Search { query, limit } => actions::search::run(&query, limit, &cfg).await,
        Command::Expand => actions::expand::run(&cli.lockfile).await,
        Command::Validate => actions::validate::run(&cli.manifest, &cli.lockfile).await,
        Command::Info => actions::info::run(&cfg).await,
        Command::Export { format, output } => {
            actions::export::run(&cli.lockfile, format, output.as_deref()).await
        }
        Command::Run { script, shell } => actions::run::run(&script, &cfg, shell).await,
        Command::Tool(sub) => match sub {
            ToolCommand::Install { script, name } => {
                actions::tool::install(&script, &cfg, name).await
            }
            ToolCommand::Uninstall { name } => actions::tool::uninstall(&name, &cfg).await,
            ToolCommand::List => actions::tool::list(&cfg).await,
        },
        Command::Test { dir, filter } => actions::test::run(&dir, filter).await,
        Command::Place { name, all, to } => {
            actions::place::run(&cli.lockfile, name, all, to).await
        }
    }
}
