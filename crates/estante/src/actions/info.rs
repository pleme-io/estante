//! `estante info` — print resolved config (cache dir, auth status).
//! No secrets ever rendered.

use crate::config::Config;

pub async fn run(cfg: &Config) -> anyhow::Result<()> {
    println!("estante {}", env!("CARGO_PKG_VERSION"));
    println!("cache-dir: {}", cfg.cache_dir.display());
    println!(
        "github-token: {}",
        if cfg.has_token() {
            "set (length redacted)"
        } else {
            "unset (public-rate-limited access)"
        }
    );
    Ok(())
}
