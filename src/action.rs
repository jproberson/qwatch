use crate::config::{Action, ActionKind, Profile};
use crate::scan::Entry;
use anyhow::{Context, Result, bail};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Change(Change),
    Open(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Move { from: PathBuf, to: PathBuf },
    Delete { path: PathBuf },
}

pub fn plan(profile: &Profile, action: &Action, entry: &Entry) -> Result<Plan> {
    let root = profile
        .root
        .canonicalize()
        .with_context(|| format!("resolving {}", profile.root.display()))?;
    let source = inside_root(&root, &entry.path)
        .with_context(|| format!("refusing to touch {}", entry.file_name))?;

    match action.kind {
        ActionKind::Edit => Ok(Plan::Open(source)),
        ActionKind::Delete => Ok(Plan::Change(Change::Delete { path: source })),
        ActionKind::Move => Ok(Plan::Change(planned_move(
            profile, &root, action, entry, source,
        )?)),
    }
}

pub fn apply(change: &Change) -> Result<()> {
    match change {
        Change::Delete { path } => {
            std::fs::remove_file(path).with_context(|| format!("deleting {}", path.display()))
        }
        Change::Move { from, to } => {
            let parent = to.parent().context("a move needs a destination directory")?;
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
            std::fs::rename(from, to)
                .with_context(|| format!("moving {} to {}", from.display(), to.display()))
        }
    }
}

impl Plan {
    pub fn describe(&self, root: &Path) -> String {
        match self {
            Plan::Open(path) => format!("open {}", file_name_of(path)),
            Plan::Change(Change::Delete { path }) => format!("delete {}", file_name_of(path)),
            Plan::Change(Change::Move { from, to }) => describe_move(from, to, root),
        }
    }
}

fn describe_move(from: &Path, to: &Path, root: &Path) -> String {
    let (before, after) = (file_name_of(from), file_name_of(to));

    if from.parent() == to.parent() {
        return format!("rename {before} to {after}");
    }

    let destination = to.parent().unwrap_or(to);
    let shown = destination
        .strip_prefix(root)
        .unwrap_or(destination)
        .display();

    if before == after {
        return format!("move {before} into {shown}");
    }
    format!("move {before} into {shown} as {after}")
}

fn planned_move(
    profile: &Profile,
    root: &Path,
    action: &Action,
    entry: &Entry,
    source: PathBuf,
) -> Result<Change> {
    let state = action
        .to_state
        .as_deref()
        .context("this action moves but names no state")?;
    let directory = profile
        .state
        .iter()
        .find(|declared| declared.name == state)
        .with_context(|| format!("no state named {state:?}"))?
        .dir
        .directory_for(&entry.queue);

    let target = normalized(&root.join(directory).join(renamed(profile, action, entry)?));
    if !target.starts_with(root) {
        bail!(
            "refusing to write outside {}: {}",
            root.display(),
            target.display()
        );
    }
    if target == source {
        bail!(
            "{} is already where {} would put it",
            entry.file_name,
            action.name
        );
    }
    if target.exists() {
        bail!("{} already exists", target.display());
    }
    Ok(Change::Move {
        from: source,
        to: target,
    })
}

fn renamed(profile: &Profile, action: &Action, entry: &Entry) -> Result<String> {
    if action.set.is_empty() {
        return Ok(entry.file_name.clone());
    }
    let filename = profile
        .filename
        .as_ref()
        .context("this action rewrites the filename but no pattern describes one")?;
    let captures = entry.captures.as_ref().with_context(|| {
        format!(
            "cannot {} {}: its name is not in the expected format",
            action.name, entry.file_name
        )
    })?;

    let mut rewritten = captures.clone();
    rewritten.extend(
        action
            .set
            .iter()
            .map(|(capture, value)| (capture.clone(), value.clone())),
    );
    Ok(filename.template.render(&rewritten))
}

fn inside_root(root: &Path, path: &Path) -> Result<PathBuf> {
    let resolved = path
        .canonicalize()
        .with_context(|| format!("{} is already gone", path.display()))?;

    if !resolved.starts_with(root) {
        bail!("{} resolves outside {}", path.display(), root.display());
    }
    Ok(resolved)
}

fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            component => out.push(component),
        }
    }
    out
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan;
    use tempfile::TempDir;

    const PROFILE: &str = r##"
root = "/replaced/by/the/test"

[[state]]
name = "queued"
dir  = "{queue}"

[[state]]
name     = "failed"
dir      = "{queue}-failed"
priority = 10

[filename]
pattern  = '^(?<claim>[\dxX])_(?<job>[A-Za-z]\w*)-(?<index>\d+)\.txt$'
template = "{claim}_{job}-{index}.txt"
label    = "{job}"

[[status]]
name  = "failed"
state = "failed"

[[status]]
name  = "waiting"
state = "queued"

[[action]]
key      = "r"
name     = "restart"
type     = "move"
to_state = "queued"
set      = { claim = "x" }

[[action]]
key  = "d"
name = "delete"
type = "delete"

[[action]]
key  = "e"
name = "edit"
type = "edit"
"##;

    struct Fixture {
        root: TempDir,
        profile: Profile,
    }

    impl Fixture {
        fn new(spec: &[(&str, &[&str])]) -> Self {
            let root = TempDir::new().unwrap();
            for (directory, files) in spec {
                let path = root.path().join(directory);
                std::fs::create_dir_all(&path).unwrap();
                for file in *files {
                    std::fs::write(path.join(file), "payload").unwrap();
                }
            }
            let mut profile: Profile = toml::from_str(PROFILE).unwrap();
            profile.validate().unwrap();
            profile.root = root.path().to_path_buf();
            Self { root, profile }
        }

        fn entry(&self, file_name: &str) -> Entry {
            scan::scan(&self.profile)
                .unwrap()
                .iter()
                .flat_map(scan::Queue::entries)
                .find(|entry| entry.file_name == file_name)
                .unwrap_or_else(|| panic!("no entry named {file_name}"))
                .clone()
        }

        fn action(&self, name: &str) -> Action {
            self.profile
                .action
                .iter()
                .find(|action| action.name == name)
                .unwrap()
                .clone()
        }

        fn plan(&self, action: &str, file_name: &str) -> Result<Plan> {
            plan(&self.profile, &self.action(action), &self.entry(file_name))
        }

        fn names_in(&self, directory: &str) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(self.root.path().join(directory))
                .map(|listing| {
                    listing
                        .flatten()
                        .map(|item| item.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        }
    }

    #[test]
    fn moves_a_dead_file_back_into_its_queue_unclaimed() {
        let fixture = Fixture::new(&[
            ("invoices", &[]),
            ("invoices-failed", &["3_ParseInvoice-1.txt"]),
        ]);
        let Plan::Change(change) = fixture.plan("restart", "3_ParseInvoice-1.txt").unwrap() else {
            panic!("restart should change the filesystem");
        };
        assert_eq!(
            change,
            Change::Move {
                from: fixture
                    .root
                    .path()
                    .canonicalize()
                    .unwrap()
                    .join("invoices-failed/3_ParseInvoice-1.txt"),
                to: fixture
                    .root
                    .path()
                    .canonicalize()
                    .unwrap()
                    .join("invoices/x_ParseInvoice-1.txt"),
            }
        );

        apply(&change).unwrap();
        assert_eq!(fixture.names_in("invoices"), ["x_ParseInvoice-1.txt"]);
        assert!(fixture.names_in("invoices-failed").is_empty());
    }

    #[test]
    fn releases_a_claimed_file_where_it_stands() {
        let fixture = Fixture::new(&[("invoices", &["3_ParseInvoice-1.txt"])]);
        let plan = fixture.plan("restart", "3_ParseInvoice-1.txt").unwrap();
        assert_eq!(
            plan.describe(fixture.root.path()),
            "rename 3_ParseInvoice-1.txt to x_ParseInvoice-1.txt"
        );

        let Plan::Change(change) = plan else {
            panic!("restart should change the filesystem");
        };
        apply(&change).unwrap();
        assert_eq!(fixture.names_in("invoices"), ["x_ParseInvoice-1.txt"]);
    }

    #[test]
    fn refuses_to_restart_a_file_already_waiting_to_be_picked_up() {
        let fixture = Fixture::new(&[("invoices", &["x_ParseInvoice-1.txt"])]);
        let message = fixture
            .plan("restart", "x_ParseInvoice-1.txt")
            .unwrap_err()
            .to_string();
        assert_eq!(
            message,
            "x_ParseInvoice-1.txt is already where restart would put it"
        );
    }

    #[test]
    fn refuses_to_restart_a_name_it_cannot_read() {
        let fixture = Fixture::new(&[("invoices", &[]), ("invoices-failed", &["notes.txt"])]);
        let message = fixture.plan("restart", "notes.txt").unwrap_err().to_string();
        assert!(message.contains("not in the expected format"), "{message}");
    }

    #[test]
    fn refuses_to_overwrite_a_file_of_the_target_name() {
        let fixture = Fixture::new(&[
            ("invoices", &["x_ParseInvoice-1.txt"]),
            ("invoices-failed", &["3_ParseInvoice-1.txt"]),
        ]);
        let message = fixture
            .plan("restart", "3_ParseInvoice-1.txt")
            .unwrap_err()
            .to_string();
        assert!(message.contains("already exists"), "{message}");
    }

    #[test]
    fn deletes_a_file_it_could_not_parse() {
        let fixture = Fixture::new(&[("invoices", &["stray-notes.txt"])]);
        let Plan::Change(change) = fixture.plan("delete", "stray-notes.txt").unwrap() else {
            panic!("delete should change the filesystem");
        };
        apply(&change).unwrap();
        assert!(fixture.names_in("invoices").is_empty());
    }

    #[test]
    fn opens_without_changing_anything() {
        let fixture = Fixture::new(&[("invoices", &["x_ParseInvoice-1.txt"])]);
        let plan = fixture.plan("edit", "x_ParseInvoice-1.txt").unwrap();
        assert!(matches!(plan, Plan::Open(_)));
        assert_eq!(plan.describe(fixture.root.path()), "open x_ParseInvoice-1.txt");
    }

    #[test]
    fn refuses_a_file_that_resolves_outside_the_root() {
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "").unwrap();

        let fixture = Fixture::new(&[("invoices", &[])]);
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            fixture.root.path().join("invoices/x_Escape-0.txt"),
        )
        .unwrap();

        let message = fixture
            .plan("delete", "x_Escape-0.txt")
            .unwrap_err()
            .to_string();
        assert!(message.contains("refusing to touch"), "{message}");
    }

    #[test]
    fn reports_a_file_that_vanished_before_the_action_ran() {
        let fixture = Fixture::new(&[("invoices", &["x_ParseInvoice-1.txt"])]);
        let entry = fixture.entry("x_ParseInvoice-1.txt");
        std::fs::remove_file(&entry.path).unwrap();

        let message = plan(&fixture.profile, &fixture.action("delete"), &entry)
            .unwrap_err()
            .to_string();
        assert!(message.contains("refusing to touch"), "{message}");
    }

    #[test]
    fn planning_alone_touches_nothing() {
        let fixture = Fixture::new(&[
            ("invoices", &[]),
            ("invoices-failed", &["3_ParseInvoice-1.txt"]),
        ]);
        fixture.plan("restart", "3_ParseInvoice-1.txt").unwrap();
        fixture.plan("delete", "3_ParseInvoice-1.txt").unwrap();

        assert!(fixture.names_in("invoices").is_empty());
        assert_eq!(fixture.names_in("invoices-failed"), ["3_ParseInvoice-1.txt"]);
    }

    #[test]
    fn describes_a_move_that_only_changes_directory() {
        let fixture = Fixture::new(&[
            ("invoices", &[]),
            ("invoices-failed", &["x_ParseInvoice-1.txt"]),
        ]);
        let root = fixture.root.path().canonicalize().unwrap();
        let description = fixture
            .plan("restart", "x_ParseInvoice-1.txt")
            .unwrap()
            .describe(&root);
        assert_eq!(description, "move x_ParseInvoice-1.txt into invoices");
    }
}
