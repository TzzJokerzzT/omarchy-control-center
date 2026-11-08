//! Test doubles for the process layer.
//!
//! Adapter tests never touch the host: they register the exact commands an
//! adapter is allowed to run and get a canned result back. A command that was not
//! registered fails loudly, so an adapter cannot silently start running something
//! new in a test.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use crate::infrastructure::process::{
    CommandOutput, CommandRunner, CommandSpec, ProcessError,
};

#[derive(Default)]
pub struct FakeRunner {
    responses: BTreeMap<String, Result<CommandOutput, ProcessError>>,
    programs: Vec<String>,
    calls: Mutex<Vec<String>>,
}

impl FakeRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares a program as present on the machine (it resolves in PATH).
    pub fn with_program(mut self, program: &str) -> Self {
        if !self.programs.iter().any(|known| known == program) {
            self.programs.push(program.to_string());
        }
        self
    }

    /// Registers a successful result for one exact command line.
    pub fn on(mut self, command: &str, stdout: &str) -> Self {
        let program = command.split(' ').next().unwrap_or_default();
        self.programs.push(program.to_string());
        self.responses.insert(
            command.to_string(),
            Ok(CommandOutput {
                exit_code: Some(0),
                stdout: stdout.to_string(),
                stderr: String::new(),
            }),
        );
        self
    }

    /// Registers a non-zero exit, the way a real tool reports "no results".
    pub fn on_exit(mut self, command: &str, code: i32, stdout: &str, stderr: &str) -> Self {
        let program = command.split(' ').next().unwrap_or_default();
        self.programs.push(program.to_string());
        self.responses.insert(
            command.to_string(),
            Ok(CommandOutput {
                exit_code: Some(code),
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
            }),
        );
        self
    }

    /// Registers a process-level failure (missing tool, timeout, cancellation).
    pub fn on_error(mut self, command: &str, error: ProcessError) -> Self {
        self.responses.insert(command.to_string(), Err(error));
        self
    }

    /// Every command that was actually attempted, in order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().map(|calls| calls.clone()).unwrap_or_default()
    }
}

impl CommandRunner for FakeRunner {
    fn resolve(&self, program: &str) -> Option<PathBuf> {
        self.programs
            .iter()
            .any(|known| known == program)
            .then(|| PathBuf::from(format!("/fake/bin/{program}")))
    }

    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ProcessError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(spec.display());
        }
        match self.responses.get(&spec.display()) {
            Some(result) => result.clone(),
            None => Err(ProcessError::NotFound { program: format!("{} (not registered)", spec.display()) }),
        }
    }

    fn run_cancellable(
        &self,
        spec: &CommandSpec,
        _cancel: &AtomicBool,
    ) -> Result<CommandOutput, ProcessError> {
        self.run(spec)
    }
}
