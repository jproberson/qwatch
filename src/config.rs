use crate::keys::{Binding, Keys};
use crate::name::{DirTemplate, NameTemplate, Pattern};
use crate::preview::Preview;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub profile: BTreeMap<String, Profile>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        for (name, profile) in &config.profile {
            profile
                .validate()
                .with_context(|| format!("in profile {name:?}"))?;
        }
        Ok(config)
    }

    pub fn select(&self, requested: Option<&str>) -> Result<&Profile> {
        let Some(name) = requested
            .or(self.default_profile.as_deref())
            .or_else(|| self.only_profile_name())
        else {
            bail!(
                "no profile asked for and no default_profile set.{}",
                self.offer()
            );
        };
        self.profile
            .get(name)
            .with_context(|| format!("no profile named {name:?}.{}", self.offer()))
    }

    fn offer(&self) -> String {
        match self.profile.is_empty() {
            true => " the config file declares none".to_string(),
            false => format!(
                " try one of: {}",
                self.profile.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        }
    }

    fn only_profile_name(&self) -> Option<&str> {
        match self.profile.len() {
            1 => self.profile.keys().next().map(String::as_str),
            _ => None,
        }
    }
}

pub fn default_config_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home()?.join(".config"),
    };
    Some(base.join("qwatch").join("config.toml"))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(deserialize_with = "expanded_path")]
    pub root: PathBuf,
    #[serde(default)]
    pub state: Vec<State>,
    #[serde(default)]
    pub filename: Option<Filename>,
    #[serde(default)]
    pub status: Vec<Status>,
    #[serde(default)]
    pub action: Vec<Action>,
    #[serde(default)]
    pub preview: Preview,
    #[serde(default)]
    pub ignore: Ignore,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub keys: Keys,
    #[serde(default = "yes")]
    pub mouse: bool,
    #[serde(default)]
    pub watch: Watch,
}

impl Profile {
    pub fn for_directory(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            state: Vec::new(),
            filename: None,
            status: Vec::new(),
            action: Vec::new(),
            preview: Preview::default(),
            ignore: Ignore::default(),
            layout: Layout::default(),
            keys: Keys::default(),
            mouse: true,
            watch: Watch::default(),
        }
    }

    pub fn states_in_display_order(&self) -> Vec<&State> {
        in_display_order(&self.state)
    }

    pub fn declares(&self, state: &str) -> bool {
        self.state.iter().any(|declared| declared.name == state)
    }

    pub fn validate(&self) -> Result<()> {
        self.validate_states()?;
        self.validate_filename()?;
        self.validate_statuses()?;
        self.validate_actions()
    }

    fn validate_states(&self) -> Result<()> {
        let mut seen = BTreeSet::new();
        for state in &self.state {
            if state.name.is_empty() {
                bail!("a state needs a name");
            }
            if !seen.insert(&state.name) {
                bail!("two states are both named {:?}", state.name);
            }
        }
        Ok(())
    }

    fn validate_filename(&self) -> Result<()> {
        let Some(filename) = &self.filename else {
            return Ok(());
        };
        let known: BTreeSet<&str> = filename.pattern.names().collect();
        let templates = [
            ("template", Some(&filename.template)),
            ("label", Some(&filename.label)),
            ("detail", filename.detail.as_ref()),
        ];
        for (field, template) in templates {
            for placeholder in template.into_iter().flat_map(NameTemplate::placeholders) {
                if !known.contains(placeholder) {
                    bail!(
                        "filename.{field} uses {{{placeholder}}}, which the pattern never captures"
                    );
                }
            }
        }
        Ok(())
    }

    fn validate_statuses(&self) -> Result<()> {
        let known = self.capture_names();
        for status in &self.status {
            if let Some(state) = &status.state
                && !self.declares(state)
            {
                bail!(
                    "status {:?} names an undeclared state {state:?}",
                    status.name
                );
            }
            for capture in status.when.keys() {
                if !known.contains(capture.as_str()) {
                    bail!(
                        "status {:?} tests {{{capture}}}, which no filename pattern captures",
                        status.name
                    );
                }
            }
            for placeholder in status.badge.iter().flat_map(NameTemplate::placeholders) {
                if !known.contains(placeholder) {
                    bail!(
                        "the badge of status {:?} uses {{{placeholder}}}, which no filename pattern captures",
                        status.name
                    );
                }
            }
        }
        Ok(())
    }

    fn validate_actions(&self) -> Result<()> {
        let known = self.capture_names();
        let mut taken: Vec<Binding> = Vec::new();
        for action in &self.action {
            let binding =
                Binding::parse(&action.key).with_context(|| format!("action {:?}", action.name))?;
            if self.keys.claims(binding) {
                bail!(
                    "action {:?} binds {:?}, which is reserved for navigation",
                    action.name,
                    action.key
                );
            }
            if taken.contains(&binding) {
                bail!("two actions are both bound to {:?}", action.key);
            }
            taken.push(binding);
            match action.kind {
                ActionKind::Move => match &action.to_state {
                    None => bail!("action {:?} moves, so it needs a to_state", action.name),
                    Some(state) if !self.declares(state) => bail!(
                        "action {:?} moves to an undeclared state {state:?}",
                        action.name
                    ),
                    Some(_) => {}
                },
                _ if action.to_state.is_some() => bail!(
                    "action {:?} does not move, so to_state means nothing",
                    action.name
                ),
                _ if !action.set.is_empty() => bail!(
                    "action {:?} does not move, so set means nothing",
                    action.name
                ),
                _ => {}
            }
            if action.kind == ActionKind::Edit && action.scope != Scope::One {
                bail!(
                    "action {:?} opens an editor, so it can only work on one file",
                    action.name
                );
            }
            for capture in action.set.keys() {
                if !known.contains(capture.as_str()) {
                    bail!(
                        "action {:?} sets {{{capture}}}, which no filename pattern captures",
                        action.name
                    );
                }
            }
        }
        Ok(())
    }

    fn capture_names(&self) -> BTreeSet<&str> {
        self.filename
            .iter()
            .flat_map(|filename| filename.pattern.names())
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub name: String,
    pub dir: DirTemplate,
    #[serde(default)]
    pub priority: i32,
}

