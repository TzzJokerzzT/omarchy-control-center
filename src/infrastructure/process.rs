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

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProcessError {
    #[error("{program} was not found in the executable search path")]
    NotFound { program: String },

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
}

impl CommandSpec {
    pub fn capture(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            timeout: Some(DEFAULT_TIMEOUT),
            output: OutputMode::Capture,
        }
    }

    pub fn interactive(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            timeout: None,
            output: OutputMode::Inherit,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
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
#[derive(Debug, Clone, Default)]
pub struct SystemRunner {
    path: Option<OsString>,
}

impl SystemRunner {
    pub fn new() -> Self {
        Self { path: None }
    }

    /// Prepends `dirs` to the inherited search path, keeping their order: the first
    /// directory that provides a program wins.
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
        }
    }

    /// Uses exactly `path`, ignoring the environment.
    #[cfg(test)]
    pub fn with_path(path: impl Into<OsString>) -> Self {
        Self {
            path: Some(path.into()),
        }
    }

    fn inherited_path() -> OsString {
        std::env::var_os("PATH").unwrap_or_default()
    }

    fn effective_path(&self) -> OsString {
        self.path.clone().unwrap_or_else(Self::inherited_path)
    }

    fn build(&self, spec: &CommandSpec) -> Result<Command, ProcessError> {
        let resolved = self
            .resolve(&spec.program)
            .ok_or_else(|| ProcessError::NotFound {
                program: spec.program.clone(),
            })?;

        let mut command = Command::new(resolved);
        command.args(&spec.args);
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

    fn run_inner(&self, spec: &CommandSpec) -> Result<CommandOutput, ProcessError> {
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
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    struct TestBin {
        dir: PathBuf,
    }

    impl TestBin {
        fn new(name: &str, script: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("occ-proc-{}-{}", std::process::id(), name));
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
        let bin = TestBin::new("present", "#!/bin/sh\nexit 0\n");
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
        let bin = TestBin::new("present", "#!/bin/sh\nexit 0\n");
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
        let bin = TestBin::new("slow", "#!/bin/sh\nsleep 30\n");
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
            .run(&CommandSpec::interactive("interactive", vec![]))
            .expect("run");
        assert_eq!(output.exit_code, Some(3));
        assert!(!output.success());
    }
}
