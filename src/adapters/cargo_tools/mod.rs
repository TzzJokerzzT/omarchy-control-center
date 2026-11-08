//! Cargo-installed tools.
//!
//! Commands used, and why:
//! * `cargo install --list` — every crate installed into this Cargo home, with its
//!   version and the binaries it provided.
//! * `cargo uninstall <crate>` — removal, delegated to Cargo. Deleting the executables
//!   from `~/.cargo/bin` by hand would leave Cargo's own bookkeeping lying, and the
//!   specification is explicit about not deleting executables arbitrarily.
//!
//! Cargo-installed tools are a separate ecosystem from the distribution's Rust
//! packages: this adapter never claims a package that pacman owns.

use std::sync::Arc;
use std::time::Duration;

use crate::domain::{
    AdapterAvailability, AdapterError, AdapterStatus, Component, ComponentAdapter,
    ComponentCapability, ComponentCategory, ComponentId, ComponentSource, ComponentStatus,
    DetailField, DetectionConfidence, DetectionMethod, OperationKind, OperationOutcome,
    OperationPlan, OperationRequest, PlannedCommand, tail_text,
};
use crate::infrastructure::process::{CommandOutput, CommandRunner, CommandSpec, ProcessError};
use crate::infrastructure::validate::validate_pacman_name;

const ADAPTER_ID: &str = "cargo";
const TIMEOUT: Duration = Duration::from_secs(30);

const COMMANDS: &[&str] = &["cargo install --list", "cargo uninstall <crate>"];

pub struct CargoToolsAdapter {
    runner: Arc<dyn CommandRunner>,
}

/// One line group of `cargo install --list`: `name vX.Y.Z:` followed by indented
/// binary names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledCrate {
    pub name: String,
    pub version: String,
    pub binaries: Vec<String>,
}

impl CargoToolsAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }

    fn run(&self, args: &[&str], context: &str) -> Result<CommandOutput, AdapterError> {
        let spec =
            CommandSpec::capture("cargo", args.iter().map(|arg| (*arg).to_string()).collect())
                .with_timeout(TIMEOUT);
        self.runner.run(&spec).map_err(|error| match error {
            ProcessError::NotFound { .. } => AdapterError::ToolUnavailable {
                tool: "cargo".to_string(),
                reason: "not found in the executable search path".to_string(),
            },
            other => AdapterError::command(context, spec.display(), other.to_string()),
        })
    }

    fn component(&self, tool: &InstalledCrate) -> Component {
        Component {
            id: ComponentId::new(ADAPTER_ID, tool.name.clone()),
            name: tool.name.clone(),
            version: Some(tool.version.clone()),
            description: None,
            category: ComponentCategory::Tool,
            source: ComponentSource::Cargo,
            status: ComponentStatus::Installed,
            confidence: DetectionConfidence::Authoritative,
            capabilities: vec![ComponentCapability::Inspect, ComponentCapability::Remove],
            detection: DetectionMethod::command(
                "cargo",
                vec!["install".to_string(), "--list".to_string()],
            ),
            installed_size: None,
            location: None,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: vec![DetailField::new("Binaries", summarise(&tool.binaries))],
        }
    }
}