pub fn in_display_order(states: &[State]) -> Vec<&State> {
    let mut ordered: Vec<(usize, &State)> = states.iter().enumerate().collect();
    ordered.sort_by_key(|(declared, state)| (-state.priority, *declared));
    ordered.into_iter().map(|(_, state)| state).collect()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filename {
    pub pattern: Pattern,
    pub template: NameTemplate,
    pub label: NameTemplate,
    #[serde(default)]
    pub detail: Option<NameTemplate>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub name: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub when: BTreeMap<String, Pattern>,
    #[serde(default)]
    pub badge: Option<NameTemplate>,
    #[serde(default)]
    pub color: Option<StatusColor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum StatusColor {
    Plain,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl StatusColor {
    pub fn parse(source: &str) -> Result<Self> {
        let named = source.trim().to_lowercase();

        if let Some(hex) = named.strip_prefix('#') {
            if hex.len() != 6 {
                bail!("a hex colour needs six digits, got {source:?}");
            }
            let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16);
            return Ok(Self::Rgb(
                channel(0).map_err(|_| bad_colour(source))?,
                channel(2).map_err(|_| bad_colour(source))?,
                channel(4).map_err(|_| bad_colour(source))?,
            ));
        }
        if let Ok(index) = named.parse::<u8>() {
            return Ok(Self::Indexed(index));
        }
        Ok(Self::Indexed(match named.as_str() {
            "plain" | "none" | "default" => return Ok(Self::Plain),
            "black" => 0,
            "red" => 1,
            "green" => 2,
            "yellow" => 3,
            "blue" => 4,
            "magenta" => 5,
            "cyan" => 6,
            "white" => 7,
            "gray" | "grey" => 8,
            "brightred" => 9,
            "brightgreen" => 10,
            "brightyellow" => 11,
            "brightblue" => 12,
            "brightmagenta" => 13,
            "brightcyan" => 14,
            "brightwhite" => 15,
            "orange" => 208,
            "amber" => 215,
            _ => return Err(bad_colour(source)),
        }))
    }
}

fn bad_colour(source: &str) -> anyhow::Error {
    anyhow::anyhow!("unknown colour {source:?}: use a name, a number from 0 to 255, or #rrggbb")
}

impl TryFrom<String> for StatusColor {
    type Error = anyhow::Error;

    fn try_from(source: String) -> Result<Self> {
        Self::parse(&source)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionKind {
    Move,
    Delete,
    Edit,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    Table,
    Grouped,
}

impl Layout {
    pub fn other(self) -> Self {
        match self {
            Layout::Table => Layout::Grouped,
            Layout::Grouped => Layout::Table,
        }
    }

    pub fn named(self) -> &'static str {
        match self {
            Layout::Table => "table",
            Layout::Grouped => "grouped",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    One,
    All,
    Status,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub key: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ActionKind,
    #[serde(default)]
    pub to_state: Option<String>,
    #[serde(default)]
    pub set: BTreeMap<String, String>,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default = "yes")]
    pub confirm: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "default_debounce")]
    pub debounce_ms: u64,
    #[serde(default = "default_backstop")]
    pub backstop_ms: u64,
}

impl Default for Watch {
    fn default() -> Self {
        Self {
            enabled: true,
            debounce_ms: default_debounce(),
            backstop_ms: default_backstop(),
        }
    }
}

fn default_debounce() -> u64 {
    120
}

fn default_backstop() -> u64 {
    4000
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ignore {
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default = "yes")]
    pub hidden: bool,
}

