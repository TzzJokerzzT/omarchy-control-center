//! Process execution. The only place in the program that starts a child process.
//!
//! Rules enforced here, not by convention:
//! * the executable is a bare name resolved against a validated `PATH`;
//! * arguments are passed as separate `execve` entries, never joined into a shell
//!   string, so `;`, `$(...)`, quotes or spaces inside an argument stay data;
//! * every captured command has a deadline and is killed when it expires;
//! * `stderr` and the exit code are always preserved, so a non-zero exit can
//!   never be reported as success.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::domain::{PlannedCommand, SUDO};
use crate::infrastructure::system;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Directories a privileged command may be taken from.
///
/// Both `sudo` and the program it elevates resolve only here, in this order, and never
/// against the session search path: a directory added with `--extra-search-path` or
/// `extra_search_paths` is writable by the user running the interface, so resolving a
/// privileged program there would let that user choose what root executes.
pub const TRUSTED_PRIVILEGED_DIRS: &[&str] = &["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

/// Serializes fixture writes against child spawns in tests only.
///
/// A thread that has a fixture file open for writing, while another thread forks,
/// lets the forked child inherit that write descriptor; the kernel then answers
/// `ETXTBSY` for an `execve` of the same file. That is a property of the testing
/// harness, not of the product, so the guard exists only under `cfg(test)`.
#[cfg(test)]
pub static FIXTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProcessError {
    #[error("{program} was not found in the executable search path")]
    NotFound { program: String },

    #[error(
        "{program} is not in a directory a privileged command may run from \
         (/usr/bin, /bin, /usr/sbin, /sbin)"
    )]
    Untrusted { program: String },

    #[error("invalid program name {program:?}: {reason}")]
    InvalidProgram { program: String, reason: String },

    #[error("could not start {program}: {message}")]
    Spawn { program: String, message: String },

    #[error("{program} exceeded its {seconds}s deadline and was killed")]
    Timeout { program: String, seconds: u64 },
}

/// How the child's stdio is wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Pipes are drained by this module and returned to the caller.
    Capture,
    /// The child owns the terminal (privilege prompt, interactive progress).
    Inherit,
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    /// `None` means "no deadline", allowed only for interactive commands.
    pub timeout: Option<Duration>,
    pub output: OutputMode,
    /// Whether the child must reach root. The interface itself never runs as
    /// root, so a privileged child is invoked through `sudo`.
    pub privilege: Privilege,
}

/// The identity a child runs with. The interface refuses to start as root, so a
/// privileged operation is the only way to reach it, one confirmed command at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege {
    /// Runs with the interface's own identity.
    AsUser,
    /// Runs as `sudo -- <resolved program> <args>`.
    AsRoot,
}

/// Elevation is needed only when the plan asks for root and we are not root already.
/// An unknown uid counts as "not root": skipping the handoff would silently fail.
pub fn elevation_needed(requires_root: bool, euid: Option<u32>) -> bool {
    requires_root && euid != Some(0)
}

