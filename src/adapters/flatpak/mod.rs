//! Flatpak adapter: applications and runtimes, in two installation scopes.
//!
//! Flatpak is optional by design (decision D6): it is compiled in and detected at
//! runtime, never a build or install dependency. When `flatpak` is missing this
//! adapter reports itself unavailable and every other adapter keeps working.
//!
//! Commands used, and why:
//! * `flatpak --version` — tool identity shown in the adapters view.
//! * `flatpak list --app --columns=ref,application,version,origin,installation,size`
//!   — installed applications, one row each, with the scope flatpak itself
//!   reports.
//! * `flatpak list --runtime --columns=...` — installed runtimes, same shape.
//! * `flatpak info <ref>` — the detail block of one component.
//!
//! This adapter never installs, updates or removes anything yet: its only
//! capability is inspection.

pub mod parse;

use std::sync::Arc;
use std::time::Duration;

use crate::domain::InstallScope;
use crate::domain::{
    AdapterAvailability, AdapterError, AdapterStatus, Component, ComponentAdapter,
    ComponentCapability, ComponentCategory, ComponentId, ComponentSource, ComponentStatus,
    DetailField, DetectionConfidence, DetectionMethod, OperationKind, OperationOutcome,
    OperationPlan, OperationRequest,
};
use crate::infrastructure::process::{CommandOutput, CommandRunner, CommandSpec, ProcessError};
use crate::infrastructure::validate::validate_flatpak_ref;

use self::parse::{Info, ParsedRow, parse_info, parse_list, parse_scope, parse_size};

const ADAPTER_ID: &str = "flatpak";
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
const COLUMNS: &str = "ref,application,version,origin,installation,size";

const LIST_APP: &str =
    "flatpak list --app --columns=ref,application,version,origin,installation,size";
const LIST_RUNTIME: &str =
    "flatpak list --runtime --columns=ref,application,version,origin,installation,size";

const COMMANDS: &[&str] = &[
    "flatpak --version",
    LIST_APP,
    LIST_RUNTIME,
    "flatpak info <ref>",
    "flatpak uninstall --user|--system --noninteractive --no-related <ref>",
    "flatpak update --user|--system --noninteractive <ref>",
];

pub struct FlatpakAdapter {
    runner: Arc<dyn CommandRunner>,
}

