//! AUR adapter: helper detection, remote metadata and operation routing for the
//! foreign pacman packages (decision D1).
//!
//! Commands used, and why:
//! * `yay --version` / `paru --version` — helper identity shown in the adapters
//!   view.
//! * `yay -Si <package>` / `paru -Si <package>` — the AUR record of one package:
//!   votes, popularity, maintainer, last modification. Run lazily, only when a
//!   detail view asks for it.
//!
//! This adapter never installs, updates or removes anything yet: its only
//! capability is inspection.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::adapters::pacman::parse::parse_qi;
use crate::domain::{AdapterAvailability, Component};
use crate::domain::{
    AdapterError, AdapterStatus, ComponentAdapter, ComponentCapability, ComponentSource,
    DetailField, OperationKind, OperationOutcome, OperationPlan, OperationRequest, PacmanOrigin,
};
use crate::infrastructure::process::{CommandOutput, CommandRunner, CommandSpec, ProcessError};
use crate::infrastructure::validate::validate_pacman_name;

const ADAPTER_ID: &str = "aur";

/// Preference order. The first helper that resolves in `PATH` wins; a second
/// helper is never used as an implicit fallback, because the user chose neither.
const HELPERS: &[&str] = &["yay", "paru"];

const METADATA_TIMEOUT: Duration = Duration::from_secs(15);

/// Labels of the remote facts shown in the detail view, and the AUR record field
/// each one is read from. Only fields the record actually has are shown. `URL` is
/// the field `-Si` always prints for the package itself; `AUR URL` is the label
/// the user sees, not a second lookup.
const METADATA_FIELDS: &[(&str, &str)] = &[
    ("AUR version", "Version"),
    ("AUR maintainer", "Maintainer"),
    ("AUR votes", "Votes"),
    ("AUR popularity", "Popularity"),
    ("AUR last modified", "Last Modified"),
    ("AUR URL", "URL"),
];

const COMMANDS: &[&str] = &[
    "yay --version",
    "paru --version",
    "yay -Si <package>",
    "paru -Si <package>",
];

/// A helper that actually resolved in the executable search path.
struct Helper {
    program: &'static str,
    path: PathBuf,
}

pub struct AurAdapter {
    runner: Arc<dyn CommandRunner>,
}

impl AurAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }

    /// The helper this adapter speaks to: `yay` first, then `paru`.
    fn helper(&self) -> Option<Helper> {
        HELPERS.iter().find_map(|program| {
            self.runner
                .resolve(program)
                .map(|path| Helper { program, path })
        })
    }

    /// What the adapters view must say about this run: which helper speaks for the
    /// AUR, which one is missing, and why this adapter lists nothing of its own.
    fn notes(&self, helper: &Helper) -> Vec<String> {
        let mut notes = vec![format!("using {} as the AUR helper", helper.program)];
        for other in HELPERS.iter().filter(|program| **program != helper.program) {
            if self.runner.resolve(other).is_none() {
                notes.push(format!("{other} is not installed"));
            }
        }
        notes.push(
            "AUR does not own components: foreign pacman packages are enriched in place, never listed twice"
                .to_string(),
        );
        notes
    }

    /// Reads the AUR record of one foreign package. Every failure is `None`: an
    /// enrichment is a bonus line, never a reason for the details view to fail.
    fn enrich_fields(&self, native_id: &str) -> Option<Vec<DetailField>> {
        // The native id becomes a process argument; validate before running.
        validate_pacman_name(native_id).ok()?;
        let helper = self.helper()?;
        let output = self
            .run(&helper, &["-Si", native_id], "AUR package information")
            .ok()?;
        if !output.success() {
            return None;
        }

        let record = parse_qi(&output.stdout).into_iter().next()?;
        let fields: Vec<DetailField> = METADATA_FIELDS
            .iter()
            .filter_map(|(label, key)| record.get(key).map(|value| DetailField::new(*label, value)))
            .collect();
        (!fields.is_empty()).then_some(fields)
    }

    fn run(
        &self,
        helper: &Helper,
        args: &[&str],
        context: &str,
    ) -> Result<CommandOutput, AdapterError> {
        let spec = CommandSpec::capture(
            helper.program,
            args.iter().map(|arg| (*arg).to_string()).collect(),
        )
        .with_timeout(METADATA_TIMEOUT);

        self.runner.run(&spec).map_err(|error| match error {
            ProcessError::NotFound { .. } => AdapterError::ToolUnavailable {
                tool: helper.program.to_string(),
                reason: "not found in the executable search path".to_string(),
            },
            other => AdapterError::command(context, spec.display(), other.to_string()),
        })
    }
}

/// `yay --version` prints one line; the version is taken from the line that
/// carries it rather than from the whole output.
fn version_line(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .map(str::trim)
        .find(|line| line.contains(" v"))
        .map(|line| line.to_string())
}

