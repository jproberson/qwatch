use anyhow::{Context, Result, bail};
use std::process::Command;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn source() -> Option<&'static str> {
    option_env!("CARGO_PKG_REPOSITORY").filter(|url| !url.is_empty())
}

pub fn described() -> String {
    match source() {
        Some(url) => format!("update from {}", url.trim_start_matches("https://")),
        None => "update".to_string(),
    }
}

pub fn run() -> Result<()> {
    let Some(url) = source() else {
        bail!("this build does not know where it came from, so it cannot update itself");
    };

    println!("running cargo install --git {url} --force");
    println!("your config and remembered settings are not touched by this\n");

    let finished = Command::new("cargo")
        .args(["install", "--git", url, "--force"])
        .status()
        .context("could not run cargo. Install Rust from https://rustup.rs and try again")?;

    if !finished.success() {
        bail!("cargo install did not finish, so nothing was replaced");
    }
    println!("\nupdated. start qwatch again to use it");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_knows_where_it_came_from() {
        assert_eq!(source(), Some("https://github.com/jproberson/qwatch"));
    }

    #[test]
    fn it_says_where_it_would_update_from_without_the_scheme() {
        assert_eq!(described(), "update from github.com/jproberson/qwatch");
    }

    #[test]
    fn it_reports_its_own_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(VERSION.split('.').count() >= 2);
    }
}
