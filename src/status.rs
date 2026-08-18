use crate::config::{Profile, Status};
use crate::name::Captures;

pub const UNKNOWN: &str = "unknown";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub name: String,
    pub badge: Option<String>,
}

pub fn resolve(profile: &Profile, state: &str, captures: Option<&Captures>) -> Resolved {
    let Some(captures) = usable_captures(profile, captures) else {
        return Resolved {
            name: UNKNOWN.to_string(),
            badge: None,
        };
    };

    profile
        .status
        .iter()
        .find(|status| applies(status, state, captures))
        .map(|status| Resolved {
            name: status.name.clone(),
            badge: status.badge.as_ref().map(|badge| badge.render(captures)),
        })
        .unwrap_or_else(|| Resolved {
            name: state.to_string(),
            badge: None,
        })
}

fn usable_captures<'a>(
    profile: &Profile,
    captures: Option<&'a Captures>,
) -> Option<&'a Captures> {
    const NONE: &Captures = &Captures::new();
    match captures {
        Some(captures) => Some(captures),
        None if profile.filename.is_some() => None,
        None => Some(NONE),
    }
}

fn applies(status: &Status, state: &str, captures: &Captures) -> bool {
    if status.state.as_deref().is_some_and(|wanted| wanted != state) {
        return false;
    }
    status.when.iter().all(|(capture, pattern)| {
        captures
            .get(capture)
            .is_some_and(|value| pattern.is_match(value))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const PROFILE: &str = r##"
[profile.invoices]
root = "/tmp/invoices"

[[profile.invoices.state]]
name = "queued"
dir  = "{queue}"

[[profile.invoices.state]]
name     = "failed"
dir      = "{queue}-failed"
priority = 10

[profile.invoices.filename]
pattern  = '^(?<claim>[\dxX])_(?<job>\w+)$'
template = "{claim}_{job}"
label    = "{job}"

[[profile.invoices.status]]
name  = "failed"
state = "failed"

[[profile.invoices.status]]
name  = "running"
state = "queued"
when  = { claim = '^\d+$' }
badge = "worker {claim}"

[[profile.invoices.status]]
name  = "waiting"
state = "queued"
"##;

    fn profile() -> Profile {
        let config: Config = toml::from_str(PROFILE).unwrap();
        config.profile.get("invoices").unwrap().clone()
    }

    fn resolve_name(state: &str, file_name: &str) -> Resolved {
        let profile = profile();
        let captures = profile
            .filename
            .as_ref()
            .and_then(|filename| filename.pattern.captures(file_name));
        resolve(&profile, state, captures.as_ref())
    }

    #[test]
    fn refines_a_state_with_the_filename() {
        assert_eq!(
            resolve_name("queued", "3_ParseInvoice"),
            Resolved {
                name: "running".to_string(),
                badge: Some("worker 3".to_string()),
            }
        );
    }

    #[test]
    fn falls_through_to_the_status_with_no_condition() {
        assert_eq!(
            resolve_name("queued", "x_ParseInvoice"),
            Resolved {
                name: "waiting".to_string(),
                badge: None,
            }
        );
    }

    #[test]
    fn takes_the_state_alone_when_that_is_all_a_status_asks_for() {
        assert_eq!(resolve_name("failed", "3_ParseInvoice").name, "failed");
        assert_eq!(resolve_name("failed", "x_ParseInvoice").name, "failed");
    }

    #[test]
    fn calls_a_file_the_pattern_rejects_unknown() {
        assert_eq!(resolve_name("queued", "not-a-job-file").name, UNKNOWN);
    }

    #[test]
    fn names_a_status_after_its_state_when_none_is_declared() {
        let profile: Profile = toml::from_str(
            r#"
root = "/tmp/jobs"
[[state]]
name = "failed"
dir = "failed"
"#,
        )
        .unwrap();
        assert_eq!(
            resolve(&profile, "failed", None),
            Resolved {
                name: "failed".to_string(),
                badge: None,
            }
        );
    }
}