impl ComponentAdapter for CargoToolsAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn display_name(&self) -> &'static str {
        "Cargo tools"
    }

    fn description(&self) -> &'static str {
        "Crates installed with 'cargo install', separate from Rust packages owned by pacman"
    }

    fn capabilities(&self) -> Vec<ComponentCapability> {
        vec![ComponentCapability::Inspect, ComponentCapability::Remove]
    }

    fn status(&self) -> AdapterStatus {
        let Some(tool) = self.runner.resolve("cargo") else {
            return AdapterStatus {
                id: ADAPTER_ID.to_string(),
                display_name: self.display_name().to_string(),
                description: self.description().to_string(),
                availability: AdapterAvailability::Unavailable {
                    reason: "cargo was not found in the executable search path".to_string(),
                },
                tool: None,
                tool_version: None,
                capabilities: self.capabilities(),
                commands: COMMANDS
                    .iter()
                    .map(|command| (*command).to_string())
                    .collect(),
                notes: Vec::new(),
            };
        };

        let tool_version = self
            .runner
            .run(&CommandSpec::capture(
                "cargo",
                vec!["--version".to_string()],
            ))
            .ok()
            .filter(CommandOutput::success)
            .and_then(|output| {
                output
                    .stdout
                    .lines()
                    .next()
                    .map(|line| line.trim().to_string())
            });

        AdapterStatus {
            id: ADAPTER_ID.to_string(),
            display_name: self.display_name().to_string(),
            description: self.description().to_string(),
            availability: AdapterAvailability::Available,
            tool: Some(tool),
            tool_version,
            capabilities: self.capabilities(),
            commands: COMMANDS
                .iter()
                .map(|command| (*command).to_string())
                .collect(),
            notes: vec![
                "an empty list means no crate was installed with cargo install".to_string(),
            ],
        }
    }

    fn list(&self) -> Result<Vec<Component>, AdapterError> {
        let output = self.run(&["install", "--list"], "cargo install --list")?;
        if !output.success() {
            return Err(AdapterError::command(
                "cargo install --list",
                "cargo install --list",
                format!("exit {:?}: {}", output.exit_code, output.stderr.trim()),
            ));
        }
        Ok(parse_install_list(&output.stdout)
            .iter()
            .map(|tool| self.component(tool))
            .collect())
    }

    fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError> {
        if let Err(reason) = validate_pacman_name(native_id) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: native_id.to_string(),
                reason: reason.to_string(),
            });
        }
        Ok(self
            .list()?
            .into_iter()
            .find(|component| component.id.native_id == native_id))
    }

    fn supports(&self, component: &Component, kind: OperationKind) -> bool {
        kind == OperationKind::Remove && matches!(component.source, ComponentSource::Cargo)
    }

    fn plan_operation(
        &self,
        request: &OperationRequest,
        component: &Component,
    ) -> Result<OperationPlan, AdapterError> {
        if request.kind != OperationKind::Remove {
            return Err(AdapterError::unsupported(ADAPTER_ID, request.kind.label()));
        }
        let name = &request.target.native_id;
        if let Err(reason) = validate_pacman_name(name) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: name.clone(),
                reason: reason.to_string(),
            });
        }

        // Re-read the list: the plan must describe what cargo knows right now.
        let installed = self.list()?;
        let Some(tool) = installed.iter().find(|tool| &tool.id.native_id == name) else {
            return Err(AdapterError::command(
                "cargo install --list",
                "cargo install --list",
                format!("{name} is not installed by cargo any more"),
            ));
        };

        Ok(OperationPlan {
            adapter: ADAPTER_ID.to_string(),
            target: request.target.clone(),
            target_name: component.name.clone(),
            target_version: component.version.clone(),
            kind: OperationKind::Remove,
            commands: vec![PlannedCommand {
                program: "cargo".to_string(),
                args: vec!["uninstall".to_string(), name.clone()],
                // A cargo install writes inside the user's own Cargo home.
                requires_root: false,
                requires_tty: false,
                rationale: "remove this crate and the binaries cargo recorded for it".to_string(),
            }],
            affected: tool
                .extra
                .iter()
                .filter(|field| field.label == "Binaries")
                .map(|field| format!("binaries: {}", field.value))
                .collect(),
            blocking: Vec::new(),
            unknown: vec![
                "whether another installation provides a binary with the same name is not \
                 reported by cargo"
                    .to_string(),
                "whether CARGO_HOME points inside a system directory, which would need \
                 privileges, is not checked"
                    .to_string(),
            ],
            notes: vec![
                "the removal is delegated to cargo, so its own bookkeeping stays consistent"
                    .to_string(),
                "the operation is not reversible without installing the crate again".to_string(),
            ],
        })
    }

    fn execute(&self, plan: &OperationPlan) -> Result<OperationOutcome, AdapterError> {
        if plan.adapter != ADAPTER_ID {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: plan.adapter.clone(),
                reason: "this plan belongs to another adapter".to_string(),
            });
        }
        if let Err(reason) = validate_pacman_name(&plan.target.native_id) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: plan.target.native_id.clone(),
                reason: reason.to_string(),
            });
        }

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = None;
        let mut command_display = String::new();

        for command in &plan.commands {
            if command.program != "cargo" {
                return Err(AdapterError::InvalidId {
                    adapter: ADAPTER_ID.to_string(),
                    value: command.program.clone(),
                    reason: "this adapter only runs cargo".to_string(),
                });
            }
            command_display = command.display();
            let spec = if command.requires_tty {
                CommandSpec::interactive(command.program.clone(), command.args.clone())
            } else {
                CommandSpec::capture(command.program.clone(), command.args.clone())
                    .with_timeout(TIMEOUT)
            };
            let output = self.runner.run(&spec).map_err(|error| {
                AdapterError::command("execute", command.display(), error.to_string())
            })?;
            stdout.push_str(&output.stdout);
            stderr.push_str(&output.stderr);
            exit_code = output.exit_code;
            if !output.success() {
                break;
            }
        }

        let success = exit_code == Some(0);
        Ok(OperationOutcome {
            success,
            exit_code,
            command_display,
            stdout_tail: tail_text(&stdout, 2000),
            stderr_tail: tail_text(&stderr, 2000),
            summary: if success {
                format!("removed cargo tool {}", plan.target_name)
            } else {
                format!("cargo did not complete the removal (exit {exit_code:?})")
            },
        })
    }
}

