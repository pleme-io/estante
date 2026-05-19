//! `estante search <query>` — discover packages via the GitHub
//! topic-search surface (`topic:estante-pkg <query>`).

use crate::config::Config;
use crate::fetch;

pub async fn run(query: &str, limit: u32, cfg: &Config) -> anyhow::Result<()> {
    let client = fetch::build_client(cfg.github_token.as_deref())?;
    let q = format!("topic:estante-pkg {query}");
    let page = client
        .search()
        .repositories(&q)
        .per_page(u8::try_from(limit.min(100)).unwrap_or(20))
        .send()
        .await?;
    if page.items.is_empty() {
        println!("No estante-pkg repositories match {query:?}.");
        return Ok(());
    }
    println!("{} result(s) for `topic:estante-pkg {query}`:", page.items.len());
    for repo in page.items {
        let owner = repo
            .owner
            .as_ref()
            .map(|o| o.login.as_str())
            .unwrap_or("?");
        let desc = repo.description.as_deref().unwrap_or("");
        println!("  github:{owner}/{}   {desc}", repo.name);
    }
    Ok(())
}
