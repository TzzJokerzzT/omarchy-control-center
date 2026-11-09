//! Regression tests for the installer script.
//!
//! They only run read-only modes (`--help`, `--dry-run`, a rejected option) and a
//! removal against a directory that does not exist. Every test points the XDG
//! directories at a scratch path, so a bug in the script cannot touch the real
//! configuration, history or quarantine area of the machine running the suite.

use std::path::PathBuf;
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("occ-installer-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn installer(name: &str, args: &[&str]) -> Output {
    let dir = scratch(name);
    let mut command = Command::new("bash");
    command
        .arg("install.sh")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("XDG_BIN_HOME", dir.join("bin"));
    command.output().expect("run install.sh")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn help_lists_the_options_and_changes_nothing() {
    let output = installer("help", &["--help"]);
    assert!(output.status.success(), "{}", text(&output));
    let text = text(&output);
    for expected in [
        "--prefix",
        "--plugin",
        "--dry-run",
        "--uninstall",
        "--purge-state",
    ] {
        assert!(
            text.contains(expected),
            "{expected} is missing from --help:\n{text}"
        );
    }
    assert!(
        text.contains("What it leaves alone unless you ask"),
        "{text}"
    );
}

#[test]
fn dry_run_prints_every_step_without_touching_the_disk() {
    let dir = scratch("dry");
    let output = installer("dry", &["--dry-run", "--no-plugin"]);
    assert!(output.status.success(), "{}", text(&output));

    let text = text(&output);
    assert!(
        text.contains("[dry-run] cargo build --release --locked"),
        "{text}"
    );
    assert!(text.contains("[dry-run] install -m 755"), "{text}");
    assert!(text.contains("Building the release binary"), "{text}");
    assert!(text.contains("Verifying the installed binary"), "{text}");

    assert!(
        !dir.join("bin").exists(),
        "a dry run must not create the prefix"
    );
}

#[test]
fn an_unknown_option_fails_and_says_so() {
    let output = installer("unknown", &["--nope"]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("unknown option"),
        "{}",
        text(&output)
    );
}

#[test]
fn uninstalling_something_that_is_not_installed_is_harmless() {
    let dir = scratch("uninstall");
    let output = installer("uninstall", &["--uninstall", "--yes"]);
    assert!(output.status.success(), "{}", text(&output));

    let text = text(&output);
    assert!(text.contains("nothing to remove"), "{text}");
    assert!(text.contains("the widget is not installed"), "{text}");
    assert!(
        text.contains("Left in place"),
        "the data policy is not stated:\n{text}"
    );
    assert!(!dir.join("state/omarchy-control-center").exists());
}

#[test]
fn the_script_is_syntactically_valid() {
    let output = Command::new("bash")
        .args(["-n", "install.sh"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("bash -n");
    assert!(output.status.success(), "{}", text(&output));
}
