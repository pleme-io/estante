//! Subcommand handlers — one module per `estante` subcommand. Each
//! exposes a `pub async fn run(...) -> anyhow::Result<()>` consumed by
//! `main.rs`. Keeping handlers thin (parse args, route to a library
//! call) keeps the tested surface in `estante-types` + `resolver` +
//! `fetch`.

pub mod add;
pub mod expand;
pub mod export;
pub mod info;
pub mod init;
pub mod install;
pub mod lock;
pub mod run;
pub mod search;
pub mod test;
pub mod tool;
pub mod validate;