impl FlatpakAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }

    fn run(&self, args: Vec<String>, context: &str) -> Result<CommandOutput, AdapterError> {
        let spec = CommandSpec::capture("flatpak", args).with_timeout(LIST_TIMEOUT);

        self.runner.run(&spec).map_err(|error| match error {
            ProcessError::NotFound { .. } => AdapterError::ToolUnavailable {
                tool: "flatpak".to_string(),
                reason: "not found in the executable search path".to_string(),
            },
            other => AdapterError::command(context, spec.display(), other.to_string()),
        })
    }

    /// Runs one `list` invocation and converts its rows into components.
    ///
    /// A row that does not carry its own facts (no known installation, wrong
    /// column count) is skipped and warned about: fabricating a component from a
    /// half-read row would invent an identity or a scope.
    fn list_kind(
        &self,
        kind: &str,
        category: ComponentCategory,
    ) -> Result<Vec<Component>, AdapterError> {
        let args = vec![
            "list".to_string(),
            kind.to_string(),
            format!("--columns={COLUMNS}"),
        ];
        let detection = DetectionMethod::command("flatpak", args.clone());
        let command_display = detection.label();

        let output = self.run(args, "flatpak list")?;
        if !output.success() {
            return Err(AdapterError::command(
                "flatpak list",
                command_display,
                format!("exit {:?}: {}", output.exit_code, output.stderr.trim()),
            ));
        }

        let mut components = Vec::new();
        for row in parse_list(&output.stdout) {
            match row {
                ParsedRow::Row(row) => {
                    components.push(self.component_from_row(&row, category, &detection))
                }
                ParsedRow::Skipped { line, reason } => {
                    tracing::warn!(%reason, line = %line, "skipping a flatpak list row")
                }
            }
        }
        Ok(components)
    }

    fn component_from_row(
        &self,
        row: &parse::ListRow,
        category: ComponentCategory,
        detection: &DetectionMethod,
    ) -> Component {
        let mut extra = Vec::new();
        if let Some(origin) = &row.origin {
            extra.push(DetailField::new("Origin", origin));
        }

        Component {
            id: ComponentId::new(ADAPTER_ID, row.reference.clone()),
            name: row.application.clone(),
            version: row.version.clone(),
            description: None,
            category,
            source: ComponentSource::Flatpak { scope: row.scope },
            status: ComponentStatus::Installed,
            confidence: DetectionConfidence::Authoritative,
            capabilities: flatpak_capabilities(),
            detection: detection.clone(),
            installed_size: row.installed_size,
            location: None,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra,
        }
    }

    fn component_from_info(&self, native_id: &str, info: &Info) -> Result<Component, AdapterError> {
        // The scope is part of the identity and `flatpak info` always states it.
        // Without it the component would be mislabelled, so this is an error.
        let Some(scope) = info.get("Installation").and_then(parse_scope) else {
            return Err(AdapterError::Parse {
                context: "flatpak info".to_string(),
                detail: format!(
                    "no usable Installation field: the scope of {native_id} is unknown"
                ),
            });
        };

        // The ref reported by flatpak is the identity it will answer to; it is
        // validated before it becomes one, and the request stands in if absent.
        let reference = match info.get("Ref") {
            Some(reference) if validate_flatpak_ref(reference).is_ok() => reference.to_string(),
            _ => native_id.to_string(),
        };

        let Some(category) = category_from_ref(&reference) else {
            return Err(AdapterError::Parse {
                context: "flatpak info".to_string(),
                detail: format!(
                    "{reference} does not say whether it is an application or a runtime"
                ),
            });
        };

        let name = info
            .header_name()
            .map(str::to_string)
            .or_else(|| application_from_ref(&reference))
            .unwrap_or_else(|| reference.clone());

        let mut extra = Vec::new();
        for (label, key) in [
            ("ID", "ID"),
            ("Architecture", "Arch"),
            ("Branch", "Branch"),
            ("License", "License"),
            ("Origin", "Origin"),
            ("Collection", "Collection"),
            ("Runtime", "Runtime"),
            ("Sdk", "Sdk"),
            ("Subject", "Subject"),
            ("Comment", "Comment"),
        ] {
            if let Some(value) = info.get(key) {
                extra.push(DetailField::new(label, value));
            }
        }

        Ok(Component {
            id: ComponentId::new(ADAPTER_ID, reference),
            name,
            version: info.get("Version").map(str::to_string),
            description: info.header_summary().map(str::to_string),
            category,
            source: ComponentSource::Flatpak { scope },
            status: ComponentStatus::Installed,
            // flatpak itself answered: this is not a derived fact.
            confidence: DetectionConfidence::Authoritative,
            capabilities: flatpak_capabilities(),
            detection: DetectionMethod::command(
                "flatpak",
                vec!["info".to_string(), native_id.to_string()],
            ),
            installed_size: info.get("Installed Size").and_then(parse_size),
            location: None,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra,
        })
    }
}

