use anyhow::{Context, Result, bail};
use std::process::Command;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn installed_commit() -> Option<&'static str> {
    option_env!("QWATCH_COMMIT").filter(|sha| sha.len() >= 7)
}

pub fn source() -> Option<&'static str> {
    option_env!("CARGO_PKG_REPOSITORY").filter(|url| !url.is_empty())
}

pub fn described() -> String {
    match source() {
        Some(url) => format!("update from {}", url.trim_start_matches("https://")),
        None => "update".to_string(),
    }
}

pub fn newest_commit(url: &str) -> Option<String> {
    let asked = Command::new("git")
        .args(["ls-remote", url, "HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !asked.status.success() {
        return None;
    }
    String::from_utf8_lossy(&asked.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
}

pub fn behind(installed: Option<&str>, newest: Option<&str>) -> bool {
    match (installed, newest) {
        (Some(here), Some(there)) => here != there,
        _ => false,
    }
}

pub fn look_for_one() -> std::sync::mpsc::Receiver<bool> {
    let (teller, told) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Some(url) = source() else {
            return;
        };
        let newest = newest_commit(url);
        let _ = teller.send(behind(installed_commit(), newest.as_deref()));
    });
    told
}

pub fn run() -> Result<()> {
    let Some(url) = source() else {
        bail!("this build does not know where it came from, so it cannot update itself");
    };

    println!("running cargo install --git {url} --force");
    println!("this replaces the program only. Your config is left alone\n");

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
    fn it_is_only_behind_when_it_knows_both_ends() {
        assert!(behind(Some("aaaa"), Some("bbbb")));
        assert!(!behind(Some("aaaa"), Some("aaaa")));
        assert!(!behind(None, Some("bbbb")));
        assert!(!behind(Some("aaaa"), None));
        assert!(!behind(None, None));
    }

    #[test]
    fn a_build_from_a_checkout_knows_its_commit() {
        let sha = installed_commit().expect("no commit recorded");
        assert_eq!(sha.len(), 40, "not a full sha: {sha}");
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn it_reports_its_own_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(VERSION.split('.').count() >= 2);
    }
}
