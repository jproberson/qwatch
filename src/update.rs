use anyhow::{Context, Result, bail};
use std::process::Command;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub type Version = (u32, u32, u32);

pub fn installed() -> Option<Version> {
    read_version(VERSION)
}

fn read_version(text: &str) -> Option<Version> {
    let parts: Vec<&str> = text.trim().trim_start_matches('v').split('.').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let mut numbers = parts.iter().map(|part| part.parse::<u32>().ok());
    let major = numbers.next()??;
    let minor = numbers.next()??;
    let patch = match numbers.next() {
        Some(given) => given?,
        None => 0,
    };
    Some((major, minor, patch))
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

pub fn newest_released(url: &str) -> Option<Version> {
    let asked = Command::new("git")
        .args(["ls-remote", "--tags", "--refs", url])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    asked.status.success().then_some(())?;

    highest(&String::from_utf8_lossy(&asked.stdout))
}

pub fn highest(advertised: &str) -> Option<Version> {
    advertised
        .lines()
        .filter_map(|line| line.rsplit("refs/tags/").next())
        .filter_map(read_version)
        .max()
}

pub fn behind(installed: Option<Version>, newest: Option<Version>) -> bool {
    match (installed, newest) {
        (Some(here), Some(there)) => there > here,
        _ => false,
    }
}

pub fn look_for_one() -> std::sync::mpsc::Receiver<bool> {
    let (teller, told) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Some(url) = source() else {
            return;
        };
        let _ = teller.send(behind(installed(), newest_released(url)));
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
    fn it_is_behind_only_when_the_newest_is_actually_newer() {
        assert!(behind(Some((0, 1, 1)), Some((0, 1, 2))));
        assert!(behind(Some((0, 1, 9)), Some((0, 2, 0))));
        assert!(!behind(Some((0, 1, 2)), Some((0, 1, 2))));
        assert!(!behind(Some((0, 2, 0)), Some((0, 1, 9))));
        assert!(!behind(None, Some((9, 9, 9))));
        assert!(!behind(Some((0, 1, 1)), None));
    }

    #[test]
    fn it_reads_a_version_off_a_tag_however_it_is_written() {
        assert_eq!(read_version("v0.1.2"), Some((0, 1, 2)));
        assert_eq!(read_version("0.1.2"), Some((0, 1, 2)));
        assert_eq!(read_version("1.10.0"), Some((1, 10, 0)));
        assert_eq!(read_version("2.3"), Some((2, 3, 0)));
    }

    #[test]
    fn it_ignores_a_tag_that_is_not_a_version() {
        assert_eq!(read_version("nightly"), None);
        assert_eq!(read_version("v1.2.3.4"), None);
        assert_eq!(read_version("v1.2.beta"), None);
        assert_eq!(read_version(""), None);
    }

    #[test]
    fn it_takes_the_highest_tag_not_the_last_one_listed() {
        let advertised = "\
aaaa\trefs/tags/v0.1.0
bbbb\trefs/tags/v0.2.0
cccc\trefs/tags/v0.1.9
dddd\trefs/tags/nightly
";
        assert_eq!(highest(advertised), Some((0, 2, 0)));
    }

    #[test]
    fn it_finds_nothing_in_an_empty_advertisement() {
        assert_eq!(highest(""), None);
    }

    #[test]
    fn it_knows_the_version_it_was_built_as() {
        assert_eq!(installed(), read_version(VERSION));
        assert!(installed().is_some());
    }

    #[test]
    fn it_reports_its_own_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(VERSION.split('.').count() >= 2);
    }
}
