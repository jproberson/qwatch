use crate::config::Profile;
use crate::scan::Queue;
use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

pub const DEBOUNCE: Duration = Duration::from_millis(120);
pub const BACKSTOP: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy)]
pub struct Settings {
    pub debounce: Duration,
    pub backstop: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            debounce: DEBOUNCE,
            backstop: BACKSTOP,
        }
    }
}

pub struct Watch {
    ticks: Receiver<()>,
    _watcher: RecommendedWatcher,
}

impl Watch {
    pub fn ticks(&self) -> &Receiver<()> {
        &self.ticks
    }
}

pub fn targets(profile: &Profile, queues: &[Queue]) -> Vec<PathBuf> {
    let mut found = vec![profile.root.clone()];

    for queue in queues {
        for state in &profile.state {
            found.push(profile.root.join(state.dir.directory_for(&queue.name)));
        }
    }
    for entry in queues.iter().flat_map(Queue::entries) {
        if let Some(parent) = entry.path.parent() {
            found.push(parent.to_path_buf());
        }
    }
    if profile.state.is_empty()
        && let Ok(listing) = std::fs::read_dir(&profile.root)
    {
        found.extend(
            listing
                .flatten()
                .filter(|item| item.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|item| item.path()),
        );
    }

    found.retain(|path| path.is_dir());
    found.sort();
    found.dedup();
    found
}

pub fn start(directories: &[PathBuf], settings: Settings) -> Result<Watch> {
    let (raw_sender, raw) = channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            let _ = raw_sender.send(());
        }
    })
    .context("starting the filesystem watcher")?;

    for directory in directories {
        watcher
            .watch(directory, RecursiveMode::NonRecursive)
            .with_context(|| format!("watching {}", directory.display()))?;
    }

    Ok(Watch {
        ticks: coalesced(raw, settings),
        _watcher: watcher,
    })
}

fn coalesced(raw: Receiver<()>, settings: Settings) -> Receiver<()> {
    let (sender, ticks) = channel();
    std::thread::spawn(move || relay(raw, sender, settings));
    ticks
}

enum Waited {
    Event,
    Backstop,
    Closed,
}

fn relay(raw: Receiver<()>, ticks: Sender<()>, settings: Settings) {
    loop {
        let still_open = match wait(&raw, settings.backstop) {
            Waited::Closed => return,
            Waited::Backstop => true,
            Waited::Event => settle(&raw, settings.debounce),
        };
        if ticks.send(()).is_err() || !still_open {
            return;
        }
    }
}

fn wait(raw: &Receiver<()>, backstop: Duration) -> Waited {
    if backstop.is_zero() {
        return match raw.recv() {
            Ok(()) => Waited::Event,
            Err(_) => Waited::Closed,
        };
    }
    match raw.recv_timeout(backstop) {
        Ok(()) => Waited::Event,
        Err(RecvTimeoutError::Timeout) => Waited::Backstop,
        Err(RecvTimeoutError::Disconnected) => Waited::Closed,
    }
}

fn settle(raw: &Receiver<()>, debounce: Duration) -> bool {
    loop {
        match raw.recv_timeout(debounce) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => return true,
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan;
    use std::sync::mpsc::Sender;
    use tempfile::TempDir;

    const QUICK: Duration = Duration::from_millis(40);
    const PATIENT: Duration = Duration::from_secs(5);

    fn feeding(settings: Settings) -> (Sender<()>, Receiver<()>) {
        let (sender, raw) = channel();
        (sender, coalesced(raw, settings))
    }

    fn bursty() -> Settings {
        Settings {
            debounce: QUICK,
            backstop: Duration::ZERO,
        }
    }

    #[test]
    fn collapses_a_burst_into_one_tick() {
        let (sender, ticks) = feeding(bursty());
        for _ in 0..5 {
            sender.send(()).unwrap();
        }

        assert!(ticks.recv_timeout(PATIENT).is_ok());
        assert!(ticks.recv_timeout(QUICK * 5).is_err());
    }

    #[test]
    fn ticks_once_per_burst_when_bursts_are_separated() {
        let (sender, ticks) = feeding(bursty());

        sender.send(()).unwrap();
        assert!(ticks.recv_timeout(PATIENT).is_ok());
        sender.send(()).unwrap();
        assert!(ticks.recv_timeout(PATIENT).is_ok());
    }

    #[test]
    fn keeps_ticking_on_the_backstop_when_nothing_happens() {
        let (_sender, ticks) = feeding(Settings {
            debounce: QUICK,
            backstop: QUICK * 2,
        });

        assert!(ticks.recv_timeout(PATIENT).is_ok());
        assert!(ticks.recv_timeout(PATIENT).is_ok());
    }

    #[test]
    fn stays_silent_without_a_backstop() {
        let (_sender, ticks) = feeding(bursty());
        assert!(ticks.recv_timeout(QUICK * 5).is_err());
    }

    #[test]
    fn delivers_a_final_tick_when_the_source_closes_mid_burst() {
        let (sender, ticks) = feeding(bursty());
        sender.send(()).unwrap();
        drop(sender);

        assert!(ticks.recv_timeout(PATIENT).is_ok());
        assert!(ticks.recv_timeout(PATIENT).is_err());
    }

    #[test]
    fn notices_a_file_appearing_in_a_watched_directory() {
        let root = TempDir::new().unwrap();
        std::fs::create_dir(root.path().join("queued")).unwrap();

        let watch = start(
            &[root.path().to_path_buf(), root.path().join("queued")],
            bursty(),
        )
        .unwrap();
        std::fs::write(root.path().join("queued/arrived.json"), "").unwrap();

        assert!(watch.ticks().recv_timeout(PATIENT).is_ok());
    }

    #[test]
    fn watches_the_root_and_every_state_directory_that_exists() {
        let root = TempDir::new().unwrap();
        for directory in ["invoices", "invoices-failed", "receipts"] {
            std::fs::create_dir(root.path().join(directory)).unwrap();
        }

        let mut profile: Profile = toml::from_str(
            r#"
root = "/replaced/by/the/test"
[[state]]
name = "queued"
dir = "{queue}"
[[state]]
name = "failed"
dir = "{queue}-failed"
"#,
        )
        .unwrap();
        profile.root = root.path().to_path_buf();

        let queues = scan::scan(&profile).unwrap();
        let watched = targets(&profile, &queues);

        assert!(watched.contains(&root.path().to_path_buf()));
        assert!(watched.contains(&root.path().join("invoices")));
        assert!(watched.contains(&root.path().join("invoices-failed")));
        assert!(watched.contains(&root.path().join("receipts")));
        assert!(!watched.contains(&root.path().join("receipts-failed")));
    }

    #[test]
    fn watches_every_subdirectory_when_no_states_are_declared() {
        let root = TempDir::new().unwrap();
        for directory in ["inbox", "failed"] {
            std::fs::create_dir(root.path().join(directory)).unwrap();
        }

        let profile = Profile::for_directory(root.path());
        let queues = scan::scan(&profile).unwrap();
        let watched = targets(&profile, &queues);

        assert!(watched.contains(&root.path().join("inbox")));
        assert!(watched.contains(&root.path().join("failed")));
    }
}