impl ComponentAdapter for FlatpakAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn display_name(&self) -> &'static str {
        "Flatpak"
    }

    fn description(&self) -> &'static str {
        "Flatpak applications and runtimes, per installation scope (optional integration)"
    }

    fn capabilities(&self) -> Vec<ComponentCapability> {
        vec![
            ComponentCapability::Inspect,
            ComponentCapability::Remove,
            ComponentCapability::Update,
        ]
    }

    fn status(&self) -> AdapterStatus {
        let Some(tool) = self.runner.resolve("flatpak") else {
            return AdapterStatus {
                id: ADAPTER_ID.to_string(),
                display_name: self.display_name().to_string(),
                description: self.description().to_string(),
                availability: AdapterAvailability::Unavailable {
                    reason: "flatpak was not found in the executable search path".to_string(),
                },
                tool: None,
                tool_version: None,
                capabilities: self.capabilities(),
                commands: COMMANDS
                    .iter()
                    .map(|command| (*command).to_string())
                    .collect(),
                notes: vec!["flatpak is optional: everything else works without it".to_string()],
            };
        };

        let tool_version = self
            .run(vec!["--version".to_string()], "flatpak version")
            .ok()
            .and_then(|output| {
                output
                    .stdout
                    .lines()
                    .next()
                    .map(|line| line.trim().to_string())
            })
            .filter(|line| !line.is_empty());

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
            notes: Vec::new(),
        }
    }

    fn list(&self) -> Result<Vec<Component>, AdapterError> {
        let mut components = self.list_kind("--app", ComponentCategory::Application)?;
        components.extend(self.list_kind("--runtime", ComponentCategory::Runtime)?);
        Ok(components)
    }

    fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError> {
        if let Err(reason) = validate_flatpak_ref(native_id) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: native_id.to_string(),
                reason: reason.to_string(),
            });
        }

        let output = self.run(
            vec!["info".to_string(), native_id.to_string()],
            "flatpak info",
        )?;
        if !output.success() {
            // A component that vanished must cancel any pending operation, not
            // be reported as an error or reconstructed from memory.
            if output.stderr.contains("not installed") {
                return Ok(None);
            }
            return Err(AdapterError::command(
                "flatpak info <ref>",
                format!("flatpak info {native_id}"),
                format!("exit {:?}: {}", output.exit_code, output.stderr.trim()),
            ));
        }

        let info = parse_info(&output.stdout);
        self.component_from_info(native_id, &info).map(Some)
    }

    /// Flatpak owns its own store, so it is the only adapter allowed to remove or
    /// update a flatpak component. It never touches the pacman database.
    fn supports(&self, component: &Component, kind: OperationKind) -> bool {
        matches!(kind, OperationKind::Remove | OperationKind::Update)
            && matches!(component.source, ComponentSource::Flatpak { .. })
    }

    fn plan_operation(
        &self,
        request: &OperationRequest,
        component: &Component,
    ) -> Result<OperationPlan, AdapterError> {
        let reference = &request.target.native_id;
        if let Err(reason) = validate_flatpak_ref(reference) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: reference.clone(),
                reason: reason.to_string(),
            });
        }
        let scope = match component.source {
            ComponentSource::Flatpak { scope } => scope,
            _ => {
                return Err(AdapterError::InvalidId {
                    adapter: ADAPTER_ID.to_string(),
                    value: reference.clone(),
                    reason: "not a flatpak component".to_string(),
                });
            }
        };
        let scope_flag = match scope {
            InstallScope::User => "--user",
            InstallScope::System => "--system",
        };

        // Re-read the object: the plan must describe the store as it is.
        let info = self.run(
            vec!["info".to_string(), reference.clone()],
            "flatpak info <ref>",
        )?;
        if !info.success() && !info.stdout.trim().is_empty() {
            return Err(AdapterError::command(
                "flatpak info <ref>",
                format!("flatpak info {reference}"),
                format!("exit {:?}: {}", info.exit_code, info.stderr.trim()),
            ));
        }
        if !info.success() {
            return Err(AdapterError::command(
                "flatpak info <ref>",
                format!("flatpak info {reference}"),
                "the reference is no longer installed".to_string(),
            ));
        }

        let mut affected = Vec::new();
        let mut unknown = Vec::new();
        let mut notes = Vec::new();
        let is_runtime = reference.starts_with("runtime/");

        match request.kind {
            OperationKind::Remove => {
                affected.push(format!(
                    "the {} record in the {} installation",
                    category_word(is_runtime),
                    scope.label()
                ));
                // The listed columns do not state which applications use a runtime,
                // so this stays an open question instead of a claim.
                unknown.push(if is_runtime {
                    "which applications use this runtime is not reported by the flatpak listing \
                     columns; flatpak may refuse the removal if a runtime is in use"
                        .to_string()
                } else {
                    "which other applications share this application's files is not reported by \
                     the flatpak listing columns"
                        .to_string()
                });
                if !is_runtime {
                    notes.push(
                        "per-application data under ~/.var/app is not removed by this operation"
                            .to_string(),
                    );
                }
                notes.push(
                    "related runtimes are not removed: --no-related keeps shared dependencies"
                        .to_string(),
                );
                notes.push("the operation is not reversible".to_string());
            }
            OperationKind::Reclaim => {
                return Err(AdapterError::unsupported(
                    ADAPTER_ID,
                    "reclaim (flatpak owns its store; every entry is removable through it)",
                ));
            }
            OperationKind::Update => {
                affected.push("the files of this reference only".to_string());
                unknown.push(
                    "whether a newer version exists is not checked; flatpak itself decides"
                        .to_string(),
                );
                notes.push(
                    "the update applies to this reference in this installation scope only"
                        .to_string(),
                );
            }
        }

        let args = match request.kind {
            OperationKind::Remove => vec![
                "uninstall".to_string(),
                scope_flag.to_string(),
                "--noninteractive".to_string(),
                "--no-related".to_string(),
                reference.clone(),
            ],
            OperationKind::Reclaim => Vec::new(),
            OperationKind::Update => vec![
                "update".to_string(),
                scope_flag.to_string(),
                "--noninteractive".to_string(),
                reference.clone(),
            ],
        };

        Ok(OperationPlan {
            adapter: ADAPTER_ID.to_string(),
            target: request.target.clone(),
            target_name: component.name.clone(),
            target_version: component.version.clone(),
            kind: request.kind,
            commands: vec![crate::domain::PlannedCommand {
                program: "flatpak".to_string(),
                args,
                requires_root: scope == InstallScope::System,
                requires_tty: false,
                rationale: format!(
                    "{} this reference in the {} installation; flatpak owns its store",
                    request.kind.label(),
                    scope.label()
                ),
            }],
            reclaim: Vec::new(),
            affected,
            blocking: Vec::new(),
            unknown,
            notes,
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
        if let Err(reason) = validate_flatpak_ref(&plan.target.native_id) {
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
            if command.program != "flatpak" {
                return Err(AdapterError::InvalidId {
                    adapter: ADAPTER_ID.to_string(),
                    value: command.program.clone(),
                    reason: "this adapter only runs flatpak".to_string(),
                });
            }
            command_display = command.display();
            let spec = if command.requires_tty {
                CommandSpec::interactive(command.program.clone(), command.args.clone())
            } else {
                CommandSpec::capture(command.program.clone(), command.args.clone())
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
            stdout_tail: crate::domain::tail_text(&stdout, 2000),
            stderr_tail: crate::domain::tail_text(&stderr, 2000),
            summary: if success {
                format!("{}d {}", plan.kind.label(), plan.target_name)
            } else {
                format!(
                    "flatpak did not complete the {} (exit {exit_code:?})",
                    plan.kind.label()
                )
            },
        })
    }
}

