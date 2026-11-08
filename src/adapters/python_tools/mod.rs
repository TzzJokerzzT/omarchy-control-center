//! Python tools installed in their own environment.
//!
//! Commands used, and why:
//! * `uv tool list` — the tools a user installed with `uv tool install`.
//! * `pipx list --json` — the same thing for pipx, in the only shape pipx documents.
//! * `--version` for whichever manager was found, so the adapters view names it.
//!
//! Deliberate limits, stated in the code because they matter:
//! * `pip` is never consulted. A `pip install` can modify the Python that the
//!   distribution owns, and the specification forbids touching that without verifying
//!   the environment.
//! * Tools are reported per manager. A virtualenv is not a tool, and a tool installed
//!   by two managers is two components in two managers' books, not one guess.
//! * No removal is offered in this version. Doing it safely means proving which
//!   environment owns the executable, and this adapter cannot prove that yet — so it
//!   inspects and says so instead of offering an action it has not verified.

use std::sync::Arc;
use std::time::Duration;

use crate::domain::{
    AdapterAvailability, AdapterError, AdapterStatus, Component, ComponentAdapter,
    ComponentCapability, ComponentCategory, ComponentId, ComponentSource, ComponentStatus,
    DetailField, DetectionConfidence, DetectionMethod, OperationKind, OperationOutcome,
    OperationPlan, OperationRequest,
};
use crate::infrastructure::process::{CommandOutput, CommandRunner, CommandSpec, ProcessError};
use crate::infrastructure::validate::validate_pacman_name;

const ADAPTER_ID: &str = "python";
const TIMEOUT: Duration = Duration::from_secs(30);

/// Preference order. `uv` first: it is the manager Omarchy's own tooling leans on.
const MANAGERS: &[&str] = &["uv", "pipx"];

const COMMANDS: &[&str] = &["uv tool list", "pipx list --json"];

pub struct PythonToolsAdapter {
    runner: Arc<dyn CommandRunner>,
}

/// A tool one manager reports, plus the manager's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledTool {
    pub name: String,
    pub version: Option<String>,
    pub binaries: Vec<String>,
    pub manager: &'static str,
}

impl PythonToolsAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }

    /// A manager is usable only when it resolves to a real executable. A mise shim
    /// (or any other lazy installer) is refused: running it can install software as a
    /// side effect, and an inventory must never install anything.
    fn manager(&self) -> Option<&'static str> {
        MANAGERS.iter().copied().find(|program| {
            self.runner
                .resolve(program)
                .is_some_and(|path| !is_lazy_shim(&path))
        })
    }

    /// Why no manager is usable. Distinguishes "not installed" from "only a shim",
    /// because the second one is not the user's fault and the reason matters.
    fn unavailable_reason(&self) -> String {
        let shims: Vec<&str> = MANAGERS
            .iter()
            .copied()
            .filter(|program| {
                self.runner
                    .resolve(program)
                    .is_some_and(|path| is_lazy_shim(&path))
            })
            .collect();
        if shims.is_empty() {
            "neither uv nor pipx was found in the executable search path".to_string()
        } else {
            format!(
                "{} resolve(s) to a lazy-install shim (mise); running it would install the tool, \
                 so it is not invoked here",
                shims.join(" / ")
            )
        }
    }

    fn run(
        &self,
        program: &str,
        args: &[&str],
        context: &str,
    ) -> Result<CommandOutput, AdapterError> {
        let spec =
            CommandSpec::capture(program, args.iter().map(|arg| (*arg).to_string()).collect())
                .with_timeout(TIMEOUT);
        self.runner.run(&spec).map_err(|error| match error {
            ProcessError::NotFound { .. } => AdapterError::ToolUnavailable {
                tool: program.to_string(),
                reason: "not found in the executable search path".to_string(),
            },
            other => AdapterError::command(context, spec.display(), other.to_string()),
        })
    }

    fn tools(&self, manager: &'static str) -> Result<Vec<InstalledTool>, AdapterError> {
        match manager {
            "uv" => {
                let output = self.run("uv", &["tool", "list"], "uv tool list")?;
                if !output.success() {
                    return Err(AdapterError::command(
                        "uv tool list",
                        "uv tool list",
                        format!("exit {:?}: {}", output.exit_code, output.stderr.trim()),
                    ));
                }
                Ok(parse_uv_tool_list(&output.stdout))
            }
            "pipx" => {
                let output = self.run("pipx", &["list", "--json"], "pipx list --json")?;
                if !output.success() {
                    return Err(AdapterError::command(
                        "pipx list --json",
                        "pipx list --json",
                        format!("exit {:?}: {}", output.exit_code, output.stderr.trim()),
                    ));
                }
                parse_pipx_list(&output.stdout)
            }
            other => Err(AdapterError::unsupported(
                ADAPTER_ID,
                format!("listing tools of {other}"),
            )),
        }
    }

    fn component(&self, tool: &InstalledTool) -> Component {
        Component {
            id: ComponentId::new(ADAPTER_ID, format!("{}:{}", tool.manager, tool.name)),
            name: tool.name.clone(),
            version: tool.version.clone(),
            description: None,
            category: ComponentCategory::Tool,
            source: ComponentSource::Python {
                manager: tool.manager.to_string(),
            },
            status: ComponentStatus::Installed,
            confidence: DetectionConfidence::Authoritative,
            capabilities: vec![ComponentCapability::Inspect],
            detection: if tool.manager == "uv" {
                DetectionMethod::command("uv", vec!["tool".to_string(), "list".to_string()])
            } else {
                DetectionMethod::command("pipx", vec!["list".to_string(), "--json".to_string()])
            },
            installed_size: None,
            location: None,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: vec![
                DetailField::new("Manager", tool.manager),
                DetailField::new(
                    "Binaries",
                    if tool.binaries.is_empty() {
                        "No disponible".to_string()
                    } else {
                        tool.binaries.join(", ")
                    },
                ),
            ],
        }
    }
}

