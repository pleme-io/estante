//! `estante test` — discover + run a package's test suite.
//!
//! Walks `tests/` (configurable via `--dir`) and exec's each
//! `*_test.bash` / `*_test.zsh` / `*_test.lisp` file in a fresh
//! subshell. The test battery in `estante-stdlib`'s `test.bash`
//! (or equivalent) handles per-test isolation, matcher output, and
//! the RSpec-shape summary.
//!
//! The CLI's job is the FIND + DISPATCH + AGGREGATE part: walk the
//! directory, route each file to the right runtime, sum the exit
//! codes, return non-zero if any test file failed.

use std::path::{Path, PathBuf};

use anyhow::Context;

pub async fn run(test_dir: &Path, filter: Option<String>) -> anyhow::Result<()> {
    if !test_dir.exists() {
        anyhow::bail!(
            "test directory {} does not exist — create it and put `*_test.{{bash,zsh,lisp}}` files there",
            test_dir.display()
        );
    }
    let mut files = discover(test_dir)?;
    files.sort();
    if let Some(pattern) = &filter {
        files.retain(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().contains(pattern))
                .unwrap_or(false)
        });
    }
    if files.is_empty() {
        println!("No test files matched.");
        return Ok(());
    }

    let mut passed_files = 0_usize;
    let mut failed_files = 0_usize;

    for file in &files {
        println!("\n─── {}", file.display());
        let runtime = detect_runtime(file);
        let status = tokio::process::Command::new(runtime)
            .arg(file)
            .status()
            .await
            .with_context(|| format!("running test file {}", file.display()))?;
        if status.success() {
            passed_files += 1;
        } else {
            failed_files += 1;
        }
    }

    println!(
        "\n{} file(s) total — {} passed, {} failed",
        files.len(),
        passed_files,
        failed_files,
    );
    if failed_files > 0 {
        anyhow::bail!("{failed_files} test file(s) failed");
    }
    Ok(())
}

fn discover(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with("_test.bash")
            || name.ends_with("_test.zsh")
            || name.ends_with("_test.sh")
            || name.ends_with("_test.lisp")
        {
            out.push(path);
        }
    }
    Ok(out)
}

fn detect_runtime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "zsh" => "zsh",
        "lisp" => "frost",
        _ => "bash",
    }
}