impl CommandSpec {
    pub fn capture(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            timeout: Some(DEFAULT_TIMEOUT),
            output: OutputMode::Capture,
            privilege: Privilege::AsUser,
        }
    }

    /// The single mapping from a confirmed plan command to a process spec.
    ///
    /// A command that needs root also owns the terminal: the password prompt must
    /// reach the tty the interface handed back, and a human typing it must never be
    /// killed at the capture deadline.
    pub fn planned(command: &PlannedCommand) -> Self {
        Self::planned_with(command, system::current_euid())
    }

    /// `planned` with the identity supplied, so the decision is testable without
    /// depending on who runs the test suite.
    pub fn planned_with(command: &PlannedCommand, euid: Option<u32>) -> Self {
        let privileged = elevation_needed(command.needs_interface_privileges(), euid);
        let owns_terminal = command.requires_tty || privileged;
        Self {
            program: command.program.clone(),
            args: command.args.clone(),
            timeout: if owns_terminal {
                None
            } else {
                Some(DEFAULT_TIMEOUT)
            },
            output: if owns_terminal {
                OutputMode::Inherit
            } else {
                OutputMode::Capture
            },
            privilege: if privileged {
                Privilege::AsRoot
            } else {
                Privilege::AsUser
            },
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Applies `timeout` only to a captured command: a child that owns the terminal
    /// is never killed, because a human may be typing a password into it.
    pub fn with_captured_timeout(mut self, timeout: Duration) -> Self {
        if self.output == OutputMode::Capture {
            self.timeout = Some(timeout);
        }
        self
    }

    /// Display-only rendering, for logs and plans. Never executed.
    pub fn display(&self) -> String {
        let mut out = self.program.clone();
        for arg in &self.args {
            out.push(' ');
            out.push_str(arg);
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    /// The only definition of success used by the program.
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

pub trait CommandRunner: Send + Sync {
    fn resolve(&self, program: &str) -> Option<PathBuf>;

    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ProcessError>;
}

/// Real runner. `path` is injectable so tests never depend on the host tools.
#[derive(Debug, Clone)]
pub struct SystemRunner {
    path: Option<OsString>,
    /// Where a privileged command — `sudo` and the program it elevates — may come from.
    /// Production always trusts `TRUSTED_PRIVILEGED_DIRS` and nothing else; a test runner
    /// that did not ask for trusted directories trusts none, so no test can reach the
    /// host's real `/usr/bin/sudo`.
    trusted: Vec<PathBuf>,
}

impl Default for SystemRunner {
    fn default() -> Self {
        Self::new()
    }
}

fn trusted_privileged_dirs() -> Vec<PathBuf> {
    TRUSTED_PRIVILEGED_DIRS.iter().map(PathBuf::from).collect()
}

impl SystemRunner {
    pub fn new() -> Self {
        Self {
            path: None,
            trusted: trusted_privileged_dirs(),
        }
    }

    /// Prepends `dirs` to the inherited search path, keeping their order: the first
    /// directory that provides a program wins. This affects unprivileged commands only:
    /// a privileged one still resolves from `TRUSTED_PRIVILEGED_DIRS`.
    pub fn with_extra_paths<I>(dirs: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<Path>,
    {
        let mut search: Vec<PathBuf> = dirs
            .into_iter()
            .map(|dir| dir.as_ref().to_path_buf())
            .collect();
        search.extend(std::env::split_paths(&Self::inherited_path()));
        Self {
            path: Some(std::env::join_paths(search).unwrap_or_default()),
            trusted: trusted_privileged_dirs(),
        }
    }

    /// Uses exactly `path`, ignoring the environment. Trusts no directory for privileged
    /// resolution: a test must opt in with `with_trusted_paths`, so it can never spawn the
    /// host's own `sudo` by accident.
    #[cfg(test)]
    pub fn with_path(path: impl Into<OsString>) -> Self {
        Self {
            path: Some(path.into()),
            trusted: Vec::new(),
        }
    }

    /// Replaces the directories a privileged command may resolve from. Test-only on
    /// purpose: the trusted list is a security boundary, not a user preference.
    #[cfg(test)]
    pub fn with_trusted_paths<I>(mut self, dirs: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<Path>,
    {
        self.trusted = dirs
            .into_iter()
            .map(|dir| dir.as_ref().to_path_buf())
            .collect();
        self
    }

    fn inherited_path() -> OsString {
        std::env::var_os("PATH").unwrap_or_default()
    }

    fn effective_path(&self) -> OsString {
        self.path.clone().unwrap_or_else(Self::inherited_path)
    }

    /// Resolution for a privileged command: the trusted directories only, never the
    /// session search path.
    fn resolve_trusted(&self, program: &str) -> Option<PathBuf> {
        resolve_in_dirs(program, &self.trusted)
    }

    fn build(&self, spec: &CommandSpec) -> Result<Command, ProcessError> {
        let (program, args) = self.command_line(spec)?;

        let mut command = Command::new(program);
        command.args(&args);
        // Deterministic labels and dates from the tools we parse; locale never
        // changes the meaning of a field name we read.
        command.env("LC_ALL", "C");
        match spec.output {
            OutputMode::Capture => {
                command
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
            }
            OutputMode::Inherit => {
                command
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit());
            }
        }
        Ok(command)
    }

    /// The argv that actually runs. A privileged command becomes
    /// `sudo -- <resolved program> <args>`: `--` ends sudo's own options, and the
    /// confirmed arguments are never re-read by sudo or by a shell.
    ///
    /// An unprivileged command resolves against the session search path, exactly as
    /// before. A privileged one resolves `sudo` *and* the program against the trusted
    /// system directories only: the search path may hold a user-writable directory, and
    /// root must never run what that directory chooses. A program the trusted directories
    /// do not provide is refused (`Untrusted`), never elevated from somewhere else.
    fn command_line(&self, spec: &CommandSpec) -> Result<(PathBuf, Vec<OsString>), ProcessError> {
        let args: Vec<OsString> = spec.args.iter().map(OsString::from).collect();
        match spec.privilege {
            Privilege::AsUser => {
                let resolved =
                    self.resolve(&spec.program)
                        .ok_or_else(|| ProcessError::NotFound {
                            program: spec.program.clone(),
                        })?;
                Ok((resolved, args))
            }
            Privilege::AsRoot => {
                let sudo = self
                    .resolve_trusted(SUDO)
                    .ok_or_else(|| ProcessError::NotFound {
                        program: SUDO.to_string(),
                    })?;
                let resolved =
                    self.resolve_trusted(&spec.program)
                        .ok_or_else(|| ProcessError::Untrusted {
                            program: spec.program.clone(),
                        })?;
                let mut wrapped: Vec<OsString> = Vec::with_capacity(args.len() + 2);
                wrapped.push(OsString::from("--"));
                wrapped.push(resolved.into_os_string());
                wrapped.extend(args);
                Ok((sudo, wrapped))
            }
        }
    }

    fn run_inner(&self, spec: &CommandSpec) -> Result<CommandOutput, ProcessError> {
        #[cfg(test)]
        let _fixture_guard = FIXTURE_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        validate_program_name(&spec.program)?;
        let mut command = self.build(spec)?;
        let mut child = command.spawn().map_err(|error| ProcessError::Spawn {
            program: spec.program.clone(),
            message: error.to_string(),
        })?;

        if spec.output == OutputMode::Inherit {
            // Interactive: the child owns the terminal and the user decides how
            // long it takes. Cancellation is Ctrl-C, not our deadline.
            let status = child.wait().map_err(|error| ProcessError::Spawn {
                program: spec.program.clone(),
                message: error.to_string(),
            })?;
            return Ok(CommandOutput {
                exit_code: status.code(),
                stdout: String::new(),
                stderr: String::new(),
            });
        }

        let stdout_reader = child.stdout.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let mut buffer = Vec::new();
                let _ = pipe.read_to_end(&mut buffer);
                buffer
            })
        });
        let stderr_reader = child.stderr.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let mut buffer = Vec::new();
                let _ = pipe.read_to_end(&mut buffer);
                buffer
            })
        });

        let deadline = spec.timeout.map(|timeout| Instant::now() + timeout);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ProcessError::Spawn {
                        program: spec.program.clone(),
                        message: error.to_string(),
                    });
                }
            }

            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                kill_and_reap(&mut child, stdout_reader, stderr_reader);
                return Err(ProcessError::Timeout {
                    program: spec.program.clone(),
                    seconds: spec.timeout.unwrap_or_default().as_secs(),
                });
            }

            std::thread::sleep(Duration::from_millis(5));
        };

        let stdout = join_reader(stdout_reader);
        let stderr = join_reader(stderr_reader);
        Ok(CommandOutput {
            exit_code: status.code(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    }
}

/// Kills the direct child and stops waiting. Reader threads are detached rather
/// than joined: a tool that forked a grandchild keeps its pipe open, and the
/// deadline must never turn into an unbounded wait on the caller.
fn kill_and_reap(
    child: &mut std::process::Child,
    stdout_reader: Option<std::thread::JoinHandle<Vec<u8>>>,
    stderr_reader: Option<std::thread::JoinHandle<Vec<u8>>>,
) {
    let _ = child.kill();
    let _ = child.wait();
    drop(stdout_reader);
    drop(stderr_reader);
}

fn join_reader(handle: Option<std::thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
}

impl CommandRunner for SystemRunner {
    fn resolve(&self, program: &str) -> Option<PathBuf> {
        resolve_in_path(program, &self.effective_path())
    }

    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ProcessError> {
        self.run_inner(spec)
    }
}

/// A program name must be a bare executable name: no path separator, no NUL.
/// Anything else would let a name choose its own executable.
fn validate_program_name(program: &str) -> Result<(), ProcessError> {
    if program.is_empty() {
        return Err(ProcessError::InvalidProgram {
            program: program.to_string(),
            reason: "empty program name".to_string(),
        });
    }
    if program.contains('\0') {
        return Err(ProcessError::InvalidProgram {
            program: program.to_string(),
            reason: "contains a NUL byte".to_string(),
        });
    }
    if program.contains('/') {
        return Err(ProcessError::InvalidProgram {
            program: program.to_string(),
            reason: "program names must be resolved through PATH, not given as a path".to_string(),
        });
    }
    Ok(())
}

/// Resolves `program` against exactly `dirs`, in order, skipping non-absolute entries.
/// Privileged resolution uses this and nothing else: the session `PATH` never takes part.
fn resolve_in_dirs(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    if validate_program_name(program).is_err() {
        return None;
    }
    dirs.iter()
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

/// Resolves `program` against `path`, ignoring empty and relative entries. A
/// relative entry would let the current directory choose the executable.
pub fn resolve_in_path(program: &str, path: &OsString) -> Option<PathBuf> {
    if validate_program_name(program).is_err() {
        return None;
    }
    for dir in std::env::split_paths(path) {
        if dir.as_os_str().is_empty() || !dir.is_absolute() {
            continue;
        }
        let candidate = dir.join(program);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::PlannedCommand;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestBin {
        dir: PathBuf,
    }

    /// Guards against two tests picking the same fixture name: the directory gets a unique
    /// suffix per instance, so one test's cleanup can never delete another test's fixture
    /// mid-write. That is exactly what CI caught: two tests both asked for "present", and
    /// whichever finished first removed the directory the other was writing into.
    static NEXT_SANDBOX: AtomicUsize = AtomicUsize::new(0);

    impl TestBin {
        fn new(name: &str, script: &str) -> Self {
            // The write must not overlap another test's fork: see FIXTURE_LOCK.
            let _guard = FIXTURE_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let unique = NEXT_SANDBOX.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("occ-proc-{}-{name}-{unique}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("create fixture dir");
            let file = dir.join(name);
            fs::write(&file, script).expect("write fixture");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod fixture");
            Self { dir }
        }

        fn path(&self) -> OsString {
            self.dir.clone().into_os_string()
        }

        fn runner(&self) -> SystemRunner {
            SystemRunner::with_path(self.path())
        }

        /// A runner that trusts only this sandbox for privileged resolution, so a fixture
        /// `sudo` is reachable and the host's real `/usr/bin/sudo` never is.
        fn privileged_runner(&self) -> SystemRunner {
            self.runner().with_trusted_paths([self.dir.clone()])
        }

        /// Adds another executable to the same sandbox, so one fixture can provide
        /// both the program under test and the `sudo` that wraps it.
        fn write(&self, name: &str, script: &str) -> PathBuf {
            let _guard = FIXTURE_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let file = self.dir.join(name);
            fs::write(&file, script).expect("write fixture");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod fixture");
            file
        }
    }

    impl Drop for TestBin {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn captures_stdout_and_a_zero_exit_code() {
        let bin = TestBin::new("hello", "#!/bin/sh\necho hello\nexit 0\n");
        let output = bin
            .runner()
            .run(&CommandSpec::capture("hello", vec![]))
            .expect("run");
        assert_eq!(output.stdout.trim(), "hello");
        assert_eq!(output.exit_code, Some(0));
        assert!(output.success());
    }

    #[test]
    fn a_non_zero_exit_code_is_never_success() {
        let bin = TestBin::new("failing", "#!/bin/sh\necho oops >&2\nexit 7\n");
        let output = bin
            .runner()
            .run(&CommandSpec::capture("failing", vec![]))
            .expect("run");
        assert_eq!(output.exit_code, Some(7));
        assert!(!output.success());
        assert_eq!(output.stderr.trim(), "oops");
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn hostile_arguments_arrive_verbatim_and_are_never_interpreted_by_a_shell() {
        let bin = TestBin::new(
            "echo-args",
            "#!/bin/sh\nfor arg in \"$@\"; do printf '[%s]\\n' \"$arg\"; done\n",
        );
        let hostile = [
            "with space",
            "quote\"inside",
            "semi;colon",
            "$(touch /tmp/occ-should-not-exist)",
            "pipe|chain",
            "back`tick`",
            "trailing\\",
            "lead-dash",
        ];
        let spec = CommandSpec::capture(
            "echo-args",
            hostile.iter().map(|arg| (*arg).to_string()).collect(),
        );
        let output = bin.runner().run(&spec).expect("run");
        let expected: String = hostile
            .iter()
            .map(|arg| format!("[{arg}]\n"))
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(output.stdout, expected);
        assert!(!Path::new("/tmp/occ-should-not-exist").exists());
    }

    #[test]
    fn a_missing_program_is_reported_as_not_found() {
        let bin = TestBin::new("present-for-missing", "#!/bin/sh\nexit 0\n");
        let error = bin
            .runner()
            .run(&CommandSpec::capture("absent-tool", vec![]))
            .unwrap_err();
        assert_eq!(
            error,
            ProcessError::NotFound {
                program: "absent-tool".to_string()
            }
        );
    }

    #[test]
    fn a_program_path_is_rejected_instead_of_executed() {
        let bin = TestBin::new("present-for-path", "#!/bin/sh\nexit 0\n");
        let spec = CommandSpec::capture("./present", vec![]);
        let error = bin.runner().run(&spec).unwrap_err();
        assert!(matches!(error, ProcessError::InvalidProgram { .. }));
    }

    #[test]
    fn relative_or_empty_path_entries_are_ignored() {
        let bin = TestBin::new("local-tool", "#!/bin/sh\nexit 0\n");
        let mixed = std::env::join_paths([OsString::from("."), OsString::from(""), bin.path()])
            .expect("join path");
        assert!(resolve_in_path("local-tool", &mixed).is_some());

        let only_relative =
            std::env::join_paths([OsString::from("."), OsString::from("")]).expect("join path");
        assert!(resolve_in_path("local-tool", &only_relative).is_none());
    }

    #[test]
    fn a_hung_command_is_killed_at_its_deadline() {
        // An absolute path: the fixture must not depend on the PATH of whoever runs the
        // tests, or it silently becomes a fast script instead of a hanging one.
        let bin = TestBin::new("slow", "#!/bin/sh\nexec /bin/sleep 30\n");
        let spec = CommandSpec::capture("slow", vec![]).with_timeout(Duration::from_millis(150));
        let started = Instant::now();
        let error = bin.runner().run(&spec).unwrap_err();
        assert!(matches!(error, ProcessError::Timeout { .. }));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn interactive_commands_keep_their_exit_code_without_capturing() {
        let bin = TestBin::new("interactive", "#!/bin/sh\nexit 3\n");
        let output = bin
            .runner()
            .run(&CommandSpec::planned_with(
                &planned("interactive", &[], false, true),
                Some(1000),
            ))
            .expect("run");
        assert_eq!(output.exit_code, Some(3));
        assert!(!output.success());
    }

    fn planned(
        program: &str,
        args: &[&str],
        requires_root: bool,
        requires_tty: bool,
    ) -> PlannedCommand {
        PlannedCommand {
            program: program.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            requires_root,
            requires_tty,
            self_elevating: false,
            rationale: "remove the package".to_string(),
        }
    }

    /// A `sudo` stand-in that records its argv one entry per line and never runs
    /// anything: the fixture proves the wrapped argv, not a real privilege prompt.
    fn recording_sudo(bin: &TestBin) -> PathBuf {
        let record = bin.dir.join("sudo-argv");
        bin.write(
            "sudo",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
                record.display()
            ),
        );
        record
    }

    #[test]
    fn a_root_command_runs_through_sudo_with_the_same_argv() {
        let bin = TestBin::new("pacman", "#!/bin/sh\nexit 0\n");
        let record = recording_sudo(&bin);

        let spec =
            CommandSpec::planned_with(&planned("pacman", &["-R", "btop"], true, true), Some(1000));
        bin.privileged_runner().run(&spec).expect("run");

        let recorded = fs::read_to_string(&record).expect("sudo recorded its argv");
        let argv: Vec<&str> = recorded.lines().collect();
        assert_eq!(
            argv[0], "--",
            "sudo's own options must end before the command"
        );
        assert_eq!(
            argv[1],
            bin.dir.join("pacman").to_str().expect("utf8 path"),
            "sudo must run the resolved program, not a PATH lookup of its own"
        );
        assert_eq!(&argv[2..], &["-R", "btop"], "arguments stay verbatim");
    }

    #[test]
    fn a_root_command_without_sudo_is_reported_as_missing() {
        let bin = TestBin::new("pacman", "#!/bin/sh\nexit 0\n");
        let error = bin
            .privileged_runner()
            .run(&CommandSpec::planned_with(
                &planned("pacman", &["-R", "btop"], true, true),
                Some(1000),
            ))
            .expect_err("no sudo among the trusted directories");
        assert!(
            matches!(error, ProcessError::NotFound { ref program } if program == "sudo"),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn an_unprivileged_command_never_calls_sudo() {
        let bin = TestBin::new("flatpak", "#!/bin/sh\nexit 0\n");
        let record = recording_sudo(&bin);

        let output = bin
            .runner()
            .run(&CommandSpec::planned_with(
                &planned("flatpak", &["uninstall", "org.x.Y"], false, false),
                Some(1000),
            ))
            .expect("run");

        assert!(output.success());
        assert!(
            !record.exists(),
            "an unprivileged command must not go through sudo"
        );
    }

    #[test]
    fn a_privileged_command_resolves_only_from_the_trusted_directories_not_the_search_path() {
        // The escalation this test exists for: the search path holds a directory the user
        // can write, and it provides both `sudo` and `pacman`. A privileged resolution that
        // fell back to it would run the user's `pacman` as root. Both scripts leave a
        // marker, so one marker file answers whether either of them ever executed.
        let hostile = TestBin::new("sudo", "#!/bin/sh\nexit 0\n");
        let executed = hostile.dir.join("executed");
        let tripwire = format!("#!/bin/sh\ntouch '{}'\nexit 0\n", executed.display());
        hostile.write("sudo", &tripwire);
        hostile.write("pacman", &tripwire);

        let privileged =
            CommandSpec::planned_with(&planned("pacman", &["-R", "btop"], true, true), Some(1000));

        // Trusted directories provide neither `sudo` nor the program: the operation is
        // refused instead of reaching for the search path.
        let unrelated = TestBin::new("unrelated", "#!/bin/sh\nexit 0\n");
        let runner =
            SystemRunner::with_path(hostile.path()).with_trusted_paths([unrelated.dir.clone()]);
        let error = runner
            .run(&privileged)
            .expect_err("nothing privileged is trusted here");
        assert!(
            matches!(error, ProcessError::NotFound { ref program } if program == "sudo"),
            "unexpected error: {error:?}"
        );
        assert!(
            !executed.exists(),
            "no search-path script may run privileged"
        );

        // With `sudo` trusted but the program outside the trusted directories, the program
        // itself is refused: it is never elevated from where it was found.
        let sudo_only = TestBin::new("sudo", "#!/bin/sh\nexit 0\n");
        let runner =
            SystemRunner::with_path(hostile.path()).with_trusted_paths([sudo_only.dir.clone()]);
        let error = runner
            .run(&privileged)
            .expect_err("the program is not trusted");
        assert!(
            matches!(error, ProcessError::Untrusted { ref program } if program == "pacman"),
            "unexpected error: {error:?}"
        );
        assert!(
            !executed.exists(),
            "no search-path script may run privileged"
        );

        // Regression pin: an unprivileged command still resolves through the search path,
        // so the trusted list narrows privileged resolution only.
        runner
            .run(&CommandSpec::capture("pacman", vec![]))
            .expect("run unprivileged");
        assert!(
            executed.exists(),
            "an unprivileged command still uses the search path"
        );
    }

    #[test]
    fn a_root_command_owns_the_terminal_and_has_no_deadline() {
        // A human typing a password must not be killed at the capture deadline, and
        // the prompt has to reach the terminal, so the pipes of Capture cannot be used.
        let spec =
            CommandSpec::planned_with(&planned("pacman", &["-R", "btop"], true, false), Some(1000));
        assert_eq!(spec.privilege, Privilege::AsRoot);
        assert_eq!(spec.output, OutputMode::Inherit);
        assert_eq!(spec.timeout, None);
    }

    #[test]
    fn a_command_that_asks_for_itself_runs_without_sudo_but_still_owns_the_terminal() {
        // yay and paru prompt for the password themselves, so the interface must not
        // wrap them, but they still need the tty the interface handed back.
        let mut command = planned("yay", &["-R", "some-aur-pkg"], true, true);
        command.self_elevating = true;
        let spec = CommandSpec::planned_with(&command, Some(1000));
        assert_eq!(spec.privilege, Privilege::AsUser);
        assert_eq!(spec.output, OutputMode::Inherit);
        assert_eq!(spec.timeout, None);
    }

    #[test]
    fn a_plain_command_keeps_the_capture_deadline() {
        let spec = CommandSpec::planned_with(&planned("pacman", &["-Q"], false, false), Some(1000));
        assert_eq!(spec.privilege, Privilege::AsUser);
        assert_eq!(spec.output, OutputMode::Capture);
        assert_eq!(spec.timeout, Some(DEFAULT_TIMEOUT));
    }

    #[test]
    fn a_command_that_needs_root_is_only_elevated_when_we_are_not_root() {
        assert!(elevation_needed(true, Some(1000)));
        assert!(
            elevation_needed(true, None),
            "an unknown uid must not skip the sudo handoff"
        );
        assert!(
            !elevation_needed(true, Some(0)),
            "already root: sudo would be noise"
        );
        assert!(!elevation_needed(false, Some(1000)));
    }
}