impl ComponentAdapter for PythonToolsAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn display_name(&self) -> &'static str {
        "Python tools"
    }

    fn description(&self) -> &'static str {
        "Tools installed by uv or pipx; the system Python is never touched"
    }

    fn capabilities(&self) -> Vec<ComponentCapability> {
        vec![ComponentCapability::Inspect]
    }

    fn status(&self) -> AdapterStatus {
        let Some(manager) = self.manager() else {
            return AdapterStatus {
                id: ADAPTER_ID.to_string(),
                display_name: self.display_name().to_string(),
                description: self.description().to_string(),
                availability: AdapterAvailability::Unavailable {
                    reason: self.unavailable_reason(),
                },
                tool: None,
                tool_version: None,
                capabilities: self.capabilities(),
                commands: COMMANDS
                    .iter()
                    .map(|command| (*command).to_string())
                    .collect(),
                notes: vec![
                    "pip is never consulted: it can modify the distribution's Python".to_string(),
                ],
            };
        };

        let tool = self.runner.resolve(manager);
        let tool_version = self
            .runner
            .run(&CommandSpec::capture(
                manager,
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
            tool,
            tool_version,
            capabilities: self.capabilities(),
            commands: COMMANDS
                .iter()
                .map(|command| (*command).to_string())
                .collect(),
            notes: vec![
                format!("using {manager}; the other manager is not consulted"),
                "no removal is offered: proving which environment owns an executable is not \
                 implemented"
                    .to_string(),
            ],
        }
    }

    fn list(&self) -> Result<Vec<Component>, AdapterError> {
        let Some(manager) = self.manager() else {
            return Err(AdapterError::ToolUnavailable {
                tool: MANAGERS.join(" / "),
                reason: self.unavailable_reason(),
            });
        };
        Ok(self
            .tools(manager)?
            .iter()
            .map(|tool| self.component(tool))
            .collect())
    }

    fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError> {
        let (manager, name) = native_id
            .split_once(':')
            .ok_or_else(|| AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: native_id.to_string(),
                reason: "expected <manager>:<tool>".to_string(),
            })?;
        if !MANAGERS.contains(&manager) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: native_id.to_string(),
                reason: "unknown Python tool manager".to_string(),
            });
        }
        if let Err(reason) = validate_pacman_name(name) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: name.to_string(),
                reason: reason.to_string(),
            });
        }
        Ok(self
            .list()?
            .into_iter()
            .find(|component| component.id.native_id == native_id))
    }

    fn supports(&self, _component: &Component, _kind: OperationKind) -> bool {
        false
    }

    fn plan_operation(
        &self,
        _request: &OperationRequest,
        _component: &Component,
    ) -> Result<OperationPlan, AdapterError> {
        Err(AdapterError::unsupported(
            ADAPTER_ID,
            "operations (which environment owns an executable cannot be proven yet)",
        ))
    }

    fn execute(&self, _plan: &OperationPlan) -> Result<OperationOutcome, AdapterError> {
        Err(AdapterError::unsupported(ADAPTER_ID, "operations"))
    }
}

