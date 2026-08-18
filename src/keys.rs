use anyhow::{Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl Binding {
    pub fn parse(source: &str) -> Result<Self> {
        let mut modifiers = KeyModifiers::NONE;
        let mut rest = source.trim();

        loop {
            let lowered = rest.to_lowercase();
            let found = ["ctrl", "control", "alt", "meta", "shift"]
                .into_iter()
                .find(|prefix| {
                    lowered.starts_with(prefix)
                        && matches!(rest.chars().nth(prefix.len()), Some('-') | Some('+'))
                });
            let Some(prefix) = found else {
                break;
            };
            modifiers |= match prefix {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "meta" => KeyModifiers::ALT,
                _ => KeyModifiers::SHIFT,
            };
            rest = &rest[prefix.len() + 1..];
        }

        Ok(Self {
            code: code_of(rest)?,
            modifiers,
        })
    }

    pub fn of(event: KeyEvent) -> Option<Self> {
        let usable = matches!(
            event.code,
            KeyCode::Char(_)
                | KeyCode::F(_)
                | KeyCode::Enter
                | KeyCode::Tab
                | KeyCode::BackTab
                | KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Insert
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
        );
        usable.then_some(Self {
            code: event.code,
            modifiers: event.modifiers - KeyModifiers::SHIFT,
        })
    }

    pub fn matches(self, event: KeyEvent) -> bool {
        let wanted = self.modifiers - KeyModifiers::SHIFT;
        let given = event.modifiers - KeyModifiers::SHIFT;
        self.code == event.code && wanted == given
    }
}

fn code_of(name: &str) -> Result<KeyCode> {
    let mut characters = name.chars();
    if let (Some(only), None) = (characters.next(), characters.next()) {
        return Ok(KeyCode::Char(only));
    }
    if let Some(number) = name.to_lowercase().strip_prefix('f')
        && let Ok(index) = number.parse::<u8>()
        && (1..=12).contains(&index)
    {
        return Ok(KeyCode::F(index));
    }
    Ok(match name.to_lowercase().as_str() {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        other => bail!("unknown key {other:?}"),
    })
}

impl TryFrom<String> for Binding {
    type Error = anyhow::Error;

    fn try_from(source: String) -> Result<Self> {
        Self::parse(&source)
    }
}

impl<'de> Deserialize<'de> for Binding {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let source = String::deserialize(deserializer)?;
        Self::parse(&source).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Down,
    Up,
    PageDown,
    PageUp,
    First,
    Last,
    PreviewDown,
    PreviewUp,
    Edit,
    Rescan,
    Sort,
    Layout,
    Settings,
    Help,
    Quit,
}

macro_rules! keymap {
    ($($field:ident => $motion:ident, $label:literal, [$($default:literal),*];)*) => {
        #[derive(Debug, Clone)]
        pub struct Keys {
            $(pub $field: Vec<Binding>,)*
        }

        #[derive(Debug, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Rebindings {
            $(#[serde(default)] $field: Option<Vec<Binding>>,)*
        }

        impl<'de> Deserialize<'de> for Keys {
            fn deserialize<D: serde::Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Self, D::Error> {
                let given = Rebindings::deserialize(deserializer)?;
                let mut keys = Self::default();
                $(if let Some(bindings) = given.$field { keys.$field = bindings; })*
                Ok(keys)
            }
        }

        impl Default for Keys {
            fn default() -> Self {
                Self {
                    $($field: [$($default),*].iter().map(|key| Binding::parse(key).unwrap()).collect(),)*
                }
            }
        }

        impl Keys {
            pub fn motion_for(&self, event: KeyEvent) -> Option<Motion> {
                $(
                    if self.$field.iter().any(|binding| binding.matches(event)) {
                        return Some(Motion::$motion);
                    }
                )*
                None
            }

            pub fn described(&self) -> Vec<(String, &'static str)> {
                let mut out = Vec::new();
                $(
                    if !self.$field.is_empty() {
                        out.push((shown(&self.$field), $label));
                    }
                )*
                out
            }

            pub fn claims(&self, binding: Binding) -> bool {
                $(if self.$field.contains(&binding) { return true; })*
                false
            }

            pub fn motions(&self) -> Vec<(&'static str, String, &'static str)> {
                vec![$((stringify!($field), shown(&self.$field), $label),)*]
            }

            pub fn rebind(&mut self, motion: &str, binding: Binding) -> bool {
                $(if motion == stringify!($field) { self.$field = vec![binding]; return true; })*
                false
            }

            pub fn written(&self, motion: &str) -> Option<Vec<String>> {
                $(if motion == stringify!($field) {
                    return Some(self.$field.iter().map(|b| b.written()).collect());
                })*
                None
            }
        }
    };
}

keymap! {
    down         => Down,        "move down",           ["j", "down"];
    up           => Up,          "move up",             ["k", "up"];
    page_down    => PageDown,    "page down",           ["pagedown"];
    page_up      => PageUp,      "page up",             ["pageup"];
    first        => First,       "first file",          ["g", "home"];
    last         => Last,        "last file",           ["G", "end"];
    preview_down => PreviewDown, "scroll the preview",  ["J"];
    preview_up   => PreviewUp,   "scroll the preview back", ["K"];
    edit         => Edit,        "open in $EDITOR",     ["enter"];
    rescan       => Rescan,      "rescan now",          ["R"];
    sort         => Sort,        "change sort order",   ["s"];
    layout       => Layout,      "switch layout",       ["t"];
    settings     => Settings,    "open settings",       ["ctrl-s"];
    help         => Help,        "show these keys",     ["?"];
    quit         => Quit,        "quit",                ["q", "esc"];
}

fn shown(bindings: &[Binding]) -> String {
    bindings
        .iter()
        .map(|binding| binding.written())
        .collect::<Vec<_>>()
        .join(" / ")
}

impl Binding {
    pub fn written(self) -> String {
        let mut out = String::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            out.push_str("ctrl-");
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            out.push_str("alt-");
        }
        out.push_str(&match self.code {
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Char(only) => only.to_string(),
            KeyCode::F(index) => format!("f{index}"),
            other => format!("{other:?}").to_lowercase(),
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn reads_a_plain_character() {
        assert!(
            Binding::parse("j")
                .unwrap()
                .matches(press(KeyCode::Char('j')))
        );
        assert!(
            !Binding::parse("j")
                .unwrap()
                .matches(press(KeyCode::Char('k')))
        );
    }

    #[test]
    fn tells_upper_and_lower_case_apart() {
        let upper = Binding::parse("J").unwrap();
        assert!(upper.matches(press(KeyCode::Char('J'))));
        assert!(!upper.matches(press(KeyCode::Char('j'))));
    }

    #[test]
    fn reads_the_named_keys() {
        assert!(
            Binding::parse("enter")
                .unwrap()
                .matches(press(KeyCode::Enter))
        );
        assert!(Binding::parse("esc").unwrap().matches(press(KeyCode::Esc)));
        assert!(
            Binding::parse("pagedown")
                .unwrap()
                .matches(press(KeyCode::PageDown))
        );
        assert!(Binding::parse("f5").unwrap().matches(press(KeyCode::F(5))));
    }

    #[test]
    fn reads_a_modifier_prefix() {
        let binding = Binding::parse("ctrl-d").unwrap();
        assert!(binding.matches(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)));
        assert!(!binding.matches(press(KeyCode::Char('d'))));
    }

    #[test]
    fn ignores_a_shift_the_character_already_carries() {
        let binding = Binding::parse("G").unwrap();
        assert!(binding.matches(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT)));
    }

    #[test]
    fn refuses_a_key_nobody_could_mean() {
        assert!(Binding::parse("wibble").is_err());
    }

    #[test]
    fn the_defaults_are_the_vim_ones() {
        let keys = Keys::default();
        assert_eq!(
            keys.motion_for(press(KeyCode::Char('j'))),
            Some(Motion::Down)
        );
        assert_eq!(keys.motion_for(press(KeyCode::Down)), Some(Motion::Down));
        assert_eq!(
            keys.motion_for(press(KeyCode::Char('G'))),
            Some(Motion::Last)
        );
        assert_eq!(keys.motion_for(press(KeyCode::Esc)), Some(Motion::Quit));
        assert_eq!(keys.motion_for(press(KeyCode::Char('z'))), None);
    }

    #[test]
    fn a_profile_can_rebind_a_motion() {
        let keys: Keys = toml::from_str(r#"down = ["n", "ctrl-j"]"#).unwrap();
        assert_eq!(
            keys.motion_for(press(KeyCode::Char('n'))),
            Some(Motion::Down)
        );
        assert_eq!(
            keys.motion_for(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)),
            Some(Motion::Down)
        );
        assert_eq!(keys.motion_for(press(KeyCode::Char('j'))), None);
    }

    #[test]
    fn rebinding_one_motion_leaves_the_others_alone() {
        let keys: Keys = toml::from_str(r#"down = ["n"]"#).unwrap();
        assert_eq!(keys.motion_for(press(KeyCode::Char('k'))), Some(Motion::Up));
    }

    #[test]
    fn knows_which_keys_it_has_claimed() {
        let keys = Keys::default();
        assert!(keys.claims(Binding::parse("q").unwrap()));
        assert!(!keys.claims(Binding::parse("r").unwrap()));
    }

    #[test]
    fn writes_a_binding_back_out_for_the_reader() {
        assert_eq!(Binding::parse("ctrl-d").unwrap().written(), "ctrl-d");
        assert_eq!(Binding::parse("enter").unwrap().written(), "enter");
        assert_eq!(Binding::parse("G").unwrap().written(), "G");
    }
}
