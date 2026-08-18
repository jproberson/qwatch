use crate::config::{Ignore, Profile, State, in_display_order};
use crate::name::{Captures, DirTemplate, ROOT_QUEUE};
use crate::status::{self, Resolved};
use anyhow::{Context, Result, bail};
use serde::{Serialize, Serializer};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const ROOT_STATE: &str = "files";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub path: PathBuf,
    pub file_name: String,
    pub queue: String,
    pub state: String,
    pub status: String,
    pub badge: Option<String>,
    pub label: String,
    pub detail: Option<String>,
    pub captures: Option<Captures>,
    #[serde(serialize_with = "epoch_seconds")]
    pub modified: SystemTime,
}

fn epoch_seconds<S: Serializer>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error> {
    let elapsed = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    serializer.serialize_u64(elapsed.as_secs())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StateFiles {
    pub state: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Queue {
    pub name: String,
    pub states: Vec<StateFiles>,
}

impl Queue {
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.states.iter().flat_map(|state| state.entries.iter())
    }

    pub fn file_count(&self) -> usize {
        self.entries().count()
    }
}

pub fn scan(profile: &Profile) -> Result<Vec<Queue>> {
    let root = &profile.root;
    if !root.exists() {
        bail!("the queue root does not exist: {}", root.display());
    }
    if !root.is_dir() {
        bail!("the queue root is not a directory: {}", root.display());
    }
    let directories = subdirectories(root, &profile.ignore)?;

    if profile.state.is_empty() && directories.is_empty() {
        return Ok(vec![whole_root(profile)]);
    }

    let states = match profile.state.is_empty() {
        true => derived_states(&directories)?,
        false => profile.state.clone(),
    };

    let queues: BTreeSet<String> = directories
        .iter()
        .filter_map(|directory| owning_state(&states, directory))
        .map(|(_, queue)| queue)
        .collect();

    Ok(queues
        .into_iter()
        .map(|queue| build_queue(profile, &states, queue))
        .collect())
}

fn build_queue(profile: &Profile, states: &[State], queue: String) -> Queue {
    let name = display_name(profile, &queue);
    let states = in_display_order(states)
        .into_iter()
        .map(|state| StateFiles {
            state: state.name.clone(),
            entries: entries_in(
                &profile.root.join(state.dir.directory_for(&queue)),
                profile,
                &name,
                &state.name,
            ),
        })
        .collect();

    Queue { name, states }
}

fn whole_root(profile: &Profile) -> Queue {
    let name = display_name(profile, ROOT_QUEUE);
    Queue {
        states: vec![StateFiles {
            state: ROOT_STATE.to_string(),
            entries: entries_in(&profile.root, profile, &name, ROOT_STATE),
        }],
        name,
    }
}

fn display_name(profile: &Profile, queue: &str) -> String {
    if queue != ROOT_QUEUE {
        return queue.to_string();
    }
    profile
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| profile.root.display().to_string())
}

fn owning_state<'a>(states: &'a [State], directory: &str) -> Option<(&'a State, String)> {
    states
        .iter()
        .filter_map(|state| {
            state
                .dir
                .queue_of(directory)
                .map(|queue| (state, queue.to_string()))
        })
        .max_by_key(|(state, _)| state.dir.specificity())
}

fn derived_states(directories: &[String]) -> Result<Vec<State>> {
    directories
        .iter()
        .map(|directory| {
            Ok(State {
                name: directory.clone(),
                dir: DirTemplate::parse(directory)?,
                priority: 0,
            })
        })
        .collect()
}

fn subdirectories(root: &Path, ignore: &Ignore) -> Result<Vec<String>> {
    let listing = std::fs::read_dir(root).with_context(|| format!("reading {}", root.display()))?;
    let mut found: Vec<String> = listing
        .flatten()
        .filter(|item| item.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|item| item.file_name().to_string_lossy().into_owned())
        .filter(|name| !ignore.skips(name))
        .collect();
    found.sort();
    Ok(found)
}

fn entries_in(directory: &Path, profile: &Profile, queue: &str, state: &str) -> Vec<Entry> {
    let Ok(listing) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = listing
        .flatten()
        .filter(|item| item.file_type().is_ok_and(|kind| !kind.is_dir()))
        .filter_map(|item| build_entry(item.path(), profile, queue, state))
        .collect();
    entries.sort_by(|left, right| {
        left.modified
            .cmp(&right.modified)
            .then_with(|| left.file_name.cmp(&right.file_name))
    });
    entries
}