/// A path under a `mise/shims` directory is a placeholder that installs the real tool
/// on first execution. Recognising it is what keeps this adapter side-effect free.
fn is_lazy_shim(path: &std::path::Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "shims")
        && path
            .components()
            .any(|component| component.as_os_str() == "mise")
}

/// Parses `uv tool list`:
///
/// ```text
/// ruff v0.6.9
/// - ruff
/// black v24.8.0
/// - black
/// - blackd
/// ```
pub fn parse_uv_tool_list(text: &str) -> Vec<InstalledTool> {
    let mut tools: Vec<InstalledTool> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(binary) = trimmed.strip_prefix("- ") {
            if let Some(current) = tools.last_mut() {
                current.binaries.push(binary.trim().to_string());
            }
            continue;
        }
        if let Some((name, version)) = trimmed.rsplit_once(" v") {
            tools.push(InstalledTool {
                name: name.trim().to_string(),
                version: Some(version.trim().to_string()),
                binaries: Vec::new(),
                manager: "uv",
            });
        } else {
            // uv prints a bare name when a tool has no version to report.
            tools.push(InstalledTool {
                name: trimmed.to_string(),
                version: None,
                binaries: Vec::new(),
                manager: "uv",
            });
        }
    }
    tools
}

/// Parses `pipx list --json`. Everything is optional: a missing version leaves the
/// field `No disponible` instead of being invented.
pub fn parse_pipx_list(text: &str) -> Result<Vec<InstalledTool>, AdapterError> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|error| AdapterError::Parse {
            context: "pipx list --json".to_string(),
            detail: error.to_string(),
        })?;

    let Some(venvs) = json.get("venvs").and_then(|value| value.as_object()) else {
        // pipx prints a JSON object even with no venvs, but never invent an empty
        // structure from something that does not look like one.
        return Err(AdapterError::Parse {
            context: "pipx list --json".to_string(),
            detail: "no 'venvs' object in the output".to_string(),
        });
    };

    let mut tools = Vec::new();
    for (name, venv) in venvs {
        let version = venv
            .pointer("/metadata/main_package/package_version")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let binaries = venv
            .get("metadata")
            .and_then(|metadata| metadata.get("apps"))
            .and_then(|apps| apps.as_array())
            .map(|apps| {
                apps.iter()
                    .filter_map(|app| app.as_str().map(|value| value.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        tools.push(InstalledTool {
            name: name.to_string(),
            version,
            binaries,
            manager: "pipx",
        });
    }
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::test_support::FakeRunner;

    const UV: &str = include_str!("../../../tests/fixtures/python/uv-tool-list.txt");

    fn adapter(runner: FakeRunner) -> PythonToolsAdapter {
        PythonToolsAdapter::new(Arc::new(runner))
    }

    fn uv_runner() -> FakeRunner {
        FakeRunner::new()
            .on("uv --version", "uv 0.4.18 (e4f4f2e 2024-09-24)\n")
            .on("uv tool list", UV)
    }

    #[test]
    fn reports_itself_unavailable_without_a_manager_and_never_falls_back_to_pip() {
        let status = adapter(FakeRunner::new()).status();
        assert!(!status.available());
        assert_eq!(
            status.availability,
            AdapterAvailability::Unavailable {
                reason: "neither uv nor pipx was found in the executable search path".to_string()
            }
        );
        assert!(
            status
                .notes
                .iter()
                .any(|note| note.contains("pip is never consulted"))
        );
        assert!(
            !status
                .commands
                .iter()
                .any(|command| command.starts_with("pip "))
        );
    }

    #[test]
    fn a_lazy_install_shim_is_unavailable_and_never_invoked() {
        struct ShimRunner;
        impl CommandRunner for ShimRunner {
            fn resolve(&self, program: &str) -> Option<std::path::PathBuf> {
                (program == "uv")
                    .then(|| std::path::PathBuf::from("/home/user/.local/share/mise/shims/uv"))
            }
            fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, ProcessError> {
                panic!("no command may run through a shim: {}", spec.display());
            }
        }

        let adapter = PythonToolsAdapter::new(Arc::new(ShimRunner));
        let status = adapter.status();
        assert!(!status.available());
        let reason = match &status.availability {
            AdapterAvailability::Unavailable { reason } => reason.clone(),
            AdapterAvailability::Available => panic!("a shim must not count as available"),
        };
        assert!(
            reason.contains("lazy-install shim"),
            "unexpected reason: {reason}"
        );
        // list() is never reached by the registry for an unavailable adapter, and if it
        // were, it refuses instead of running anything.
        assert!(adapter.list().is_err());
    }

    #[test]
    fn parses_uv_tool_list_with_its_binaries() {
        let tools = parse_uv_tool_list(UV);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "ruff");
        assert_eq!(tools[0].version.as_deref(), Some("0.6.9"));
        assert_eq!(tools[0].binaries, vec!["ruff".to_string()]);
        assert_eq!(
            tools[1].binaries,
            vec!["black".to_string(), "blackd".to_string()]
        );
    }

    #[test]
    fn prefers_uv_and_inspects_only() {
        let adapter = adapter(uv_runner());
        let status = adapter.status();
        assert!(status.available());
        assert_eq!(
            status.tool_version.as_deref(),
            Some("uv 0.4.18 (e4f4f2e 2024-09-24)")
        );
        assert_eq!(status.capabilities, vec![ComponentCapability::Inspect]);

        let components = adapter.list().expect("list");
        assert_eq!(components.len(), 2);
        let ruff = components
            .iter()
            .find(|component| component.name == "ruff")
            .expect("ruff");
        assert_eq!(ruff.id.to_string(), "python:uv:ruff");
        assert_eq!(
            ruff.source,
            ComponentSource::Python {
                manager: "uv".to_string()
            }
        );
        assert!(
            ruff.extra
                .iter()
                .any(|field| field.label == "Manager" && field.value == "uv")
        );
    }

    #[test]
    fn falls_back_to_pipx_when_uv_is_absent() {
        let runner = FakeRunner::new().on(
            "pipx list --json",
            r#"{"venvs":{"poetry":{"metadata":{"main_package":{"package_version":"1.8.3"},"apps":["poetry"]}}}}"#,
        );
        let components = adapter(runner).list().expect("list");
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].name, "poetry");
        assert_eq!(components[0].version.as_deref(), Some("1.8.3"));
        assert_eq!(components[0].id.to_string(), "python:pipx:poetry");
    }

    #[test]
    fn a_missing_version_stays_unavailable_instead_of_being_invented() {
        let runner = FakeRunner::new().on(
            "pipx list --json",
            r#"{"venvs":{"tool":{"metadata":{"main_package":{}}}}}"#,
        );
        let components = adapter(runner).list().expect("list");
        assert!(components[0].version.is_none());
    }

    #[test]
    fn invalid_pipx_json_is_a_parse_error_not_an_empty_list() {
        let error = parse_pipx_list("not json").expect_err("must fail");
        assert!(matches!(error, AdapterError::Parse { .. }));
        let error = parse_pipx_list("{}").expect_err("must fail");
        assert!(matches!(error, AdapterError::Parse { .. }));
    }

    #[test]
    fn operations_are_refused_explicitly() {
        let adapter = adapter(uv_runner());
        let component = adapter
            .list()
            .expect("list")
            .into_iter()
            .next()
            .expect("one");
        assert!(!adapter.supports(&component, OperationKind::Remove));
        let request = OperationRequest::new(component.id.clone(), OperationKind::Remove);
        assert!(adapter.plan_operation(&request, &component).is_err());
    }

    #[test]
    fn details_rejects_a_malformed_identity() {
        let adapter = adapter(uv_runner());
        assert!(adapter.details("uv").is_err());
        assert!(adapter.details("pip:ruff").is_err());
        assert!(adapter.details("uv:ruff; rm -rf /").is_err());
    }
}