impl Default for Ignore {
    fn default() -> Self {
        Self {
            names: Vec::new(),
            hidden: true,
        }
    }
}

impl Ignore {
    pub fn skips(&self, name: &str) -> bool {
        if self.hidden && name.starts_with('.') {
            return true;
        }
        self.names.iter().any(|ignored| ignored == name)
    }
}

fn yes() -> bool {
    true
}

fn expanded_path<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PathBuf, D::Error> {
    let raw = String::deserialize(deserializer)?;
    Ok(expand_home(&raw))
}

pub fn expand_home(raw: &str) -> PathBuf {
    let Some(rest) = raw.strip_prefix('~') else {
        return PathBuf::from(raw);
    };
    let Some(home) = home() else {
        return PathBuf::from(raw);
    };
    home.join(rest.trim_start_matches(['/', '\\']))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INGEST: &str = r##"
[profile.ingest]
root = "/tmp/ingest"

[[profile.ingest.state]]
name = "queued"
dir  = "{queue}"

[[profile.ingest.state]]
name     = "failed"
dir      = "{queue}-failed"
priority = 10

[profile.ingest.filename]
pattern  = '^(?<claim>[\dxX])_(?<source>\w+)_(?<stamp>.+)-(?<job>[A-Za-z][\w.]*)-(?<index>\d+)\.txt$'
template = "{claim}_{source}_{stamp}-{job}-{index}.txt"
label    = "{job}"
detail   = "#{index}"

[[profile.ingest.status]]
name  = "failed"
state = "failed"

[[profile.ingest.status]]
name  = "running"
state = "queued"
when  = { claim = '^\d+$' }
badge = "worker {claim}"

[[profile.ingest.status]]
name  = "waiting"
state = "queued"

[[profile.ingest.action]]
key      = "r"
name     = "restart"
type     = "move"
to_state = "queued"
set      = { claim = "x" }

[[profile.ingest.action]]
key  = "d"
name = "delete"
type = "delete"

[profile.ingest.ignore]
names = [".DS_Store", ".gitkeep"]
"##;

    fn parse(text: &str) -> Result<Config> {
        let config: Config = toml::from_str(text)?;
        for profile in config.profile.values() {
            profile.validate()?;
        }
        Ok(config)
    }

    fn ingest() -> Profile {
        parse(INGEST).unwrap().profile.remove("ingest").unwrap()
    }

    #[test]
    fn loads_a_realistic_profile() {
        let profile = ingest();
        assert_eq!(profile.state.len(), 2);
        assert_eq!(profile.status.len(), 3);
        assert_eq!(profile.action.len(), 2);
        assert!(profile.ignore.skips(".DS_Store"));
        assert!(profile.ignore.skips(".hidden"));
        assert!(!profile.ignore.skips("x_worker_stamp-Job-0.txt"));
    }

    #[test]
    fn priority_lifts_a_state_above_the_order_it_was_declared_in() {
        let profile = ingest();
        let displayed: Vec<&str> = profile
            .states_in_display_order()
            .iter()
            .map(|state| state.name.as_str())
            .collect();
        assert_eq!(displayed, ["failed", "queued"]);
    }

    #[test]
    fn declaration_order_holds_when_priorities_match() {
        let profile: Profile = toml::from_str(
            r#"
root = "/tmp/jobs"
[[state]]
name = "inbox"
dir = "inbox"
[[state]]
name = "failed"
dir = "failed"
"#,
        )
        .unwrap();
        let displayed: Vec<&str> = profile
            .states_in_display_order()
            .iter()
            .map(|state| state.name.as_str())
            .collect();
        assert_eq!(displayed, ["inbox", "failed"]);
    }

    #[test]
    fn defaults_to_the_only_profile_when_none_is_named() {
        let config = parse(INGEST).unwrap();
        assert_eq!(config.select(None).unwrap().state.len(), 2);
    }

    #[test]
    fn says_which_profiles_exist_when_the_asked_for_one_does_not() {
        let config = parse(INGEST).unwrap();
        let message = config.select(Some("missing")).unwrap_err().to_string();
        assert!(message.contains("missing"), "{message}");
        assert!(message.contains("ingest"), "{message}");
    }

    #[test]
    fn says_which_profiles_exist_when_none_was_asked_for() {
        let two = format!("{}\n[profile.other]\nroot = \"/tmp\"\n", INGEST);
        let message = parse(&two).unwrap().select(None).unwrap_err().to_string();
        assert!(message.contains("ingest"), "{message}");
        assert!(message.contains("other"), "{message}");
    }

    #[test]
    fn rejects_a_template_placeholder_the_pattern_never_captures() {
        let broken = INGEST.replace(r#"label    = "{job}""#, r#"label    = "{nope}""#);
        let message = parse(&broken).unwrap_err().to_string();
        assert!(message.contains("nope"), "{message}");
    }

    #[test]
    fn rejects_an_action_that_moves_nowhere_real() {
        let broken = INGEST.replace(r#"to_state = "queued""#, r#"to_state = "gone""#);
        let message = parse(&broken).unwrap_err().to_string();
        assert!(message.contains("gone"), "{message}");
    }

    #[test]
    fn rejects_a_move_with_no_destination() {
        let broken = INGEST.replace("to_state = \"queued\"\n", "");
        let message = parse(&broken).unwrap_err().to_string();
        assert!(message.contains("to_state"), "{message}");
    }

    #[test]
    fn rejects_an_action_bound_to_a_navigation_key() {
        let broken = INGEST.replace(r#"key      = "r""#, r#"key      = "q""#);
        let message = parse(&broken).unwrap_err().to_string();
        assert!(message.contains("reserved"), "{message}");
    }

    #[test]
    fn rejects_two_actions_on_one_key() {
        let broken = INGEST.replace(r#"key  = "d""#, r#"key  = "r""#);
        let message = parse(&broken).unwrap_err().to_string();
        assert!(message.contains("bound"), "{message}");
    }

    #[test]
    fn rejects_two_states_with_one_name() {
        let broken = INGEST.replace(r#"name     = "failed""#, r#"name     = "queued""#);
        let message = parse(&broken).unwrap_err().to_string();
        assert!(message.contains("queued"), "{message}");
    }

    #[test]
    fn rejects_a_status_that_tests_an_uncaptured_field() {
        let broken = INGEST.replace(
            r#"when  = { claim = '^\d+$' }"#,
            r#"when  = { nope = '^\d+$' }"#,
        );
        let message = parse(&broken).unwrap_err().to_string();
        assert!(message.contains("nope"), "{message}");
    }

    #[test]
    fn rejects_an_unknown_field() {
        let broken = INGEST.replace(
            "[[profile.ingest.state]]",
            "[[profile.ingest.state]]\nsuffix = \"-failed\"",
        );
        assert!(parse(&broken).is_err());
    }

    #[test]
    fn watching_is_on_by_default_and_fully_adjustable() {
        let profile: Profile = toml::from_str("root = \"/tmp\"").unwrap();
        assert!(profile.watch.enabled);
        assert_eq!(profile.watch.debounce_ms, 120);
        assert_eq!(profile.watch.backstop_ms, 4000);
        assert!(profile.mouse);

        let tuned: Profile = toml::from_str(
            r#"
root = "/tmp"
mouse = false
[watch]
enabled = false
backstop_ms = 0
"#,
        )
        .unwrap();
        assert!(!tuned.watch.enabled);
        assert_eq!(tuned.watch.backstop_ms, 0);
        assert_eq!(tuned.watch.debounce_ms, 120);
        assert!(!tuned.mouse);
    }

    #[test]
    fn a_profile_can_rebind_navigation_and_then_free_up_the_old_key() {
        let profile: Profile = toml::from_str(
            r#"
root = "/tmp"
[[state]]
name = "queued"
dir  = "queued"
[keys]
quit = ["ctrl-c"]
[[action]]
key  = "q"
name = "quarantine"
type = "delete"
"#,
        )
        .unwrap();
        profile.validate().unwrap();
        assert_eq!(profile.action[0].key, "q");
    }

    #[test]
    fn expands_a_leading_tilde() {
        unsafe { std::env::set_var("HOME", "/home/tester") };
        assert_eq!(
            expand_home("~/queues"),
            PathBuf::from("/home/tester/queues")
        );
        assert_eq!(expand_home("/absolute"), PathBuf::from("/absolute"));
    }
}