fn flatpak_capabilities() -> Vec<ComponentCapability> {
    vec![
        ComponentCapability::Inspect,
        ComponentCapability::Remove,
        ComponentCapability::Update,
    ]
}

fn category_word(is_runtime: bool) -> &'static str {
    if is_runtime { "runtime" } else { "application" }
}

/// flatpak refs start with the kind of component they name.
fn category_from_ref(reference: &str) -> Option<ComponentCategory> {
    match reference.split('/').next()? {
        "app" => Some(ComponentCategory::Application),
        "runtime" => Some(ComponentCategory::Runtime),
        _ => None,
    }
}

/// The application id inside a ref (`app/org.example.App/x86_64/stable`), used as
/// a name only when flatpak printed no `Name - summary` header.
fn application_from_ref(reference: &str) -> Option<String> {
    let mut segments = reference.split('/');
    match (segments.next(), segments.next()) {
        (Some("app" | "runtime"), Some(application)) if !application.is_empty() => {
            Some(application.to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::InstallScope;
    use crate::infrastructure::test_support::FakeRunner;

    const APPS: &str = include_str!("../../../tests/fixtures/flatpak/list-apps.txt");
    const RUNTIMES: &str = include_str!("../../../tests/fixtures/flatpak/list-runtimes.txt");
    const INFO: &str = include_str!("../../../tests/fixtures/flatpak/info.txt");
    const FIREFOX_REF: &str = "app/org.mozilla.firefox/x86_64/stable";

    fn adapter(runner: FakeRunner) -> FlatpakAdapter {
        FlatpakAdapter::new(Arc::new(runner))
    }

    fn complete_runner() -> FakeRunner {
        FakeRunner::new()
            .on("flatpak --version", "Flatpak 1.15.10\n")
            .on(LIST_APP, APPS)
            .on(LIST_RUNTIME, RUNTIMES)
    }

    #[test]
    fn reports_itself_unavailable_when_flatpak_is_missing() {
        // The case that actually happens on a host without flatpak (decision D6).
        let status = adapter(FakeRunner::new()).status();
        assert_eq!(
            status.availability,
            AdapterAvailability::Unavailable {
                reason: "flatpak was not found in the executable search path".to_string()
            }
        );
        assert!(
            status.tool.is_none(),
            "no tool can be reported for a missing tool"
        );
        assert!(status.tool_version.is_none(), "no version can be invented");
    }

    #[test]
    fn lists_applications_and_runtimes_with_their_own_scope() {
        let components = adapter(complete_runner()).list().expect("list");
        assert_eq!(components.len(), 4);

        let firefox = components
            .iter()
            .find(|c| c.name == "org.mozilla.firefox")
            .expect("firefox");
        assert_eq!(firefox.id.to_string(), format!("flatpak:{FIREFOX_REF}"));
        assert_eq!(firefox.category, ComponentCategory::Application);
        assert_eq!(
            firefox.source,
            ComponentSource::Flatpak {
                scope: InstallScope::System
            }
        );
        assert_eq!(firefox.source.label(), "flatpak / system");
        assert_eq!(firefox.version.as_deref(), Some("129.0.2"));
        assert_eq!(firefox.installed_size, Some(250_400_000));
        assert_eq!(firefox.status, ComponentStatus::Installed);
        assert_eq!(firefox.confidence, DetectionConfidence::Authoritative);
        assert_eq!(firefox.capabilities, flatpak_capabilities());
        assert_eq!(firefox.detection.label(), LIST_APP.to_string());
        assert!(
            firefox
                .extra
                .iter()
                .any(|field| field.label == "Origin" && field.value == "flathub")
        );

        let calculator = components
            .iter()
            .find(|c| c.name == "org.gnome.Calculator")
            .expect("calc");
        assert_eq!(
            calculator.source,
            ComponentSource::Flatpak {
                scope: InstallScope::User
            }
        );

        let platform = components
            .iter()
            .find(|c| c.name == "org.freedesktop.Platform")
            .expect("runtime");
        assert_eq!(platform.category, ComponentCategory::Runtime);
        assert_eq!(
            platform.source,
            ComponentSource::Flatpak {
                scope: InstallScope::System
            }
        );
        assert_eq!(platform.installed_size, Some(1_200_000_000));
        assert_eq!(platform.detection.label(), LIST_RUNTIME);
        assert_ne!(
            firefox.id, platform.id,
            "identity is the flatpak ref, scope included"
        );
    }

    #[test]
    fn skips_a_row_with_an_installation_it_cannot_name() {
        let unknown =
            "app/org.example.App/x86_64/stable\torg.example.App\t1.0\tflathub\tportable\t1.0 MB\n";
        let runner = FakeRunner::new().on(LIST_APP, unknown).on(LIST_RUNTIME, "");
        let components = adapter(runner)
            .list()
            .expect("a skipped row is not a failure");
        assert!(
            components.is_empty(),
            "an unknown installation must not be mislabelled: {components:?}"
        );
    }

    #[test]
    fn keeps_the_good_rows_when_one_row_is_malformed() {
        let mixed = format!("{APPS}app/org.example.Broken\tbroken\n");
        let runner = FakeRunner::new()
            .on(LIST_APP, &mixed)
            .on(LIST_RUNTIME, RUNTIMES);
        let components = adapter(runner)
            .list()
            .expect("a malformed row is not a failure");
        assert_eq!(
            components.len(),
            4,
            "the two apps and two runtimes stay: {components:?}"
        );
        assert!(!components.iter().any(|c| c.name.contains("Broken")));
    }

    #[test]
    fn reports_the_version_when_flatpak_is_present() {
        let status = adapter(complete_runner()).status();
        assert!(status.available());
        assert_eq!(status.tool_version.as_deref(), Some("Flatpak 1.15.10"));
        assert!(
            status
                .commands
                .iter()
                .any(|command| command == LIST_RUNTIME)
        );
    }

    #[test]
    fn an_empty_list_is_an_empty_inventory_not_an_error() {
        let runner = FakeRunner::new().on(LIST_APP, "").on(LIST_RUNTIME, "");
        assert!(adapter(runner).list().expect("empty is valid").is_empty());
    }

    #[test]
    fn a_failing_list_is_an_error_not_an_empty_inventory() {
        let runner = FakeRunner::new().on_exit(LIST_APP, 1, "", "error: no installations found\n");
        let error = adapter(runner).list().expect_err("must fail");
        assert!(
            matches!(error, AdapterError::Command { .. }),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn details_reads_the_info_block_of_one_ref() {
        let runner = complete_runner().on(&format!("flatpak info {FIREFOX_REF}"), INFO);
        let component = adapter(runner)
            .details(FIREFOX_REF)
            .expect("details")
            .expect("present");

        assert_eq!(component.id.to_string(), format!("flatpak:{FIREFOX_REF}"));
        assert_eq!(component.name, "Firefox");
        assert_eq!(
            component.description.as_deref(),
            Some("Fast, private and secure web browser")
        );
        assert_eq!(component.version.as_deref(), Some("129.0.2"));
        assert_eq!(component.category, ComponentCategory::Application);
        assert_eq!(
            component.source,
            ComponentSource::Flatpak {
                scope: InstallScope::System
            }
        );
        assert_eq!(component.installed_size, Some(250_400_000));
        assert_eq!(component.confidence, DetectionConfidence::Authoritative);
        assert_eq!(
            component.detection.label(),
            format!("flatpak info {FIREFOX_REF}")
        );
        assert!(component.extra.iter().any(|field| field.label == "Runtime"));
    }

    #[test]
    fn details_reports_a_ref_that_is_no_longer_installed() {
        let runner = complete_runner().on_exit(
            &format!("flatpak info {FIREFOX_REF}"),
            1,
            "",
            &format!("error: {FIREFOX_REF} not installed\n"),
        );
        assert!(
            adapter(runner)
                .details(FIREFOX_REF)
                .expect("details")
                .is_none()
        );
    }

    #[test]
    fn details_refuses_a_traversal_shaped_ref_before_running_anything() {
        let error = adapter(complete_runner())
            .details("../../etc/passwd")
            .expect_err("must refuse");
        match error {
            AdapterError::InvalidId { value, .. } => assert_eq!(value, "../../etc/passwd"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn never_guesses_the_scope_when_the_info_block_omits_it() {
        let runner = complete_runner().on(
            &format!("flatpak info {FIREFOX_REF}"),
            "Firefox - Web browser\n\nRef: app/org.mozilla.firefox/x86_64/stable\nVersion: 129.0.2\n",
        );
        let error = adapter(runner)
            .details(FIREFOX_REF)
            .expect_err("must not invent a scope");
        assert!(
            matches!(error, AdapterError::Parse { .. }),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn removal_and_update_are_claimed_only_for_flatpak_components() {
        let adapter = FlatpakAdapter::new(Arc::new(complete_runner()));
        let component = ad_hoc_component("app/org.example.App/x86_64/stable", InstallScope::User);

        assert_eq!(
            adapter.capabilities(),
            vec![
                ComponentCapability::Inspect,
                ComponentCapability::Remove,
                ComponentCapability::Update
            ]
        );
        assert!(adapter.supports(&component, OperationKind::Remove));
        assert!(adapter.supports(&component, OperationKind::Update));
    }

    #[test]
    fn a_user_removal_is_scope_explicit_and_never_removes_related_refs() {
        let runner = complete_runner().on(
            "flatpak info app/org.example.App/x86_64/stable",
            "Name: Example\nVersion: 1.0\n",
        );
        let adapter = FlatpakAdapter::new(Arc::new(runner));
        let component = ad_hoc_component("app/org.example.App/x86_64/stable", InstallScope::User);
        let request = OperationRequest::new(
            crate::domain::ComponentId::new("flatpak", "app/org.example.App/x86_64/stable"),
            OperationKind::Remove,
        );
        let plan = adapter.plan_operation(&request, &component).expect("plan");

        assert_eq!(plan.commands.len(), 1);
        assert_eq!(
            plan.commands[0].display(),
            "flatpak uninstall --user --noninteractive --no-related app/org.example.App/x86_64/stable"
        );
        assert!(
            !plan.commands[0].requires_root,
            "a user installation needs no privileges"
        );
        assert!(plan.is_executable());
        assert!(plan.notes.iter().any(|note| note.contains("~/.var/app")));
    }

    #[test]
    fn a_system_removal_asks_for_privileges() {
        let runner = complete_runner().on(
            "flatpak info app/org.example.App/x86_64/stable",
            "Name: Example\nVersion: 1.0\n",
        );
        let adapter = FlatpakAdapter::new(Arc::new(runner));
        let component = ad_hoc_component("app/org.example.App/x86_64/stable", InstallScope::System);
        let request = OperationRequest::new(
            crate::domain::ComponentId::new("flatpak", "app/org.example.App/x86_64/stable"),
            OperationKind::Remove,
        );
        let plan = adapter.plan_operation(&request, &component).expect("plan");
        assert!(plan.commands[0].requires_root);
        assert!(plan.commands[0].args.iter().any(|arg| arg == "--system"));
    }

    #[test]
    fn a_runtime_removal_states_that_dependents_are_not_reported() {
        let runner = complete_runner().on(
            "flatpak info runtime/org.gnome.Platform/x86_64/48",
            "Name: GNOME Platform\n",
        );
        let adapter = FlatpakAdapter::new(Arc::new(runner));
        let component =
            ad_hoc_component("runtime/org.gnome.Platform/x86_64/48", InstallScope::System);
        let request = OperationRequest::new(
            crate::domain::ComponentId::new("flatpak", "runtime/org.gnome.Platform/x86_64/48"),
            OperationKind::Remove,
        );
        let plan = adapter.plan_operation(&request, &component).expect("plan");

        assert!(
            plan.unknown
                .iter()
                .any(|fact| fact.contains("which applications use this runtime")),
            "the dependency question must stay open: {:?}",
            plan.unknown
        );
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("not reversible"))
        );
    }

    #[test]
    fn a_traversal_shaped_reference_is_refused_before_any_process_runs() {
        let runner = complete_runner();
        let adapter = FlatpakAdapter::new(Arc::new(runner));
        let component = ad_hoc_component("../../etc/passwd", InstallScope::User);
        let request = OperationRequest::new(
            crate::domain::ComponentId::new("flatpak", "../../etc/passwd"),
            OperationKind::Remove,
        );
        let error = adapter
            .plan_operation(&request, &component)
            .expect_err("must refuse");
        assert!(
            matches!(error, AdapterError::InvalidId { .. }),
            "unexpected: {error}"
        );
    }

    #[test]
    fn a_failing_uninstall_is_never_reported_as_success() {
        let runner = FakeRunner::new().on_exit(
            "flatpak uninstall --user --noninteractive --no-related app/org.example.App/x86_64/stable",
            1,
            "",
            "error: app is in use\n",
        );
        let adapter = FlatpakAdapter::new(Arc::new(runner));
        let plan = OperationPlan {
            adapter: "flatpak".to_string(),
            target: crate::domain::ComponentId::new("flatpak", "app/org.example.App/x86_64/stable"),
            target_name: "Example".to_string(),
            target_version: None,
            kind: OperationKind::Remove,
            commands: vec![crate::domain::PlannedCommand {
                program: "flatpak".to_string(),
                args: vec![
                    "uninstall".to_string(),
                    "--user".to_string(),
                    "--noninteractive".to_string(),
                    "--no-related".to_string(),
                    "app/org.example.App/x86_64/stable".to_string(),
                ],
                requires_root: false,
                requires_tty: false,
                rationale: "remove".to_string(),
            }],
            reclaim: Vec::new(),
            affected: Vec::new(),
            blocking: Vec::new(),
            unknown: Vec::new(),
            notes: Vec::new(),
        };
        let outcome = adapter.execute(&plan).expect("execute");
        assert!(!outcome.success);
        assert_eq!(outcome.exit_code, Some(1));
        assert!(outcome.stderr_tail.contains("in use"));
    }

    fn ad_hoc_component(reference: &str, scope: InstallScope) -> Component {
        Component {
            id: crate::domain::ComponentId::new("flatpak", reference),
            name: "Example".to_string(),
            version: Some("1.0".to_string()),
            description: None,
            category: ComponentCategory::Application,
            source: ComponentSource::Flatpak { scope },
            status: crate::domain::ComponentStatus::Installed,
            confidence: crate::domain::DetectionConfidence::Authoritative,
            capabilities: flatpak_capabilities(),
            detection: crate::domain::DetectionMethod::command("flatpak", vec!["list".to_string()]),
            installed_size: None,
            location: None,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: Vec::new(),
        }
    }
    #[test]
    fn reads_the_scope_and_size_helpers_it_depends_on() {
        assert_eq!(parse_scope("user"), Some(InstallScope::User));
        assert_eq!(parse_scope("system"), Some(InstallScope::System));
        assert_eq!(parse_scope("portable"), None);
        assert_eq!(parse_size("Unknown"), None);
        assert!(matches!(parse_list(""), _ if true));
    }
}
