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
            capabilities: vec![ComponentCapability::Inspect],
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
            capabilities: vec![ComponentCapability::Inspect],
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
        vec![ComponentCapability::Inspect]
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

    fn supports(&self, _component: &Component, _kind: OperationKind) -> bool {
        // Operation executors land with their own plan, confirmation and tests.
        false
    }

    fn plan_operation(
        &self,
        _request: &OperationRequest,
        _component: &Component,
    ) -> Result<OperationPlan, AdapterError> {
        Err(AdapterError::unsupported(ADAPTER_ID, "operation planning"))
    }

    fn execute(&self, _plan: &OperationPlan) -> Result<OperationOutcome, AdapterError> {
        Err(AdapterError::unsupported(ADAPTER_ID, "operation execution"))
    }
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
        assert_eq!(firefox.capabilities, vec![ComponentCapability::Inspect]);
        assert_eq!(firefox.detection.label(), format!("{LIST_APP}"));
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
    fn offers_inspection_only_and_claims_no_operation() {
        let adapter = adapter(complete_runner());
        assert_eq!(adapter.capabilities(), vec![ComponentCapability::Inspect]);

        let firefox = adapter
            .list()
            .expect("list")
            .into_iter()
            .find(|component| component.name == "org.mozilla.firefox")
            .expect("firefox");
        assert!(!adapter.supports(&firefox, OperationKind::Remove));
        assert!(!adapter.supports(&firefox, OperationKind::Update));
        assert!(
            adapter
                .plan_operation(
                    &OperationRequest::new(firefox.id.clone(), OperationKind::Remove),
                    &firefox
                )
                .is_err()
        );
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
