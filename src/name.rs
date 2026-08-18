use anyhow::{Result, bail};
use regex::Regex;
use serde::Deserialize;
use std::collections::BTreeMap;

pub type Captures = BTreeMap<String, String>;

pub const ROOT_QUEUE: &str = "";
pub const QUEUE_PLACEHOLDER: &str = "queue";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Capture(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct NameTemplate {
    segments: Vec<Segment>,
}

impl NameTemplate {
    pub fn parse(source: &str) -> Result<Self> {
        let mut segments = Vec::new();
        let mut literal = String::new();
        let mut characters = source.chars().peekable();

        while let Some(character) = characters.next() {
            match character {
                '{' if characters.peek() == Some(&'{') => {
                    characters.next();
                    literal.push('{');
                }
                '}' if characters.peek() == Some(&'}') => {
                    characters.next();
                    literal.push('}');
                }
                '{' => {
                    if !literal.is_empty() {
                        segments.push(Segment::Literal(std::mem::take(&mut literal)));
                    }
                    segments.push(Segment::Capture(read_placeholder(
                        &mut characters,
                        source,
                    )?));
                }
                '}' => bail!("unmatched }} in template {source:?}"),
                character => literal.push(character),
            }
        }

        if !literal.is_empty() {
            segments.push(Segment::Literal(literal));
        }
        Ok(Self { segments })
    }

    pub fn render(&self, values: &Captures) -> String {
        let mut rendered = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Literal(text) => rendered.push_str(text),
                Segment::Capture(name) => {
                    if let Some(value) = values.get(name) {
                        rendered.push_str(value);
                    }
                }
            }
        }
        rendered
    }

    pub fn placeholders(&self) -> impl Iterator<Item = &str> {
        self.segments.iter().filter_map(|segment| match segment {
            Segment::Capture(name) => Some(name.as_str()),
            Segment::Literal(_) => None,
        })
    }
}

fn read_placeholder(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
    source: &str,
) -> Result<String> {
    let mut name = String::new();
    for character in characters {
        if character == '}' {
            if name.is_empty() {
                bail!("empty placeholder in template {source:?}");
            }
            return Ok(name);
        }
        name.push(character);
    }
    bail!("unclosed placeholder in template {source:?}")
}

impl TryFrom<String> for NameTemplate {
    type Error = anyhow::Error;

    fn try_from(source: String) -> Result<Self> {
        Self::parse(&source)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct DirTemplate {
    prefix: String,
    suffix: String,
    per_queue: bool,
}

impl DirTemplate {
    pub fn parse(source: &str) -> Result<Self> {
        let template = NameTemplate::parse(source)?;
        let mut prefix = String::new();
        let mut suffix = String::new();
        let mut per_queue = false;

        for segment in &template.segments {
            match segment {
                Segment::Literal(text) if per_queue => suffix.push_str(text),
                Segment::Literal(text) => prefix.push_str(text),
                Segment::Capture(name) if name != QUEUE_PLACEHOLDER => bail!(
                    "a state directory may only use {{{QUEUE_PLACEHOLDER}}}, found {{{name}}} in {source:?}"
                ),
                Segment::Capture(_) if per_queue => bail!(
                    "a state directory may only use {{{QUEUE_PLACEHOLDER}}} once: {source:?}"
                ),
                Segment::Capture(_) => per_queue = true,
            }
        }

        if !per_queue && prefix.is_empty() {
            bail!("a state directory name cannot be empty");
        }
        Ok(Self {
            prefix,
            suffix,
            per_queue,
        })
    }

    pub fn queue_of<'a>(&self, directory: &'a str) -> Option<&'a str> {
        if !self.per_queue {
            return (directory == self.prefix).then_some(ROOT_QUEUE);
        }
        let queue = directory
            .strip_prefix(&self.prefix)?
            .strip_suffix(&self.suffix)?;
        (!queue.is_empty()).then_some(queue)
    }

    pub fn directory_for(&self, queue: &str) -> String {
        if !self.per_queue {
            return self.prefix.clone();
        }
        format!("{}{queue}{}", self.prefix, self.suffix)
    }

    pub fn specificity(&self) -> usize {
        self.prefix.len() + self.suffix.len()
    }
}

impl TryFrom<String> for DirTemplate {
    type Error = anyhow::Error;

    fn try_from(source: String) -> Result<Self> {
        Self::parse(&source)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "String")]
pub struct Pattern(Regex);

impl Pattern {
    pub fn parse(source: &str) -> Result<Self> {
        Ok(Self(Regex::new(source)?))
    }

    pub fn captures(&self, text: &str) -> Option<Captures> {
        let matched = self.0.captures(text)?;
        Some(
            self.names()
                .filter_map(|name| {
                    let value = matched.name(name)?;
                    Some((name.to_string(), value.as_str().to_string()))
                })
                .collect(),
        )
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.0.is_match(text)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.capture_names().flatten()
    }
}

