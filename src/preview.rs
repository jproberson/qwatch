use crate::name::Pattern;
use serde::Deserialize;
use std::path::Path;

pub const MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    #[default]
    Raw,
    Json,
    DelimitedJson,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    #[serde(default)]
    pub format: Format,
    #[serde(default = "tab")]
    pub split: String,
    #[serde(default = "comma")]
    pub field_separator: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub detect: Vec<Detect>,
}

impl Default for Preview {
    fn default() -> Self {
        Self {
            format: Format::Raw,
            split: tab(),
            field_separator: comma(),
            labels: Vec::new(),
            detect: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detect {
    pub pattern: Pattern,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Field { label: String, value: String },
    Text(String),
    Blank,
    Notice(String),
}

pub fn of_file(preview: &Preview, path: &Path) -> Vec<Line> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return vec![Line::Notice(format!(
                "cannot read {}: {error}",
                path.display()
            ))];
        }
    };

    let truncated = bytes.len() > MAX_BYTES;
    let head = &bytes[..bytes.len().min(MAX_BYTES)];
    if head.contains(&0) {
        return vec![Line::Notice(format!("{} is not text", path.display()))];
    }

    let mut lines = render(preview, &String::from_utf8_lossy(head));
    if truncated {
        lines.push(Line::Blank);
        lines.push(Line::Notice(format!("truncated at {MAX_BYTES} bytes")));
    }
    lines
}

pub fn render(preview: &Preview, contents: &str) -> Vec<Line> {
    match preview.format {
        Format::Raw => contents.lines().map(text).collect(),
        Format::Json => as_json(contents),
        Format::DelimitedJson => as_delimited(preview, contents),
    }
}

fn as_delimited(preview: &Preview, contents: &str) -> Vec<Line> {
    let trimmed = contents.trim_end_matches(['\n', '\r']);
    let (head, payload) = match trimmed.split_once(preview.split.as_str()) {
        Some((head, payload)) => (head, Some(payload)),
        None => (trimmed, None),
    };

    let mut lines: Vec<Line> = head
        .split(preview.field_separator.as_str())
        .enumerate()
        .map(|(position, field)| Line::Field {
            label: label_for(preview, position, field),
            value: field.to_string(),
        })
        .collect();

    if let Some(payload) = payload.map(str::trim).filter(|payload| !payload.is_empty()) {
        lines.push(Line::Blank);
        lines.extend(as_json(payload));
    }
    lines
}

fn label_for(preview: &Preview, position: usize, field: &str) -> String {
    if let Some(label) = preview
        .labels
        .get(position)
        .filter(|label| !label.is_empty())
    {
        return label.clone();
    }
    preview
        .detect
        .iter()
        .find(|detect| detect.pattern.is_match(field))
        .map(|detect| detect.label.clone())
        .unwrap_or_else(|| format!("field {}", position + 1))
}

fn as_json(contents: &str) -> Vec<Line> {
    match serde_json::from_str::<serde_json::Value>(contents.trim()) {
        Ok(value) => serde_json::to_string_pretty(&value)
            .unwrap_or_default()
            .lines()
            .map(text)
            .collect(),
        Err(error) => {
            let mut lines = vec![
                Line::Notice(format!("not valid JSON: {error}")),
                Line::Blank,
            ];
            lines.extend(contents.lines().map(text));
            lines
        }
    }
}

fn text(line: &str) -> Line {
    Line::Text(line.to_string())
}

fn tab() -> String {
    "\t".to_string()
}

fn comma() -> String {
    ",".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn delimited() -> Preview {
        toml::from_str(
            r#"
format = "delimited-json"
labels = ["job"]

[[detect]]
pattern = '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
label   = "id"
"#,
        )
        .unwrap()
    }

    fn field(label: &str, value: &str) -> Line {
        Line::Field {
            label: label.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn breaks_a_header_into_labelled_fields() {
        let lines = render(
            &delimited(),
            "RenderReport,0,b27e4ac4-1111-2222-3333-444455556666,False",
        );
        assert_eq!(
            lines,
            [
                field("job", "RenderReport"),
                field("field 2", "0"),
                field("id", "b27e4ac4-1111-2222-3333-444455556666"),
                field("field 4", "False"),
            ]
        );
    }

    #[test]
    fn pretty_prints_the_payload_after_the_split() {
        let lines = render(&delimited(), "RenderReport\t{\"b\":2,\"a\":[1,2]}");
        assert_eq!(
            lines,
            [
                field("job", "RenderReport"),
                Line::Blank,
                Line::Text("{".to_string()),
                Line::Text("  \"a\": [".to_string()),
                Line::Text("    1,".to_string()),
                Line::Text("    2".to_string()),
                Line::Text("  ],".to_string()),
                Line::Text("  \"b\": 2".to_string()),
                Line::Text("}".to_string()),
            ]
        );
    }

    #[test]
    fn sorts_object_keys_so_the_order_is_stable() {
        let lines = render(&delimited(), "job\t{\"zebra\":1,\"apple\":2,\"mango\":3}");
        let keys: Vec<&Line> = lines
            .iter()
            .filter(|line| matches!(line, Line::Text(text) if text.contains('"')))
            .collect();
        assert_eq!(
            keys,
            [
                &Line::Text("  \"apple\": 2,".to_string()),
                &Line::Text("  \"mango\": 3,".to_string()),
                &Line::Text("  \"zebra\": 1".to_string()),
            ]
        );
    }

    #[test]
    fn shows_the_raw_payload_when_it_is_not_valid_json() {
        let lines = render(&delimited(), "job\tnot json at all");
        assert!(matches!(lines[2], Line::Notice(_)));
        assert!(lines.contains(&Line::Text("not json at all".to_string())));
    }

    #[test]
    fn handles_a_header_with_no_payload() {
        let lines = render(&delimited(), "job,0\n");
        assert_eq!(lines, [field("job", "job"), field("field 2", "0")]);
    }

    #[test]
    fn passes_raw_contents_through_untouched() {
        let lines = render(&Preview::default(), "one\ntwo");
        assert_eq!(
            lines,
            [Line::Text("one".to_string()), Line::Text("two".to_string())]
        );
    }

    #[test]
    fn reads_a_file_from_disk() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("job.txt");
        std::fs::write(&path, "RenderReport,0").unwrap();

        assert_eq!(
            of_file(&delimited(), &path),
            [field("job", "RenderReport"), field("field 2", "0")]
        );
    }

    #[test]
    fn refuses_to_render_a_binary_file() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("blob.bin");
        std::fs::write(&path, [0u8, 1, 2, 3]).unwrap();

        assert!(matches!(of_file(&delimited(), &path)[0], Line::Notice(_)));
    }

    #[test]
    fn says_so_when_a_file_is_missing() {
        let lines = of_file(&Preview::default(), Path::new("/nowhere/at/all.txt"));
        assert!(matches!(lines[0], Line::Notice(_)));
    }

    #[test]
    fn truncates_a_very_large_file() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("huge.txt");
        std::fs::write(&path, "x".repeat(MAX_BYTES + 100)).unwrap();

        let lines = of_file(&Preview::default(), &path);
        assert!(matches!(lines.last(), Some(Line::Notice(_))));
    }
}
