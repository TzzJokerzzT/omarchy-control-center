//! Regression tests for the installer script.
//!
//! Nothing here builds the real project. A stub `cargo` is put first on PATH, so the
//! script's own logic — paths, the short-name link, the marked PATH block, the uninstall —
//! is exercised in milliseconds against a scratch HOME. That keeps the suite fast and
//! still proves the behaviour a user depends on.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MARK_BEGIN: &str = "# >>> omarchy-control-center >>>";
const MARK_END: &str = "# <<< omarchy-control-center <<<";

struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("occ-installer-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home")).expect("home");
        std::fs::create_dir_all(dir.join("stub")).expect("stub");
        std::fs::write(
            dir.join("home/.zshrc"),
            "export ZSH=\"$HOME/.oh-my-zsh\"\nplugins=(git)\n",
        )
        .expect("zshrc");

        // A `cargo` that pretends to build: it just leaves a runnable binary behind.
        sandbox_tool(
            &dir,
            "cargo",
            "mkdir -p target/release\nprintf '#!/bin/sh\\necho omarchy-control-center 0.1.0\\n' > target/release/omarchy-control-center\nchmod +x target/release/omarchy-control-center\n",
        );
        sandbox_tool(&dir, "rustc", "printf 'rustc 1.99.1\\n'\n");
        Self { dir }
    }

    fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    fn prefix(&self) -> PathBuf {
        self.dir.join("bin")
    }

    fn rc(&self) -> PathBuf {
        self.home().join(".zshrc")
    }

    fn rc_text(&self) -> String {
        std::fs::read_to_string(self.rc()).unwrap_or_default()
    }

    fn run(&self, args: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.dir.join("stub").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new("bash");
        command
            .arg("install.sh")
            .args(args)
            .args(["--prefix", &self.prefix().display().to_string()])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("PATH", path)
            .env("HOME", self.home())
            .env("SHELL", "/bin/zsh")
            .env("XDG_CONFIG_HOME", self.dir.join("config"))
            .env("XDG_STATE_HOME", self.dir.join("state"))
            .env_remove("XDG_BIN_HOME");
        command.output().expect("run install.sh")
    }
}

fn sandbox_tool(dir: &Path, name: &str, body: &str) {
    let file = dir.join("stub").join(name);
    std::fs::write(&file, format!("#!/bin/sh\n{body}")).expect("write stub");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn count_block(text: &str) -> usize {
    text.lines().filter(|line| *line == MARK_BEGIN).count()
}

#[test]
fn help_lists_the_options_and_changes_nothing() {
    let sandbox = Sandbox::new("help");
    let output = sandbox.run(&["--help"]);
    assert!(output.status.success(), "{}", text(&output));
    let text = text(&output);
    for expected in [
        "--prefix",
        "--path",
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
        text.contains("control-center"),
        "the short name is not documented:\n{text}"
    );
    assert!(
        text.contains("What it leaves alone unless you ask"),
        "{text}"
    );
}

#[test]
fn dry_run_prints_every_step_without_touching_the_disk() {
    let sandbox = Sandbox::new("dry");
    let output = sandbox.run(&["--dry-run", "--no-plugin", "--path"]);
    assert!(output.status.success(), "{}", text(&output));

    let text = text(&output);
    assert!(
        text.contains("[dry-run] cargo build --release --locked"),
        "{text}"
    );
    assert!(text.contains("[dry-run] install -m 755"), "{text}");
    assert!(text.contains("control-center"), "{text}");
    assert!(text.contains("append the PATH block"), "{text}");

    assert!(
        !sandbox.prefix().exists(),
        "a dry run must not create the prefix"
    );
    assert_eq!(
        count_block(&sandbox.rc_text()),
        0,
        "a dry run must not edit the shell config"
    );
}

#[test]
fn an_unknown_option_fails_and_says_so() {
    let sandbox = Sandbox::new("unknown");
    let output = sandbox.run(&["--nope"]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("unknown option"),
        "{}",
        text(&output)
    );
}

#[test]
fn installing_creates_the_program_and_the_short_name_that_runs_from_anywhere() {
    let sandbox = Sandbox::new("install");
    let output = sandbox.run(&["--no-plugin", "--path"]);
    assert!(output.status.success(), "{}", text(&output));

    let binary = sandbox.prefix().join("omarchy-control-center");
    let link = sandbox.prefix().join("control-center");
    assert!(binary.is_file(), "the program was not installed");
    assert!(
        std::fs::symlink_metadata(&link)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false),
        "control-center must be a link to the program"
    );

    // The point of the short name: it runs from any directory, with no alias loaded.
    let run = Command::new(&link)
        .arg("--version")
        .current_dir(std::env::temp_dir())
        .env("HOME", sandbox.home())
        .output()
        .expect("run the link");
    assert!(
        run.status.success(),
        "control-center --version failed: {}",
        text(&run)
    );
    assert!(text(&run).contains("omarchy-control-center"));
}

#[test]
fn the_path_block_is_added_once_no_matter_how_often_you_install() {
    let sandbox = Sandbox::new("idempotent");
    for _ in 0..3 {
        let output = sandbox.run(&["--no-plugin", "--path"]);
        assert!(output.status.success(), "{}", text(&output));
    }

    let rc = sandbox.rc_text();
    assert_eq!(count_block(&rc), 1, "the block was duplicated:\n{rc}");
    assert_eq!(
        rc.lines().filter(|line| *line == MARK_END).count(),
        1,
        "{rc}"
    );
    assert!(
        rc.contains("plugins=(git)"),
        "the existing configuration was lost:\n{rc}"
    );
    assert!(rc.contains(sandbox.prefix().to_str().unwrap()), "{rc}");
}

#[test]
fn no_path_leaves_the_shell_configuration_alone() {
    let sandbox = Sandbox::new("no-path");
    let before = sandbox.rc_text();
    let output = sandbox.run(&["--no-plugin", "--no-path"]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        sandbox.rc_text(),
        before,
        "--no-path edited the shell configuration"
    );
    assert!(
        text(&output).contains("leaving your shell configuration alone"),
        "{}",
        text(&output)
    );
}

#[test]
fn uninstalling_removes_the_link_and_restores_the_shell_configuration_exactly() {
    let sandbox = Sandbox::new("uninstall");
    let before = sandbox.rc_text();

    assert!(sandbox.run(&["--no-plugin", "--path"]).status.success());
    assert_eq!(count_block(&sandbox.rc_text()), 1);

    let output = sandbox.run(&["--uninstall", "--yes"]);
    assert!(output.status.success(), "{}", text(&output));

    assert!(
        !sandbox.prefix().join("control-center").exists(),
        "the link is still there"
    );
    assert!(
        !sandbox.prefix().join("omarchy-control-center").exists(),
        "the program is still there"
    );
    assert_eq!(
        sandbox.rc_text(),
        before,
        "the shell configuration was not restored to its original content"
    );
    assert!(
        text(&output).contains("Left in place"),
        "the data policy is not stated"
    );
}

#[test]
fn uninstalling_something_that_is_not_installed_is_harmless() {
    let sandbox = Sandbox::new("absent");
    let output = sandbox.run(&["--uninstall", "--yes"]);
    assert!(output.status.success(), "{}", text(&output));

    let text = text(&output);
    assert!(text.contains("nothing to remove"), "{text}");
    assert!(text.contains("has no block from this installer"), "{text}");
    assert!(text.contains("the widget is not installed"), "{text}");
    assert!(!sandbox.dir.join("state/omarchy-control-center").exists());
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
