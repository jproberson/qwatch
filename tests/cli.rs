use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn qwatch(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_qwatch"))
        .args(arguments)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run qwatch")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn queue_tree() -> TempDir {
    let root = TempDir::new().unwrap();
    for directory in ["orders", "orders-failed"] {
        std::fs::create_dir(root.path().join(directory)).unwrap();
    }
    std::fs::write(
        root.path().join("orders/x_PlaceOrder-0.txt"),
        "PlaceOrder,0",
    )
    .unwrap();
    std::fs::write(
        root.path().join("orders-failed/2_PlaceOrder-1.txt"),
        "PlaceOrder,1",
    )
    .unwrap();
    root
}

fn as_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn reports_its_version() {
    let output = qwatch(&["--version"]);
    assert!(output.status.success());
    assert!(text(&output.stdout).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn explains_itself() {
    let output = qwatch(&["--help"]);
    assert!(output.status.success());

    let help = text(&output.stdout);
    for expected in ["--profile", "--config", "--list", "--json", "init"] {
        assert!(help.contains(expected), "--help never mentions {expected}");
    }
}

#[test]
fn lists_a_directory_with_no_configuration_at_all() {
    let root = queue_tree();
    let output = qwatch(&["--list", &as_str(root.path())]);
    assert!(output.status.success(), "{}", text(&output.stderr));

    let listing = text(&output.stdout);
    assert!(listing.contains("x_PlaceOrder-0.txt"));
    assert!(listing.contains("2_PlaceOrder-1.txt"));
    assert_eq!(listing.lines().count(), 2);
}

#[test]
fn emits_json_that_looks_like_json() {
    let root = queue_tree();
    let output = qwatch(&["--json", &as_str(root.path())]);
    assert!(output.status.success());

    let emitted = text(&output.stdout);
    assert!(emitted.trim_start().starts_with('['));
    assert!(emitted.trim_end().ends_with(']'));
    for key in [
        "\"name\"",
        "\"states\"",
        "\"entries\"",
        "\"path\"",
        "\"modified\"",
    ] {
        assert!(emitted.contains(key), "json has no {key}");
    }
}

#[test]
fn writes_a_starter_config_that_it_can_then_read_back() {
    let root = queue_tree();
    let written = root.path().join("profile.toml");

    let created = qwatch(&["init", &as_str(root.path()), "--output", &as_str(&written)]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(written.exists());

    let used = qwatch(&["--config", &as_str(&written), "--list"]);
    assert!(used.status.success(), "{}", text(&used.stderr));
    assert!(text(&used.stdout).contains("PlaceOrder"));
}

#[test]
fn init_prints_without_writing_anything() {
    let root = queue_tree();
    let output = qwatch(&["init", &as_str(root.path()), "--print"]);
    assert!(output.status.success());
    assert!(text(&output.stdout).contains("[[profile."));
    assert!(!root.path().join("config.toml").exists());
}

#[test]
fn a_directory_called_init_is_still_reachable() {
    let root = TempDir::new().unwrap();
    let awkward = root.path().join("init");
    std::fs::create_dir(&awkward).unwrap();
    std::fs::create_dir(awkward.join("pending")).unwrap();
    std::fs::write(awkward.join("pending/job.txt"), "").unwrap();

    let output = qwatch(&["--list", &as_str(&awkward)]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(text(&output.stdout).contains("job.txt"));
}

#[test]
fn fails_loudly_when_the_root_is_not_there() {
    let output = qwatch(&["--list", "/nowhere/at/all"]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("does not exist"));
    assert!(text(&output.stdout).is_empty(), "errors belong on stderr");
}

#[test]
fn fails_loudly_when_no_directory_and_no_config_exist() {
    let root = TempDir::new().unwrap();
    let output = qwatch(&["--config", &as_str(&root.path().join("absent.toml"))]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("qwatch <directory>"));
}

#[test]
fn refuses_a_config_it_cannot_make_sense_of() {
    let root = TempDir::new().unwrap();
    let broken = root.path().join("broken.toml");
    std::fs::write(&broken, "[profile.p]\nroot = \"/tmp\"\nmystery = true\n").unwrap();

    let output = qwatch(&["--config", &as_str(&broken), "--list"]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("mystery"));
}

#[test]
fn a_closed_pipe_is_not_an_error() {
    let root = TempDir::new().unwrap();
    let queue = root.path().join("bulk");
    std::fs::create_dir(&queue).unwrap();
    for at in 0..6000 {
        std::fs::write(
            queue.join(format!("a_long_enough_filename_to_fill_a_pipe_{at}.txt")),
            "",
        )
        .unwrap();
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_qwatch"))
        .args(["--list", &as_str(root.path())])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    drop(child.stdout.take());
    let finished = child.wait_with_output().unwrap();

    assert!(
        finished.status.success(),
        "a closed pipe should not be a failure: {}",
        text(&finished.stderr)
    );
    assert!(!text(&finished.stderr).contains("panicked"));
}

#[test]
fn asking_for_the_browser_without_a_terminal_explains_itself() {
    let root = queue_tree();
    let output = qwatch(&[&as_str(root.path())]);

    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("needs a terminal"));
    assert!(!text(&output.stderr).contains("panicked"));
}

#[test]
fn listing_and_json_cannot_both_be_asked_for() {
    let root = queue_tree();
    let output = qwatch(&["--list", "--json", &as_str(root.path())]);

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("cannot be used with"));
}

#[test]
fn it_can_describe_itself_to_a_shell() {
    for shell in ["bash", "zsh", "fish"] {
        let output = qwatch(&["completions", shell]);
        assert!(output.status.success(), "no completions for {shell}");
        assert!(text(&output.stdout).contains("qwatch"));
    }
}

#[test]
fn the_help_shows_how_to_actually_use_it() {
    let help = text(&qwatch(&["--help"]).stdout);
    assert!(help.contains("Examples:"));
    assert!(help.contains("qwatch --json"));
}
