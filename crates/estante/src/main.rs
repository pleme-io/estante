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
        Command::Init { name } => actions::init::run(&cli.manifest, name).await,
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
    }
}
