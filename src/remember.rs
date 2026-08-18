use crate::config::Layout;
use crate::keys::Binding;
use crate::ui::table::Order;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Remembered {
    pub root: Option<PathBuf>,
    pub layout: Option<Layout>,
    pub sort: Option<Order>,
    pub watching: Option<bool>,
    pub keys: BTreeMap<String, Vec<String>>,
}

impl Remembered {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub fn bindings(&self, motion: &str) -> Option<Vec<Binding>> {
        let written = self.keys.get(motion)?;
        written
            .iter()
            .map(|key| Binding::parse(key).ok())
            .collect::<Option<Vec<_>>>()
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Book {
    pub entries: BTreeMap<String, Remembered>,
}

impl Book {
    pub fn read(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("writing remembered settings")?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }

    pub fn of(&self, name: &str) -> Remembered {
        self.entries.get(name).cloned().unwrap_or_default()
    }

    pub fn keep(&mut self, name: &str, remembered: Remembered) {
        match remembered.is_empty() {
            true => {
                self.entries.remove(name);
            }
            false => {
                self.entries.insert(name.to_string(), remembered);
            }
        }
    }
}

pub fn beside(config: &Path) -> PathBuf {
    config.with_file_name("remembered.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn a_missing_file_remembers_nothing_rather_than_failing() {
        let book = Book::read(Path::new("/nowhere/at/all.toml"));
        assert!(book.of("anything").is_empty());
    }

    #[test]
    fn what_is_written_comes_back() {
        let home = TempDir::new().unwrap();
        let path = home.path().join("nested").join("remembered.toml");

        let mut book = Book::default();
        book.keep(
            "ingest",
            Remembered {
                root: None,
                layout: Some(Layout::ByStatus),
                sort: Some(Order::Age),
                watching: Some(false),
                keys: BTreeMap::from([("down".to_string(), vec!["n".to_string()])]),
            },
        );
        book.write(&path).unwrap();

        let read = Book::read(&path).of("ingest");
        assert_eq!(read.layout, Some(Layout::ByStatus));
        assert_eq!(read.sort, Some(Order::Age));
        assert_eq!(read.watching, Some(false));
        assert_eq!(read.bindings("down").unwrap().len(), 1);
    }

    #[test]
    fn a_root_survives_a_round_trip() {
        let home = TempDir::new().unwrap();
        let path = home.path().join("remembered.toml");

        let mut book = Book::default();
        book.keep(
            "ingest",
            Remembered {
                root: Some(PathBuf::from("/somewhere/else")),
                ..Default::default()
            },
        );
        book.write(&path).unwrap();
        assert_eq!(
            Book::read(&path).of("ingest").root,
            Some(PathBuf::from("/somewhere/else"))
        );
    }

    #[test]
    fn one_profile_does_not_disturb_another() {
        let home = TempDir::new().unwrap();
        let path = home.path().join("remembered.toml");

        let mut book = Book::default();
        book.keep(
            "one",
            Remembered {
                layout: Some(Layout::ByQueue),
                ..Default::default()
            },
        );
        book.keep(
            "two",
            Remembered {
                sort: Some(Order::Status),
                ..Default::default()
            },
        );
        book.write(&path).unwrap();

        let read = Book::read(&path);
        assert_eq!(read.of("one").layout, Some(Layout::ByQueue));
        assert_eq!(read.of("one").sort, None);
        assert_eq!(read.of("two").sort, Some(Order::Status));
    }

    #[test]
    fn remembering_nothing_takes_the_profile_back_out() {
        let mut book = Book::default();
        book.keep(
            "gone",
            Remembered {
                layout: Some(Layout::Table),
                ..Default::default()
            },
        );
        book.keep("gone", Remembered::default());
        assert!(book.entries.is_empty());
    }

    #[test]
    fn a_binding_nobody_can_parse_is_ignored_rather_than_fatal() {
        let remembered = Remembered {
            keys: BTreeMap::from([("down".to_string(), vec!["wibble".to_string()])]),
            ..Default::default()
        };
        assert!(remembered.bindings("down").is_none());
    }

    #[test]
    fn it_sits_beside_the_config_it_belongs_to() {
        assert_eq!(
            beside(Path::new("/home/me/.config/qwatch/config.toml")),
            PathBuf::from("/home/me/.config/qwatch/remembered.toml")
        );
    }
}