impl TryFrom<String> for Pattern {
    type Error = anyhow::Error;

    fn try_from(source: String) -> Result<Self> {
        Self::parse(&source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captures(pairs: &[(&str, &str)]) -> Captures {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn renders_placeholders_from_captures() {
        let template = NameTemplate::parse("{claim}_{source}-{job}.txt").unwrap();
        let rendered = template.render(&captures(&[
            ("claim", "x"),
            ("source", "worker"),
            ("job", "RenderReport"),
        ]));
        assert_eq!(rendered, "x_worker-RenderReport.txt");
    }

    #[test]
    fn renders_a_missing_capture_as_nothing() {
        let template = NameTemplate::parse("{claim}_{missing}end").unwrap();
        assert_eq!(template.render(&captures(&[("claim", "x")])), "x_end");
    }

    #[test]
    fn treats_doubled_braces_as_literal() {
        let template = NameTemplate::parse("{{literal}} {queue}").unwrap();
        assert_eq!(
            template.render(&captures(&[("queue", "invoices")])),
            "{literal} invoices"
        );
    }

    #[test]
    fn rejects_a_malformed_template() {
        assert!(NameTemplate::parse("{unclosed").is_err());
        assert!(NameTemplate::parse("closed}").is_err());
        assert!(NameTemplate::parse("{}").is_err());
    }

    #[test]
    fn lists_the_placeholders_it_uses() {
        let template = NameTemplate::parse("{a}-{b}-{a}").unwrap();
        assert_eq!(template.placeholders().collect::<Vec<_>>(), ["a", "b", "a"]);
    }

    #[test]
    fn reads_the_queue_out_of_a_suffixed_directory() {
        let template = DirTemplate::parse("{queue}-failed").unwrap();
        assert_eq!(template.queue_of("invoices-failed"), Some("invoices"));
        assert_eq!(template.queue_of("receipts-failed"), Some("receipts"));
        assert_eq!(template.queue_of("invoices"), None);
        assert_eq!(template.queue_of("-failed"), None);
    }

    #[test]
    fn reads_the_queue_out_of_a_bare_directory() {
        let template = DirTemplate::parse("{queue}").unwrap();
        assert_eq!(template.queue_of("invoices"), Some("invoices"));
        assert_eq!(template.queue_of("invoices-failed"), Some("invoices-failed"));
    }

    #[test]
    fn matches_a_fixed_directory_to_the_root_queue() {
        let template = DirTemplate::parse("failed").unwrap();
        assert_eq!(template.queue_of("failed"), Some(ROOT_QUEUE));
        assert_eq!(template.queue_of("failed-extra"), None);
    }

    #[test]
    fn ranks_a_longer_literal_as_more_specific() {
        let bare = DirTemplate::parse("{queue}").unwrap();
        let suffixed = DirTemplate::parse("{queue}-failed").unwrap();
        let fixed = DirTemplate::parse("failed").unwrap();
        assert!(suffixed.specificity() > bare.specificity());
        assert!(fixed.specificity() > bare.specificity());
    }

    #[test]
    fn builds_the_directory_for_a_queue() {
        assert_eq!(
            DirTemplate::parse("{queue}-failed")
                .unwrap()
                .directory_for("invoices"),
            "invoices-failed"
        );
        assert_eq!(
            DirTemplate::parse("failed").unwrap().directory_for("ignored"),
            "failed"
        );
    }

    #[test]
    fn rejects_a_state_directory_with_the_wrong_placeholder() {
        assert!(DirTemplate::parse("{state}-failed").is_err());
        assert!(DirTemplate::parse("{queue}-{queue}").is_err());
        assert!(DirTemplate::parse("").is_err());
    }

    #[test]
    fn collects_named_groups_only() {
        let pattern = Pattern::parse(r"^(?<claim>[\dxX])_(\w+)_(?<job>\w+)$").unwrap();
        let found = pattern.captures("x_worker_RenderReport").unwrap();
        assert_eq!(found, captures(&[("claim", "x"), ("job", "RenderReport")]));
        assert!(pattern.captures("nonsense").is_none());
    }

    #[test]
    fn round_trips_a_job_filename() {
        let pattern = Pattern::parse(
            r"^(?<claim>[\dxX])_(?<source>\w+)_(?<stamp>.+)-(?<job>[A-Za-z][\w.]*)-(?<index>\d+)\.txt$",
        )
        .unwrap();
        let template =
            NameTemplate::parse("{claim}_{source}_{stamp}-{job}-{index}.txt").unwrap();
        let original = "3_worker_2026-08-05T23_42_168340860-05_00-ParseInvoice-1.txt";

        let mut found = pattern.captures(original).unwrap();
        assert_eq!(template.render(&found), original);

        found.insert("claim".to_string(), "x".to_string());
        assert_eq!(
            template.render(&found),
            "x_worker_2026-08-05T23_42_168340860-05_00-ParseInvoice-1.txt"
        );
    }
}