fn build_entry(path: PathBuf, profile: &Profile, queue: &str, state: &str) -> Option<Entry> {
    let file_name = path.file_name()?.to_string_lossy().into_owned();
    if profile.ignore.skips(&file_name) {
        return None;
    }

    let captures = profile
        .filename
        .as_ref()
        .and_then(|filename| filename.pattern.captures(&file_name));
    let (label, detail) = describe(profile, &file_name, captures.as_ref());
    let Resolved {
        name: status,
        badge,
    } = status::resolve(profile, state, captures.as_ref());

    Some(Entry {
        file_name,
        queue: queue.to_string(),
        state: state.to_string(),
        status,
        badge,
        label,
        detail,
        captures,
        modified: path
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH),
        path,
    })
}

fn describe(
    profile: &Profile,
    file_name: &str,
    captures: Option<&Captures>,
) -> (String, Option<String>) {
    match (&profile.filename, captures) {
        (Some(filename), Some(captures)) => (
            filename.label.render(captures),
            filename
                .detail
                .as_ref()
                .map(|detail| detail.render(captures)),
        ),
        _ => (file_name.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const INGEST: &str = r##"
root = "/replaced/by/the/test"

[[state]]
name = "queued"
dir  = "{queue}"

[[state]]
name     = "failed"
dir      = "{queue}-failed"
priority = 10

[filename]
pattern  = '^(?<claim>[\dxX])_(?<source>\w+)_(?<stamp>.+)-(?<job>[A-Za-z][\w.]*)-(?<index>\d+)\.txt$'
template = "{claim}_{source}_{stamp}-{job}-{index}.txt"
label    = "{job}"
detail   = "#{index}"

[[status]]
name  = "failed"
state = "failed"

[[status]]
name  = "running"
state = "queued"
when  = { claim = '^\d+$' }
badge = "worker {claim}"

[[status]]
name  = "waiting"
state = "queued"
"##;

    fn tree(spec: &[(&str, &[&str])]) -> TempDir {
        let root = TempDir::new().unwrap();
        let mut position = 0;
        for (directory, files) in spec {
            let path = root.path().join(directory);
            std::fs::create_dir_all(&path).unwrap();
            for file in *files {
                crate::testing::write_in_order(&path.join(file), "", position);
                position += 1;
            }
        }
        root
    }

    fn ingest_profile(root: &Path) -> Profile {
        let mut profile: Profile = toml::from_str(INGEST).unwrap();
        profile.validate().unwrap();
        profile.root = root.to_path_buf();
        profile
    }

    fn name(job: &str) -> String {
        format!("x_worker_2026-08-05T23_42_16-{job}-0.txt")
    }

    fn claimed(server: u32, job: &str) -> String {
        format!("{server}_worker_2026-08-05T23_42_16-{job}-1.txt")
    }

    type Layout = Vec<(String, Vec<(String, Vec<String>)>)>;

    fn layout(queues: &[Queue]) -> Layout {
        queues
            .iter()
            .map(|queue| {
                (
                    queue.name.clone(),
                    queue
                        .states
                        .iter()
                        .map(|state| {
                            (
                                state.state.clone(),
                                state
                                    .entries
                                    .iter()
                                    .map(|entry| entry.label.clone())
                                    .collect(),
                            )
                        })
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn pairs_a_queue_with_its_failure_sibling() {
        let root = tree(&[
            ("invoices", &[]),
            ("invoices-failed", &[]),
            ("receipts", &[&name("RenderReport")]),
            ("receipts-failed", &[&name("ParseInvoice")]),
        ]);
        let queues = scan(&ingest_profile(root.path())).unwrap();

        assert_eq!(
            layout(&queues),
            [
                (
                    "invoices".to_string(),
                    vec![
                        ("failed".to_string(), vec![]),
                        ("queued".to_string(), vec![]),
                    ]
                ),
                (
                    "receipts".to_string(),
                    vec![
                        ("failed".to_string(), vec!["ParseInvoice".to_string()]),
                        ("queued".to_string(), vec!["RenderReport".to_string()]),
                    ]
                ),
            ]
        );
    }

    #[test]
    fn never_reads_a_failure_directory_as_a_queue_of_its_own() {
        let root = tree(&[("invoices", &[]), ("invoices-failed", &[])]);
        let queues = scan(&ingest_profile(root.path())).unwrap();
        assert_eq!(
            queues.iter().map(|queue| &queue.name).collect::<Vec<_>>(),
            ["invoices"]
        );
    }

    #[test]
    fn lists_a_queue_whose_failure_directory_does_not_exist_yet() {
        let root = tree(&[("statements", &[&name("RenderReport")])]);
        let queues = scan(&ingest_profile(root.path())).unwrap();
        assert_eq!(queues.len(), 1);
        assert_eq!(queues[0].states.len(), 2);
        assert_eq!(queues[0].file_count(), 1);
    }

    #[test]
    fn resolves_status_and_detail_from_the_filename() {
        let root = tree(&[(
            "invoices",
            &[&name("RenderReport"), &claimed(3, "ParseInvoice")],
        )]);
        let queues = scan(&ingest_profile(root.path())).unwrap();
        let queued = &queues[0].states[1];

        let statuses: Vec<(&str, Option<&str>, Option<&str>)> = queued
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.status.as_str(),
                    entry.badge.as_deref(),
                    entry.detail.as_deref(),
                )
            })
            .collect();
        assert!(statuses.contains(&("waiting", None, Some("#0"))));
        assert!(statuses.contains(&("running", Some("worker 3"), Some("#1"))));
    }

    #[test]
    fn calls_an_unrecognised_filename_unknown_and_labels_it_verbatim() {
        let root = tree(&[("invoices", &["notes.txt"])]);
        let queues = scan(&ingest_profile(root.path())).unwrap();
        let entry = queues[0].entries().next().unwrap();
        assert_eq!(entry.status, "unknown");
        assert_eq!(entry.label, "notes.txt");
        assert_eq!(entry.detail, None);
    }

    #[test]
    fn skips_ignored_and_hidden_files() {
        let root = tree(&[("invoices", &[".DS_Store", ".hidden", &name("Real")])]);
        let mut profile = ingest_profile(root.path());
        profile.ignore.names = vec![".DS_Store".to_string()];
        let queues = scan(&profile).unwrap();
        assert_eq!(queues[0].file_count(), 1);
    }

    #[test]
    fn treats_every_subdirectory_as_a_state_when_no_profile_declares_any() {
        let root = tree(&[("inbox", &["one.json"]), ("failed", &["two.json"])]);
        let queues = scan(&Profile::for_directory(root.path())).unwrap();

        assert_eq!(queues.len(), 1);
        assert_eq!(
            queues[0]
                .states
                .iter()
                .map(|state| state.state.as_str())
                .collect::<Vec<_>>(),
            ["failed", "inbox"]
        );
        assert_eq!(queues[0].file_count(), 2);
    }

    #[test]
    fn treats_a_directory_of_plain_files_as_one_state() {
        let root = tree(&[("", &["one.json", "two.json"])]);
        let queues = scan(&Profile::for_directory(root.path())).unwrap();

        assert_eq!(queues.len(), 1);
        assert_eq!(queues[0].states.len(), 1);
        assert_eq!(queues[0].states[0].state, ROOT_STATE);
        assert_eq!(queues[0].file_count(), 2);
    }

    #[test]
    fn names_the_root_queue_after_the_root_directory() {
        let root = tree(&[("", &["one.json"])]);
        let queues = scan(&Profile::for_directory(root.path())).unwrap();
        let expected = root.path().file_name().unwrap().to_string_lossy();
        assert_eq!(queues[0].name, expected);
    }

    #[test]
    fn reports_a_root_that_does_not_exist() {
        let profile = Profile::for_directory(Path::new("/nowhere/at/all"));
        let message = scan(&profile).unwrap_err().to_string();
        assert_eq!(message, "the queue root does not exist: /nowhere/at/all");
    }

    #[test]
    fn reports_a_root_that_is_a_file() {
        let root = tree(&[("", &["not-a-directory"])]);
        let profile = Profile::for_directory(&root.path().join("not-a-directory"));
        let message = scan(&profile).unwrap_err().to_string();
        assert!(
            message.starts_with("the queue root is not a directory"),
            "{message}"
        );
    }
}