impl ComponentAdapter for AurAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn display_name(&self) -> &'static str {
        "AUR helper"
    }

    fn description(&self) -> &'static str {
        "Metadata and operation routing for foreign packages, through yay or paru"
    }

    fn capabilities(&self) -> Vec<ComponentCapability> {
        vec![ComponentCapability::Inspect]
    }

    fn status(&self) -> AdapterStatus {
        let Some(helper) = self.helper() else {
            return AdapterStatus {
                id: ADAPTER_ID.to_string(),
                display_name: self.display_name().to_string(),
                description: self.description().to_string(),
                availability: AdapterAvailability::Unavailable {
                    reason: "neither yay nor paru was found in the executable search path"
                        .to_string(),
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
            .run(&helper, &["--version"], "helper version")
            .ok()
            .and_then(|output| version_line(&output.stdout));

        AdapterStatus {
            id: ADAPTER_ID.to_string(),
            display_name: self.display_name().to_string(),
            description: self.description().to_string(),
            availability: AdapterAvailability::Available,
            tool: Some(helper.path.clone()),
            tool_version,
            capabilities: self.capabilities(),
            commands: COMMANDS
                .iter()
                .map(|command| (*command).to_string())
                .collect(),
            notes: self.notes(&helper),
        }
    }

    /// Never lists components (decision D1). AUR packages are foreign
    /// *pacman-owned* components: `pacman` lists them, `pacman` owns their
    /// identity, and this adapter only enriches and routes operations for them.
    /// Listing them here would show every AUR package twice under one name.
    fn list(&self) -> Result<Vec<Component>, AdapterError> {
        Ok(Vec::new())
    }

    fn details(&self, _native_id: &str) -> Result<Option<Component>, AdapterError> {
        Err(AdapterError::unsupported(
            ADAPTER_ID,
            "details (AUR does not own components; the pacman adapter owns them)",
        ))
    }

    /// Remote AUR facts for a foreign component, or nothing at all.
    ///
    /// These facts come from the AUR web metadata that the helper prints, not
    /// from the installed package database, so they are not `Authoritative`
    /// detection of what is on this machine: they only enrich a component whose
    /// ownership pacman already established.
    fn enrich(&self, component: &Component) -> Option<Vec<DetailField>> {
        if !is_foreign(component) {
            return None;
        }
        self.enrich_fields(&component.id.native_id)
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

/// Whether `component` is a foreign pacman package, the only kind of component
/// the AUR can say anything about.
fn is_foreign(component: &Component) -> bool {
    matches!(
        component.source,
        ComponentSource::Pacman {
            origin: PacmanOrigin::Foreign
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        Component, ComponentCategory, ComponentId, ComponentStatus, DetectionConfidence,
        DetectionMethod, InstallScope,
    };
    use crate::infrastructure::test_support::FakeRunner;

    const SI: &str = include_str!("../../../tests/fixtures/aur/si.txt");

    fn adapter(runner: FakeRunner) -> AurAdapter {
        AurAdapter::new(Arc::new(runner))
    }

    fn helper_runner() -> FakeRunner {
        FakeRunner::new().on("yay --version", "yay v13.0.1 - libalpm v16.0.1\n")
    }

    fn component_with(source: ComponentSource, native_id: &str) -> Component {
        Component {
            id: ComponentId::new("pacman", native_id),
            name: native_id.to_string(),
            version: Some("1.99.3-1".to_string()),
            description: None,
            category: ComponentCategory::Package,
            source,
            status: ComponentStatus::Installed,
            confidence: DetectionConfidence::Authoritative,
            capabilities: vec![ComponentCapability::Inspect],
            detection: DetectionMethod::command("pacman", vec!["-Q".to_string()]),
            installed_size: None,
            location: None,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: Vec::new(),
        }
    }

    fn foreign(native_id: &str) -> Component {
        component_with(
            ComponentSource::Pacman {
                origin: PacmanOrigin::Foreign,
            },
            native_id,
        )
    }

    #[test]
    fn reports_itself_unavailable_when_neither_helper_is_installed() {
        let status = adapter(FakeRunner::new()).status();
        assert_eq!(
            status.availability,
            AdapterAvailability::Unavailable {
                reason: "neither yay nor paru was found in the executable search path".to_string()
            }
        );
        assert!(status.tool.is_none());
        assert!(status.tool_version.is_none());
    }

    #[test]
    fn prefers_yay_over_paru_and_reports_its_version() {
        let runner = helper_runner().with_program("paru");
        let status = adapter(runner).status();
        assert!(status.available());
        assert_eq!(status.tool, Some(PathBuf::from("/fake/bin/yay")));
        assert_eq!(
            status.tool_version.as_deref(),
            Some("yay v13.0.1 - libalpm v16.0.1")
        );
        assert!(
            status.notes.iter().any(|note| note.contains("yay")),
            "notes must say which helper was chosen: {:?}",
            status.notes
        );
        assert!(
            !status
                .notes
                .iter()
                .any(|note| note.contains("paru is not installed")),
            "paru is installed here: {:?}",
            status.notes
        );
    }

    #[test]
    fn falls_back_to_paru_and_says_yay_is_missing() {
        let runner = FakeRunner::new().on("paru --version", "paru v2.0.4 - libalpm v16.0.1\n");
        let status = adapter(runner).status();
        assert!(status.available());
        assert_eq!(status.tool, Some(PathBuf::from("/fake/bin/paru")));
        assert_eq!(
            status.tool_version.as_deref(),
            Some("paru v2.0.4 - libalpm v16.0.1")
        );
        assert!(status.notes.iter().any(|note| note.contains("paru")));
        assert!(
            status
                .notes
                .iter()
                .any(|note| note.contains("yay is not installed")),
            "the missing helper must be named: {:?}",
            status.notes
        );
    }

    #[test]
    fn never_lists_a_component_of_its_own() {
        let components = adapter(helper_runner())
            .list()
            .expect("AUR never fails the inventory");
        assert!(
            components.is_empty(),
            "AUR must not duplicate pacman-owned foreign packages"
        );
    }

    #[test]
    fn offers_inspection_only_and_claims_no_operation() {
        let adapter = adapter(helper_runner());
        assert_eq!(adapter.capabilities(), vec![ComponentCapability::Inspect]);

        let package = foreign("visual-studio-code-bin");
        assert!(!adapter.supports(&package, OperationKind::Remove));
        assert!(!adapter.supports(&package, OperationKind::Update));
    }

    #[test]
    fn details_is_refused_explicitly() {
        let error = adapter(helper_runner())
            .details("visual-studio-code-bin")
            .expect_err("must refuse");
        assert_eq!(
            error,
            AdapterError::unsupported(
                "aur",
                "details (AUR does not own components; the pacman adapter owns them)"
            )
        );
    }

    #[test]
    fn enriches_a_foreign_component_with_the_aur_record() {
        let runner = helper_runner().on("yay -Si visual-studio-code-bin", SI);
        let fields = adapter(runner)
            .enrich(&foreign("visual-studio-code-bin"))
            .expect("AUR fields");

        let value = |label: &str| {
            fields
                .iter()
                .find(|field| field.label == label)
                .unwrap_or_else(|| panic!("no {label} field in {fields:?}"))
                .value
                .clone()
        };
        assert_eq!(value("AUR version"), "1.99.3-1");
        assert_eq!(
            value("AUR maintainer"),
            "Some Maintainer <maintainer@example.invalid>"
        );
        assert_eq!(value("AUR votes"), "2731");
        assert_eq!(value("AUR popularity"), "0.913421");
        assert_eq!(
            value("AUR last modified"),
            "Thu 10 Apr 2025 09:41:00 AM -03"
        );
        assert_eq!(value("AUR URL"), "https://code.visualstudio.com/");
    }

    #[test]
    fn nothing_to_enrich_for_a_component_the_aur_does_not_own() {
        let runner = helper_runner().on("yay -Si btop", SI);
        let adapter = adapter(runner);

        let official = component_with(
            ComponentSource::Pacman {
                origin: PacmanOrigin::Official { repo: None },
            },
            "btop",
        );
        assert!(adapter.enrich(&official).is_none());

        let flatpak = component_with(
            ComponentSource::Flatpak {
                scope: InstallScope::User,
            },
            "app/org.mozilla.firefox/x86_64/stable",
        );
        assert!(adapter.enrich(&flatpak).is_none());
    }

    #[test]
    fn a_failing_helper_never_fails_the_details_view() {
        let runner = helper_runner().on_exit("yay -Si ghost", 1, "", "error: package not found\n");
        assert!(adapter(runner).enrich(&foreign("ghost")).is_none());

        let timed_out = helper_runner().on_error(
            "yay -Si ghost",
            ProcessError::Timeout {
                program: "yay".to_string(),
                seconds: 15,
            },
        );
        assert!(adapter(timed_out).enrich(&foreign("ghost")).is_none());
    }

    #[test]
    fn there_is_nothing_to_enrich_without_a_helper() {
        assert!(
            adapter(FakeRunner::new())
                .enrich(&foreign("visual-studio-code-bin"))
                .is_none()
        );
    }

    #[test]
    fn refuses_a_hostile_identifier_before_running_the_helper() {
        let hostile = "libfoo; rm -rf /";
        // The response exists: only refusing the identifier can keep it unused.
        let runner = helper_runner().on(&format!("yay -Si {hostile}"), SI);
        assert!(adapter(runner).enrich(&foreign(hostile)).is_none());
    }

    #[test]
    fn reads_the_version_line_of_a_helper() {
        assert_eq!(
            version_line("yay v13.0.1 - libalpm v16.0.1\n"),
            Some("yay v13.0.1 - libalpm v16.0.1".to_string())
        );
        assert_eq!(version_line(""), None);
    }

    #[test]
    fn a_foreign_component_is_the_shape_it_understands() {
        assert!(is_foreign(&foreign("yay")));
        assert!(!is_foreign(&component_with(
            ComponentSource::Pacman {
                origin: PacmanOrigin::Official {
                    repo: Some("extra".to_string())
                }
            },
            "btop"
        )));
    }
}