/// Parses `cargo install --list`:
///
/// ```text
/// ripgrep v14.1.0:
///     rg
/// just v1.36.0:
///     just
/// ```
pub fn parse_install_list(text: &str) -> Vec<InstalledCrate> {
    let mut tools: Vec<InstalledCrate> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if indented {
            if let Some(current) = tools.last_mut() {
                let binary = line.trim().to_string();
                if !binary.is_empty() {
                    current.binaries.push(binary);
                }
            }
            continue;
        }

        // `name vX.Y.Z:` — the trailing colon is always present.
        let Some((head, _)) = line.split_once(':') else {
            continue;
        };
        let head = head.trim();
        let Some((name, version)) = head.rsplit_once(" v") else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || version.trim().is_empty() {
            continue;
        }
        tools.push(InstalledCrate {
            name: name.to_string(),
            version: version.trim().to_string(),
            binaries: Vec::new(),
        });
    }
    tools
}

fn summarise(items: &[String]) -> String {
    if items.is_empty() {
        return "No disponible".to_string();
    }
    items.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::test_support::FakeRunner;

    const LIST: &str = include_str!("../../../tests/fixtures/cargo/install-list.txt");

    fn adapter(runner: FakeRunner) -> CargoToolsAdapter {
        CargoToolsAdapter::new(Arc::new(runner))
    }

    fn runner() -> FakeRunner {
        FakeRunner::new()
            .on("cargo --version", "cargo 1.99.1 (5f94df478 2026-08-27)\n")
            .on("cargo install --list", LIST)
    }

    #[test]
    fn reports_itself_unavailable_without_cargo() {
        let status = adapter(FakeRunner::new()).status();
        assert!(!status.available());
        assert!(status.tool.is_none());
    }

    #[test]
    fn parses_names_versions_and_binaries() {
        let tools = parse_install_list(LIST);
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "ripgrep");
        assert_eq!(tools[0].version, "14.1.0");
        assert_eq!(tools[0].binaries, vec!["rg".to_string()]);
        assert_eq!(tools[1].name, "just");
        assert_eq!(tools[2].version, "1.12.3");
    }

    #[test]
    fn an_empty_list_is_an_empty_inventory_not_an_error() {
        let adapter = adapter(FakeRunner::new().on("cargo install --list", ""));
        assert!(adapter.list().expect("list").is_empty());
    }

    #[test]
    fn lists_one_component_per_crate_with_its_binaries() {
        let components = adapter(runner()).list().expect("list");
        assert_eq!(components.len(), 3);
        let ripgrep = components
            .iter()
            .find(|component| component.name == "ripgrep")
            .expect("ripgrep");
        assert_eq!(ripgrep.id.to_string(), "cargo:ripgrep");
        assert_eq!(ripgrep.version.as_deref(), Some("14.1.0"));
        assert_eq!(ripgrep.category, ComponentCategory::Tool);
        assert_eq!(ripgrep.source, ComponentSource::Cargo);
        assert!(
            ripgrep
                .extra
                .iter()
                .any(|field| field.label == "Binaries" && field.value == "rg")
        );
    }

    #[test]
    fn a_failing_list_is_an_error_not_an_empty_inventory() {
        let adapter = adapter(FakeRunner::new().on_exit(
            "cargo install --list",
            1,
            "",
            "error: could not read the install metadata\n",
        ));
        assert!(matches!(
            adapter.list().expect_err("must fail"),
            AdapterError::Command { .. }
        ));
    }

    #[test]
    fn removal_is_delegated_to_cargo_uninstall() {
        let components = adapter(runner()).list().expect("list");
        let ripgrep = components
            .iter()
            .find(|component| component.name == "ripgrep")
            .expect("ripgrep")
            .clone();
        let request = OperationRequest::new(ripgrep.id.clone(), OperationKind::Remove);
        let plan = adapter(runner())
            .plan_operation(&request, &ripgrep)
            .expect("plan");

        assert_eq!(plan.commands[0].display(), "cargo uninstall ripgrep");
        assert!(!plan.commands[0].requires_root);
        assert!(plan.is_executable());
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("delegated to cargo"))
        );
    }

    #[test]
    fn execution_runs_exactly_the_confirmed_command() {
        let runner = runner().on_exit("cargo uninstall ripgrep", 0, "uninstalled\n", "");
        let runner = Arc::new(runner);
        let adapter = CargoToolsAdapter::new(Arc::clone(&runner) as Arc<dyn CommandRunner>);
        let components = adapter.list().expect("list");
        let ripgrep = components
            .iter()
            .find(|component| component.name == "ripgrep")
            .expect("ripgrep")
            .clone();
        let request = OperationRequest::new(ripgrep.id.clone(), OperationKind::Remove);
        let plan = adapter.plan_operation(&request, &ripgrep).expect("plan");

        let outcome = adapter.execute(&plan).expect("execute");
        assert!(outcome.success);
        assert!(
            runner
                .calls()
                .contains(&"cargo uninstall ripgrep".to_string())
        );
    }

    #[test]
    fn a_hostile_crate_name_is_refused_before_any_process_runs() {
        let request = OperationRequest::new(
            ComponentId::new("cargo", "ripgrep; rm -rf /"),
            OperationKind::Remove,
        );
        let components = adapter(runner()).list().expect("list");
        let component = components.first().expect("one").clone();
        let error = adapter(runner())
            .plan_operation(&request, &component)
            .expect_err("must refuse");
        assert!(
            matches!(error, AdapterError::InvalidId { .. }),
            "unexpected: {error}"
        );
    }

    #[test]
    fn a_failing_uninstall_is_never_reported_as_success() {
        let runner = runner().on_exit(
            "cargo uninstall ripgrep",
            1,
            "",
            "error: permission denied\n",
        );
        let runner = Arc::new(runner);
        let adapter = CargoToolsAdapter::new(Arc::clone(&runner) as Arc<dyn CommandRunner>);
        let components = adapter.list().expect("list");
        let ripgrep = components
            .iter()
            .find(|component| component.name == "ripgrep")
            .expect("ripgrep")
            .clone();
        let request = OperationRequest::new(ripgrep.id.clone(), OperationKind::Remove);
        let plan = adapter.plan_operation(&request, &ripgrep).expect("plan");
        let outcome = adapter.execute(&plan).expect("execute");
        assert!(!outcome.success);
        assert_eq!(outcome.exit_code, Some(1));
    }
}
