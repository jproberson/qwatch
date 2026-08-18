use crate::config::expand_home;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub root: PathBuf,
    pub suffix: Option<String>,
    pub queues: Vec<String>,
    pub states: Vec<String>,
    pub sample: Option<String>,
}

pub fn detect(root: &Path) -> Result<Layout> {
    let directories = subdirectories(root)?;
    let suffix = common_suffix(&directories);

    let (queues, states) = match &suffix {
        Some(suffix) => (
            directories
                .iter()
                .filter(|name| directories.contains(&format!("{name}{suffix}")))
                .cloned()
                .collect(),
            vec!["queued".to_string(), state_name(suffix)],
        ),
        None => (Vec::new(), directories.clone()),
    };

    Ok(Layout {
        sample: first_file(root, &directories),
        root: root.to_path_buf(),
        suffix,
        queues,
        states,
    })
}

fn subdirectories(root: &Path) -> Result<Vec<String>> {
    let listing = std::fs::read_dir(root).with_context(|| format!("reading {}", root.display()))?;
    let mut found: Vec<String> = listing
        .flatten()
        .filter(|item| item.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|item| item.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    found.sort();
    Ok(found)
}

fn common_suffix(directories: &[String]) -> Option<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for outer in directories {
        for inner in directories {
            if inner.len() > outer.len()
                && let Some(suffix) = inner.strip_prefix(outer.as_str())
            {
                *counts.entry(suffix.to_string()).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|(suffix, count)| (*count, suffix.len()))
        .filter(|(_, count)| *count > 0)
        .map(|(suffix, _)| suffix)
}

const ATTENTION: [&str; 9] = [
    "fail",
    "error",
    "dead",
    "reject",
    "quarantine",
    "poison",
    "stuck",
    "retry",
    "invalid",
];

pub fn wants_attention(state: &str) -> bool {
    let lowered = state.to_lowercase();
    ATTENTION.iter().any(|word| lowered.contains(word))
}

fn state_name(suffix: &str) -> String {
    let trimmed = suffix.trim_start_matches(['-', '_', '.']);
    match trimmed.is_empty() {
        true => "failed".to_string(),
        false => trimmed.to_string(),
    }
}

fn first_file(root: &Path, directories: &[String]) -> Option<String> {
    let places =
        std::iter::once(root.to_path_buf()).chain(directories.iter().map(|name| root.join(name)));

    for place in places {
        let Ok(listing) = std::fs::read_dir(&place) else {
            continue;
        };
        let found = listing
            .flatten()
            .filter(|item| item.file_type().is_ok_and(|kind| !kind.is_dir()))
            .map(|item| item.file_name().to_string_lossy().into_owned())
            .find(|name| !name.starts_with('.'));
        if found.is_some() {
            return found;
        }
    }
    None
}

pub fn suggested_name(root: &Path) -> String {
    let raw = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut name = String::new();
    for character in raw.chars() {
        match character.is_ascii_alphanumeric() || character == '_' {
            true => name.push(character),
            false if name.ends_with('-') => {}
            false => name.push('-'),
        }
    }
    let trimmed = name.trim_matches('-');
    match trimmed.is_empty() {
        true => "queues".to_string(),
        false => trimmed.to_string(),
    }
}

pub fn config_text(layout: &Layout, name: &str) -> String {
    let mut out = format!("[profile.{name}]\nroot = \"{}\"\n", shortened(&layout.root));

    match &layout.suffix {
        Some(suffix) => {
            out.push_str(&format!(
                "\n[[profile.{name}.state]]\nname = \"{}\"\ndir  = \"{{queue}}\"\n",
                layout.states[0]
            ));
            out.push_str(&format!(
                "\n[[profile.{name}.state]]\nname     = \"{}\"\ndir      = \"{{queue}}{suffix}\"\npriority = 10\n",
                layout.states[1]
            ));
        }
        None => {
            for state in &layout.states {
                out.push_str(&format!(
                    "\n[[profile.{name}.state]]\nname = \"{state}\"\ndir  = \"{state}\"\n"
                ));
                if wants_attention(state) {
                    out.push_str("priority = 10\n");
                }
            }
        }
    }

    out.push_str(&filename_hint(layout, name));

    out.push_str(&format!(
        "\n[[profile.{name}.action]]\nkey     = \"d\"\nname    = \"delete\"\ntype    = \"delete\"\n"
    ));
    if layout.suffix.is_some() {
        out.push_str(&format!(
            "\n[[profile.{name}.action]]\nkey      = \"r\"\nname     = \"restart\"\ntype     = \"move\"\nto_state = \"{}\"\n",
            layout.states[0]
        ));
    }
    out
}

fn filename_hint(layout: &Layout, name: &str) -> String {
    let sample = match &layout.sample {
        Some(sample) => format!("#   {sample}\n"),
        None => "#   (no files here yet to look at)\n".to_string(),
    };

    format!(
        "\n# Files are listed by name until you describe them. One of yours looks like:\n\
         {sample}#\n\
         # Fill in a pattern with named captures and a template to rebuild a name,\n\
         # then actions can rewrite parts of it. For example:\n\
         #\n\
         # [profile.{name}.filename]\n\
         # pattern  = '^(?<claim>[\\dx])_(?<job>[A-Za-z]\\w*)-(?<index>\\d+)\\.txt$'\n\
         # template = \"{{claim}}_{{job}}-{{index}}.txt\"\n\
         # label    = \"{{job}}\"\n\
         # detail   = \"#{{index}}\"\n"
    )
}

fn shortened(root: &Path) -> String {
    let shown = root.display().to_string();
    let Some(home) = std::env::var_os("HOME") else {
        return shown;
    };
    let home = home.to_string_lossy().into_owned();
    match shown.strip_prefix(&home) {
        Some(rest) => format!("~{rest}"),
        None => shown,
    }
}

pub fn resolve(directory: Option<PathBuf>) -> Result<PathBuf> {
    let root = match directory {
        Some(given) => expand_home(&given.to_string_lossy()),
        None => std::env::current_dir()?,
    };
    root.canonicalize()
        .with_context(|| format!("no such directory: {}", root.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use tempfile::TempDir;

    fn tree(directories: &[&str], files: &[(&str, &str)]) -> TempDir {
        let root = TempDir::new().unwrap();
        for directory in directories {
            std::fs::create_dir_all(root.path().join(directory)).unwrap();
        }
        for (directory, file) in files {
            std::fs::write(root.path().join(directory).join(file), "").unwrap();
        }
        root
    }

    fn parsed(text: &str) -> Config {
        let config: Config = toml::from_str(text).unwrap();
        for profile in config.profile.values() {
            profile.validate().unwrap();
        }
        config
    }

    #[test]
    fn finds_the_suffix_that_pairs_the_most_directories() {
        let root = tree(
            &[
                "invoices",
                "invoices-failed",
                "receipts",
                "receipts-failed",
                "receipts-archive",
            ],
            &[],
        );
        let layout = detect(root.path()).unwrap();
        assert_eq!(layout.suffix.as_deref(), Some("-failed"));
        assert_eq!(layout.queues, ["invoices", "receipts"]);
        assert_eq!(layout.states, ["queued", "failed"]);
    }

    #[test]
    fn is_not_fooled_by_a_queue_whose_name_extends_another() {
        let root = tree(
            &[
                "jobs",
                "jobs-deadletters",
                "jobs-long",
                "jobs-long-deadletters",
                "reports",
                "reports-deadletters",
            ],
            &[],
        );
        let layout = detect(root.path()).unwrap();
        assert_eq!(layout.suffix.as_deref(), Some("-deadletters"));
        assert_eq!(layout.queues, ["jobs", "jobs-long", "reports"]);
        assert_eq!(layout.states[1], "deadletters");
    }

    #[test]
    fn treats_unpaired_subdirectories_as_states_of_one_queue() {
        let root = tree(&["inbox", "processing", "done"], &[]);
        let layout = detect(root.path()).unwrap();
        assert_eq!(layout.suffix, None);
        assert_eq!(layout.states, ["done", "inbox", "processing"]);
    }

    #[test]
    fn picks_up_a_sample_filename_to_show_the_reader() {
        let root = tree(&["inbox"], &[("inbox", "x_Job-1.txt")]);
        assert_eq!(
            detect(root.path()).unwrap().sample.as_deref(),
            Some("x_Job-1.txt")
        );
    }

    #[test]
    fn copes_with_a_directory_holding_nothing() {
        let root = TempDir::new().unwrap();
        let layout = detect(root.path()).unwrap();
        assert_eq!(layout.suffix, None);
        assert!(layout.states.is_empty());
        assert_eq!(layout.sample, None);
    }

    #[test]
    fn writes_a_config_that_actually_loads() {
        let root = tree(
            &["invoices", "invoices-failed"],
            &[("invoices", "x_Job-1.txt")],
        );
        let layout = detect(root.path()).unwrap();
        let config = parsed(&config_text(&layout, "ingest"));

        let profile = &config.profile["ingest"];
        assert_eq!(profile.state.len(), 2);
        assert_eq!(profile.action.len(), 2);
    }

    #[test]
    fn writes_a_loadable_config_for_unpaired_directories_too() {
        let root = tree(&["inbox", "failed"], &[]);
        let layout = detect(root.path()).unwrap();
        let config = parsed(&config_text(&layout, "jobs"));

        assert_eq!(config.profile["jobs"].state.len(), 2);
        assert_eq!(config.profile["jobs"].action.len(), 1);
    }

    #[test]
    fn offers_restart_only_where_there_is_somewhere_to_restart_from() {
        let paired = detect(tree(&["a", "a-failed"], &[]).path()).unwrap();
        assert!(config_text(&paired, "p").contains("restart"));

        let flat = detect(tree(&["inbox"], &[]).path()).unwrap();
        assert!(!config_text(&flat, "p").contains("restart"));
    }

    #[test]
    fn shows_the_sample_filename_as_a_comment_not_as_config() {
        let root = tree(&["inbox"], &[("inbox", "x_Job-1.txt")]);
        let text = config_text(&detect(root.path()).unwrap(), "p");
        assert!(text.contains("#   x_Job-1.txt"));
        parsed(&text);
    }

    #[test]
    fn flags_a_failure_looking_state_so_it_sorts_and_colours_first() {
        let root = tree(&["inbox", "processing", "failed"], &[]);
        let config = parsed(&config_text(&detect(root.path()).unwrap(), "jobs"));

        let states = &config.profile["jobs"].state;
        let failed = states.iter().find(|state| state.name == "failed").unwrap();
        let inbox = states.iter().find(|state| state.name == "inbox").unwrap();
        assert!(failed.priority > inbox.priority);
    }

    #[test]
    fn recognises_the_usual_words_for_a_failure_directory() {
        assert!(wants_attention("failed"));
        assert!(wants_attention("deadletters"));
        assert!(wants_attention("Errors"));
        assert!(wants_attention("quarantine"));
        assert!(!wants_attention("inbox"));
        assert!(!wants_attention("processing"));
    }

    #[test]
    fn names_a_profile_after_its_directory() {
        assert_eq!(suggested_name(Path::new("/tmp/ingest")), "ingest");
        assert_eq!(suggested_name(Path::new("/tmp/queue_two")), "queue_two");
    }

    #[test]
    fn beats_a_directory_name_into_something_toml_accepts_as_a_key() {
        assert_eq!(suggested_name(Path::new("/tmp/.tmpAb12")), "tmpAb12");
        assert_eq!(suggested_name(Path::new("/tmp/queue.v2")), "queue-v2");
        assert_eq!(suggested_name(Path::new("/tmp/my queues")), "my-queues");
        assert_eq!(suggested_name(Path::new("/tmp/a  b")), "a-b");
        assert_eq!(suggested_name(Path::new("/")), "queues");
        assert_eq!(suggested_name(Path::new("/tmp/...")), "queues");
    }

    #[test]
    fn writes_loadable_config_however_odd_the_directory_is_called() {
        for awkward in [".hidden", "queue.v2", "with space", "-leading"] {
            let root = TempDir::new().unwrap();
            let directory = root.path().join(awkward);
            std::fs::create_dir(&directory).unwrap();
            std::fs::create_dir(directory.join("pending")).unwrap();

            let layout = detect(&directory).unwrap();
            parsed(&config_text(&layout, &suggested_name(&directory)));
        }
    }
}
