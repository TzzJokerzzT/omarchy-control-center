//! Pacman adapter: the system package inventory.
//!
//! Commands used, and why:
//! * `pacman --version` — tool identity shown in the adapters view.
//! * `pacman -Q` — one line per installed package; the authoritative list.
//! * `pacman -Qi` — all fields (description, size, dependencies, dependents,
//!   install date, install reason) in a single invocation; calling it per package
//!   would mean thousands of processes.
//! * `pacman -Qm` — packages not owned by a synced repository ("foreign", where
//!   AUR packages live).
//! * `pacman -Qdtq` — orphaned dependencies, used only as a status label.
//! * `pacman -Si <package>` — the repository of an official package, read lazily
//!   when a detail view needs it because `pacman -Qi` does not report it.
//!
//! This adapter never removes or updates anything yet: its only capability is
//! inspection.

pub mod parse;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use crate::domain::{
    AdapterError, AdapterStatus, Component, ComponentCapability, ComponentCategory, ComponentId,
    ComponentSource, ComponentStatus, DetailField, DetectionConfidence, DetectionMethod,
    OperationKind, OperationOutcome, OperationPlan, OperationRequest, PacmanOrigin,
    ComponentAdapter,
};
use crate::infrastructure::process::{CommandOutput, CommandRunner, CommandSpec, ProcessError};
use crate::infrastructure::validate::validate_pacman_name;

use self::parse::{QiRecord, parse_name_list, parse_q, parse_qi, parse_repository, parse_size};

const ADAPTER_ID: &str = "pacman";
const BULK_TIMEOUT: Duration = Duration::from_secs(60);

const COMMANDS: &[&str] = &[
    "pacman --version",
    "pacman -Q",
    "pacman -Qi",
    "pacman -Qm",
    "pacman -Qdtq",
    "pacman -Si <package>",
];

pub struct PacmanAdapter {
    runner: Arc<dyn CommandRunner>,
}

impl PacmanAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }

    fn run(&self, args: &[&str], context: &str) -> Result<CommandOutput, AdapterError> {
        let spec = CommandSpec::capture(
            "pacman",
            args.iter().map(|arg| (*arg).to_string()).collect(),
        )
        .with_timeout(BULK_TIMEOUT);

        self.runner.run(&spec).map_err(|error| match error {
            ProcessError::NotFound { .. } => AdapterError::ToolUnavailable {
                tool: "pacman".to_string(),
                reason: "not found in the executable search path".to_string(),
            },
            other => AdapterError::command(context, spec.display(), other.to_string()),
        })
    }

    /// Reads a name list (`-Qm`, `-Qdtq`). pacman exits non-zero when nothing
    /// matches, so an empty result is not an error; only a real message is.
    fn name_set(&self, flag: &str, context: &str) -> Result<BTreeSet<String>, AdapterError> {
        let output = self.run(&[flag], context)?;
        if output.success() || !output.stdout.trim().is_empty() {
            return Ok(parse_name_list(&output.stdout));
        }
        if !output.stderr.trim().is_empty() {
            return Err(AdapterError::command(
                context,
                format!("pacman {flag}"),
                format!("exit {:?}: {}", output.exit_code, output.stderr.trim()),
            ));
        }
        Ok(BTreeSet::new())
    }

    fn component_from(
        &self,
        record: &QiRecord,
        foreign: &BTreeSet<String>,
        orphans: &BTreeSet<String>,
        fallback_version: Option<&str>,
    ) -> Result<Component, AdapterError> {
        let name = record
            .get("Name")
            .ok_or_else(|| AdapterError::Parse {
                context: "pacman -Qi record".to_string(),
                detail: "record without a Name field".to_string(),
            })?
            .to_string();

        let version = record
            .get("Version")
            .map(|value| value.to_string())
            .or_else(|| fallback_version.map(|value| value.to_string()));

        let origin = if foreign.contains(&name) {
            PacmanOrigin::Foreign
        } else {
            PacmanOrigin::Official { repo: None }
        };
        let is_orphan = orphans.contains(&name);
        let capabilities = capabilities_for(&origin);

        let mut extra = Vec::new();
        for (label, key) in [
            ("Architecture", "Architecture"),
            ("Licenses", "Licenses"),
            ("Packager", "Packager"),
            ("Provides", "Provides"),
            ("Optional For", "Optional For"),
            ("Install Reason", "Install Reason"),
            ("Repository URL", "URL"),
        ] {
            if let Some(value) = record.get(key) {
                extra.push(DetailField::new(label, value));
            }
        }

        Ok(Component {
            id: ComponentId::new(ADAPTER_ID, name.clone()),
            name,
            version,
            description: record.get("Description").map(|value| value.to_string()),
            category: ComponentCategory::Package,
            source: ComponentSource::Pacman { origin },
            status: if is_orphan { ComponentStatus::Orphan } else { ComponentStatus::Installed },
            confidence: DetectionConfidence::Authoritative,
            capabilities,
            detection: DetectionMethod::command("pacman", vec!["-Q".to_string()])
                .with_note("listed by pacman -Q, fields from pacman -Qi"),
            installed_size: record.get("Installed Size").and_then(parse_size),
            location: None,
            installed_at: record.get("Install Date").map(|value| value.to_string()),
            depends_on: record.list("Depends On"),
            required_by: record.list("Required By"),
            extra,
        })
    }
}

/// `pacman --version` prints an ASCII logo before the real version line, so the
/// version is taken from the line that actually carries it.
fn version_line(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.find("Pacman v").map(|index| line[index..].trim().to_string()))
}

impl ComponentAdapter for PacmanAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn display_name(&self) -> &'static str {
        "Pacman"
    }

    fn description(&self) -> &'static str {
        "System packages managed by pacman: official repositories and foreign packages"
    }

    fn capabilities(&self) -> Vec<ComponentCapability> {
        vec![ComponentCapability::Inspect, ComponentCapability::Remove]
    }

    fn status(&self) -> AdapterStatus {
        let Some(tool) = self.runner.resolve("pacman") else {
            return AdapterStatus {
                id: ADAPTER_ID.to_string(),
                display_name: self.display_name().to_string(),
                description: self.description().to_string(),
                availability: crate::domain::AdapterAvailability::Unavailable {
                    reason: "pacman was not found in the executable search path".to_string(),
                },
                tool: None,
                tool_version: None,
                capabilities: self.capabilities(),
                commands: COMMANDS.iter().map(|command| (*command).to_string()).collect(),
                notes: Vec::new(),
            };
        };

        let tool_version = self
            .run(&["--version"], "pacman version")
            .ok()
            .and_then(|output| version_line(&output.stdout));

        AdapterStatus {
            id: ADAPTER_ID.to_string(),
            display_name: self.display_name().to_string(),
            description: self.description().to_string(),
            availability: crate::domain::AdapterAvailability::Available,
            tool: Some(tool),
            tool_version,
            capabilities: self.capabilities(),
            commands: COMMANDS.iter().map(|command| (*command).to_string()).collect(),
            notes: Vec::new(),
        }
    }

    fn list(&self) -> Result<Vec<Component>, AdapterError> {
        let installed = self.run(&["-Q"], "pacman -Q")?;
        if !installed.success() {
            return Err(AdapterError::command(
                "pacman -Q",
                "pacman -Q",
                format!("exit {:?}: {}", installed.exit_code, installed.stderr.trim()),
            ));
        }
        let packages = parse_q(&installed.stdout);
        let foreign = self.name_set("-Qm", "pacman -Qm")?;
        let orphans = self.name_set("-Qdtq", "pacman -Qdtq")?;

        // A single `-Qi` gives every field for every package. If it fails we keep
        // the authoritative name/version list and say so, instead of failing.
        let records = match self.run(&["-Qi"], "pacman -Qi") {
            Ok(output) if output.success() => parse_qi(&output.stdout),
            Ok(output) => {
                tracing::warn!(
                    exit = ?output.exit_code,
                    stderr = output.stderr.trim(),
                    "pacman -Qi failed; falling back to pacman -Q fields only"
                );
                Vec::new()
            }
            Err(error) => {
                tracing::warn!(%error, "pacman -Qi failed; falling back to pacman -Q fields only");
                Vec::new()
            }
        };

        let mut components = Vec::with_capacity(packages.len());
        let mut detail_by_name: std::collections::BTreeMap<&str, &QiRecord> =
            std::collections::BTreeMap::new();
        for record in &records {
            if let Some(name) = record.get("Name") {
                detail_by_name.insert(name, record);
            }
        }

        for (name, version) in &packages {
            match detail_by_name.get(name.as_str()) {
                Some(record) => {
                    components.push(self.component_from(record, &foreign, &orphans, Some(version))?)
                }
                None => components.push(self.minimal_component(name, version, &foreign, &orphans)),
            }
        }

        Ok(components)
    }

    fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError> {
        if let Err(reason) = validate_pacman_name(native_id) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: native_id.to_string(),
                reason: reason.to_string(),
            });
        }

        let output = self.run(&["-Qi", native_id], "pacman -Qi <package>")?;
        if !output.success() {
            if output.stderr.contains("was not found") {
                return Ok(None);
            }
            return Err(AdapterError::command(
                "pacman -Qi <package>",
                format!("pacman -Qi {native_id}"),
                format!("exit {:?}: {}", output.exit_code, output.stderr.trim()),
            ));
        }

        let records = parse_qi(&output.stdout);
        let Some(record) = records.first() else { return Ok(None) };

        let foreign = self.name_set("-Qm", "pacman -Qm")?;
        let orphans = self.name_set("-Qdtq", "pacman -Qdtq")?;
        let mut component = self.component_from(record, &foreign, &orphans, None)?;

        // The repository is not part of `-Qi`; ask for it once, and leave it as
        // unavailable if the answer is not trustworthy.
        if matches!(component.source, ComponentSource::Pacman { origin: PacmanOrigin::Official { repo: None } })
            && let Ok(si) = self.run(&["-Si", native_id], "pacman -Si <package>")
            && si.success()
            && let Some(repo) = parse_repository(&si.stdout)
        {
            component.source = ComponentSource::Pacman { origin: PacmanOrigin::Official { repo: Some(repo) } };
        }

        Ok(Some(component))
    }

    /// Pacman owns removal for packages that come from a synced repository.
    /// Foreign packages are removed through their AUR helper, which is a different
    /// database owner; updates are not offered here because Arch does not support
    /// partial upgrades (see `docs/adapters.md`).
    fn supports(&self, component: &Component, kind: OperationKind) -> bool {
        kind == OperationKind::Remove
            && matches!(
                component.source,
                ComponentSource::Pacman { origin: PacmanOrigin::Official { .. } }
            )
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

        // Re-read the package now: the plan must describe the system as it is.
        let output = self.run(&["-Qi", name], "pacman -Qi <package>")?;
        if !output.success() {
            return Err(AdapterError::command(
                "pacman -Qi <package>",
                format!("pacman -Qi {name}"),
                format!("the package is no longer installed: {}", output.stderr.trim()),
            ));
        }
        let records = parse_qi(&output.stdout);
        let Some(record) = records.first() else {
            return Err(AdapterError::Parse {
                context: format!("pacman -Qi {name}"),
                detail: "no record returned".to_string(),
            });
        };

        let dependents = record.list("Required By");
        let mut blocking = Vec::new();
        let mut affected = Vec::new();

        if !dependents.is_empty() {
            blocking.push(format!(
                "{} package(s) require it: {}. Removing it would break them, and this version \
                 does not cascade removals.",
                dependents.len(),
                summarise_list(&dependents)
            ));
            affected.extend(dependents.iter().map(|dependent| format!("dependent: {dependent}")));
        }

        let mut unknown = Vec::new();
        match self.run(&["-Ql", name], "pacman -Ql <package>") {
            Ok(listing) if listing.success() => {
                let files = listing.stdout.lines().filter(|line| !line.trim().is_empty()).count();
                affected.push(format!("{files} file(s) owned by the package (pacman -Ql)"));
            }
            _ => unknown.push(
                "the list of files owned by the package could not be read (pacman -Ql)".to_string(),
            ),
        }
        unknown.push("whether a package database backup exists is not checked".to_string());

        Ok(OperationPlan {
            adapter: ADAPTER_ID.to_string(),
            target: request.target.clone(),
            target_name: component.name.clone(),
            target_version: component.version.clone(),
            kind: OperationKind::Remove,
            commands: vec![crate::domain::PlannedCommand {
                program: "pacman".to_string(),
                args: vec!["-R".to_string(), name.clone()],
                requires_root: true,
                requires_tty: true,
                rationale: "remove this package and its files, keeping its dependencies installed"
                    .to_string(),
            }],
            affected,
            blocking,
            unknown,
            notes: vec![
                "dependencies are left installed and may become orphans; this operation never \
                 removes them"
                    .to_string(),
                "the operation is not reversible: pacman keeps no automatic backup of a removal"
                    .to_string(),
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

        // Execute exactly what was confirmed, in order, and stop at the first
        // failure instead of trying to repair it with another command.
        for command in &plan.commands {
            if command.program != "pacman" {
                return Err(AdapterError::InvalidId {
                    adapter: ADAPTER_ID.to_string(),
                    value: command.program.clone(),
                    reason: "this adapter only runs pacman".to_string(),
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
                format!("removed {}", plan.target_name)
            } else {
                format!("pacman did not complete the removal (exit {exit_code:?})")
            },
        })
    }
}

/// Removal is offered only where this adapter owns the record. Foreign packages
/// belong to their AUR helper's database.
fn capabilities_for(origin: &PacmanOrigin) -> Vec<ComponentCapability> {
    match origin {
        PacmanOrigin::Official { .. } => {
            vec![ComponentCapability::Inspect, ComponentCapability::Remove]
        }
        PacmanOrigin::Foreign => vec![ComponentCapability::Inspect],
    }
}

fn summarise_list(items: &[String]) -> String {
    const LIMIT: usize = 8;
    if items.len() <= LIMIT {
        return items.join(", ");
    }
    format!("{} … (+{} more)", items[..LIMIT].join(", "), items.len() - LIMIT)
}

impl PacmanAdapter {
    /// Component built from `pacman -Q` alone, used when `-Qi` was unavailable.
    fn minimal_component(
        &self,
        name: &str,
        version: &str,
        foreign: &BTreeSet<String>,
        orphans: &BTreeSet<String>,
    ) -> Component {
        let origin = if foreign.contains(name) {
            PacmanOrigin::Foreign
        } else {
            PacmanOrigin::Official { repo: None }
        };
        let capabilities = capabilities_for(&origin);
        Component {
            id: ComponentId::new(ADAPTER_ID, name.to_string()),
            name: name.to_string(),
            version: Some(version.to_string()),
            description: None,
            category: ComponentCategory::Package,
            source: ComponentSource::Pacman { origin },
            status: if orphans.contains(name) {
                ComponentStatus::Orphan
            } else {
                ComponentStatus::Installed
            },
            confidence: DetectionConfidence::Authoritative,
            capabilities,
            detection: DetectionMethod::command("pacman", vec!["-Q".to_string()])
                .with_note("details unavailable: pacman -Qi failed"),
            installed_size: None,
            location: None,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::AdapterAvailability;
    use crate::infrastructure::test_support::FakeRunner;

    const QI: &str = include_str!("../../../tests/fixtures/pacman/qi.txt");
    const Q: &str = include_str!("../../../tests/fixtures/pacman/q.txt");
    const QM: &str = include_str!("../../../tests/fixtures/pacman/qm.txt");
    const QDTQ: &str = include_str!("../../../tests/fixtures/pacman/qdtq.txt");
    const SI: &str = include_str!("../../../tests/fixtures/pacman/si.txt");

    fn adapter(runner: FakeRunner) -> PacmanAdapter {
        PacmanAdapter::new(Arc::new(runner))
    }

    fn complete_runner() -> FakeRunner {
        FakeRunner::new()
            .on("pacman --version", "Pacman v7.1.0 - libalpm v16.0.1\n")
            .on("pacman -Q", Q)
            .on("pacman -Qi", QI)
            .on("pacman -Qm", QM)
            .on("pacman -Qdtq", QDTQ)
    }

    #[test]
    fn reports_itself_unavailable_when_pacman_is_missing() {
        let status = adapter(FakeRunner::new()).status();
        assert_eq!(
            status.availability,
            AdapterAvailability::Unavailable {
                reason: "pacman was not found in the executable search path".to_string()
            }
        );
        assert!(status.tool.is_none());
    }

    #[test]
    fn reads_the_version_through_the_ascii_logo() {
        let stdout = "\n .--.                  Pacman v7.1.0 - libalpm v16.0.1\n/ _.-'  Copyright\n";
        assert_eq!(version_line(stdout).as_deref(), Some("Pacman v7.1.0 - libalpm v16.0.1"));
        assert_eq!(version_line(""), None);
    }

    #[test]
    fn reports_the_tool_and_version_when_available() {
        let status = adapter(complete_runner()).status();
        assert!(status.available());
        assert_eq!(status.tool_version.as_deref(), Some("Pacman v7.1.0 - libalpm v16.0.1"));
        assert!(status.commands.iter().any(|command| command == "pacman -Qi"));
    }

    #[test]
    fn lists_every_installed_package_with_its_own_facts() {
        let components = adapter(complete_runner()).list().expect("list");
        assert_eq!(components.len(), 4);

        let btop = components.iter().find(|component| component.name == "btop").expect("btop");
        assert_eq!(btop.id.to_string(), "pacman:btop");
        assert_eq!(btop.version.as_deref(), Some("1.4.0-1"));
        assert_eq!(btop.description.as_deref(), Some("A monitor of resources"));
        assert_eq!(btop.installed_size, Some(5_326_766));
        assert_eq!(btop.depends_on, vec!["gcc-libs", "glibc", "libstdc++.so=6-64"]);
        assert!(btop.required_by.is_empty());
        assert_eq!(btop.status, ComponentStatus::Installed);
        assert_eq!(btop.confidence, DetectionConfidence::Authoritative);
        assert_eq!(btop.category, ComponentCategory::Package);
        assert_eq!(
            btop.source,
            ComponentSource::Pacman { origin: PacmanOrigin::Official { repo: None } }
        );
        assert_eq!(btop.installed_at.as_deref(), Some("Mon 04 Aug 2025 10:22:33 PM -03"));
    }

    #[test]
    fn marks_foreign_packages_and_orphans_from_their_own_lists() {
        let components = adapter(complete_runner()).list().expect("list");

        let yay = components.iter().find(|component| component.name == "yay").expect("yay");
        assert_eq!(yay.source, ComponentSource::Pacman { origin: PacmanOrigin::Foreign });
        assert_eq!(yay.status, ComponentStatus::Installed);

        let libfoo = components.iter().find(|component| component.name == "libfoo").expect("libfoo");
        assert_eq!(libfoo.source, ComponentSource::Pacman { origin: PacmanOrigin::Foreign });
        assert_eq!(libfoo.status, ComponentStatus::Orphan);
    }

    #[test]
    fn a_dependent_package_reports_who_requires_it() {
        let components = adapter(complete_runner()).list().expect("list");
        let gcc = components.iter().find(|component| component.name == "gcc-libs").expect("gcc");
        assert_eq!(gcc.required_by, vec!["btop"]);
    }

    #[test]
    fn removal_is_offered_only_for_packages_this_adapter_owns() {
        let components = adapter(complete_runner()).list().expect("list");
        let pacman = adapter(complete_runner());

        let btop = components.iter().find(|component| component.name == "btop").expect("btop");
        assert_eq!(
            btop.capabilities,
            vec![ComponentCapability::Inspect, ComponentCapability::Remove]
        );
        assert!(pacman.supports(btop, OperationKind::Remove));

        // Foreign packages are removed through their AUR helper: a different
        // database owner, so this adapter must not claim them.
        let yay = components.iter().find(|component| component.name == "yay").expect("yay");
        assert_eq!(yay.capabilities, vec![ComponentCapability::Inspect]);
        assert!(!pacman.supports(yay, OperationKind::Remove));

        // Arch does not support partial upgrades, so no update is offered here.
        assert!(!pacman.supports(btop, OperationKind::Update));
        assert!(!pacman.supports(yay, OperationKind::Update));
    }

    #[test]
    fn keeps_the_authoritative_list_when_the_detail_dump_fails() {
        let runner = FakeRunner::new()
            .on("pacman -Q", Q)
            .on_exit("pacman -Qi", 1, "", "error: database corrupted")
            .on("pacman -Qm", QM)
            .on("pacman -Qdtq", QDTQ);

        let components = adapter(runner).list().expect("list must not fail");
        assert_eq!(components.len(), 4);
        let btop = components.iter().find(|component| component.name == "btop").expect("btop");
        assert_eq!(btop.version.as_deref(), Some("1.4.0-1"));
        assert!(btop.description.is_none(), "no invented description");
        assert!(btop.detection.note.as_deref().unwrap_or_default().contains("pacman -Qi failed"));
    }

    #[test]
    fn a_failing_package_list_is_an_error_not_an_empty_inventory() {
        let runner = FakeRunner::new().on_exit("pacman -Q", 1, "", "error: could not open database");
        let error = adapter(runner).list().expect_err("must fail");
        assert!(matches!(error, AdapterError::Command { .. }), "unexpected error: {error}");
    }

    #[test]
    fn an_empty_orphan_list_is_not_an_error() {
        let runner = FakeRunner::new()
            .on("pacman -Q", "btop 1.4.0-1\n")
            .on("pacman -Qi", "Name : btop\nVersion : 1.4.0-1\n")
            .on("pacman -Qm", "")
            .on_exit("pacman -Qdtq", 1, "", "");
        let components = adapter(runner).list().expect("empty lists are valid");
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].status, ComponentStatus::Installed);
    }

    #[test]
    fn details_refuses_a_hostile_identifier_before_running_anything() {
        let error = adapter(complete_runner())
            .details("btop; rm -rf /")
            .expect_err("must refuse");
        match error {
            AdapterError::InvalidId { value, .. } => assert_eq!(value, "btop; rm -rf /"),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn details_reports_a_component_that_no_longer_exists() {
        let runner = complete_runner()
            .on_exit("pacman -Qi ghost", 1, "", "error: package 'ghost' was not found\n");
        assert!(adapter(runner).details("ghost").expect("details").is_none());
    }

    #[test]
    fn details_fills_the_repository_from_a_single_si_query() {
        let runner = complete_runner()
            .on("pacman -Qi btop", "Name : btop\nVersion : 1.4.0-1\nDepends On : glibc\n")
            .on("pacman -Si btop", SI);
        let component = adapter(runner).details("btop").expect("details").expect("present");
        assert_eq!(
            component.source,
            ComponentSource::Pacman { origin: PacmanOrigin::Official { repo: Some("extra".to_string()) } }
        );
        assert_eq!(component.source.label(), "pacman / extra");
    }

    #[test]
    fn details_never_invents_a_repository_when_si_fails() {
        let runner = complete_runner()
            .on("pacman -Qi btop", "Name : btop\nVersion : 1.4.0-1\n")
            .on_exit("pacman -Si btop", 1, "", "error: package not found");
        let component = adapter(runner).details("btop").expect("details").expect("present");
        assert_eq!(
            component.source,
            ComponentSource::Pacman { origin: PacmanOrigin::Official { repo: None } }
        );
        assert_eq!(component.source.label(), "pacman / official");
    }
}

#[cfg(test)]
mod removal_tests {
    use super::*;
    use crate::domain::{ComponentCapability, PlannedCommand};
    use crate::infrastructure::test_support::FakeRunner;
    use std::sync::Arc;

    const QI: &str = include_str!("../../../tests/fixtures/pacman/qi.txt");

    fn runner_with_btop(dependents: bool, files: Option<&str>) -> FakeRunner {
        let qi = if dependents {
            "Name : btop\nVersion : 1.4.0-1\nRequired By : hyprland\n".to_string()
        } else {
            "Name : btop\nVersion : 1.4.0-1\nRequired By : None\n".to_string()
        };
        let mut runner = FakeRunner::new().on("pacman -Qi btop", &qi);
        match files {
            Some(listing) => runner = runner.on("pacman -Ql btop", listing),
            None => runner = runner.on_exit("pacman -Ql btop", 1, "", "error: database locked"),
        }
        runner
    }

    fn component() -> Component {
        Component {
            id: ComponentId::new("pacman", "btop"),
            name: "btop".to_string(),
            version: Some("1.4.0-1".to_string()),
            description: None,
            category: ComponentCategory::Package,
            source: ComponentSource::Pacman { origin: PacmanOrigin::Official { repo: Some("extra".into()) } },
            status: ComponentStatus::Installed,
            confidence: DetectionConfidence::Authoritative,
            capabilities: vec![ComponentCapability::Inspect, ComponentCapability::Remove],
            detection: DetectionMethod::command("pacman", vec!["-Q".to_string()]),
            installed_size: None,
            location: None,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: Vec::new(),
        }
    }

    fn request() -> OperationRequest {
        OperationRequest::new(ComponentId::new("pacman", "btop"), OperationKind::Remove)
    }

    #[test]
    fn the_plan_is_exactly_one_non_cascading_removal() {
        let adapter = PacmanAdapter::new(Arc::new(runner_with_btop(false, Some("btop /usr/bin/btop\n"))));
        let plan = adapter.plan_operation(&request(), &component()).expect("plan");

        assert_eq!(plan.commands.len(), 1);
        let command = &plan.commands[0];
        assert_eq!(command.display(), "pacman -R btop");
        assert_eq!(command.args, vec!["-R".to_string(), "btop".to_string()]);
        assert!(command.requires_root);
        assert!(command.requires_tty);
        // No recursive or forced removal is ever proposed by this version.
        for forbidden in ["-Rns", "-Rdd", "-Rsc", "-Rn", "-Rc"] {
            assert!(!command.args.iter().any(|arg| arg == forbidden), "{forbidden} must not appear");
        }
        assert!(plan.is_executable());
    }

    #[test]
    fn a_plan_with_dependents_is_blocked_and_names_them() {
        let adapter = PacmanAdapter::new(Arc::new(runner_with_btop(true, Some("btop /usr/bin/btop\n"))));
        let plan = adapter.plan_operation(&request(), &component()).expect("plan");

        assert!(!plan.is_executable());
        assert!(plan.blocking.iter().any(|reason| reason.contains("hyprland")));
        assert!(plan.affected.iter().any(|item| item.contains("hyprland")));
        assert!(plan.blocking[0].contains("does not cascade"));
    }

    #[test]
    fn the_plan_states_how_many_files_would_be_removed() {
        let adapter = PacmanAdapter::new(Arc::new(runner_with_btop(
            false,
            Some("btop /usr/bin/btop\nbtop /usr/share/doc/btop\n"),
        )));
        let plan = adapter.plan_operation(&request(), &component()).expect("plan");
        assert!(plan.affected.iter().any(|item| item.contains("2 file(s)")), "{:?}", plan.affected);
        assert!(!plan.unknown.iter().any(|fact| fact.contains("pacman -Ql")));
    }

    #[test]
    fn a_failed_file_listing_becomes_an_unknown_fact_not_a_zero() {
        let adapter = PacmanAdapter::new(Arc::new(runner_with_btop(false, None)));
        let plan = adapter.plan_operation(&request(), &component()).expect("plan");
        assert!(plan.unknown.iter().any(|fact| fact.contains("pacman -Ql")), "{:?}", plan.unknown);
        assert!(!plan.affected.iter().any(|item| item.contains("file(s)")));
    }

    #[test]
    fn the_plan_never_claims_a_removal_is_reversible() {
        let adapter = PacmanAdapter::new(Arc::new(runner_with_btop(false, Some("btop /usr/bin/btop\n"))));
        let plan = adapter.plan_operation(&request(), &component()).expect("plan");
        assert!(plan.notes.iter().any(|note| note.contains("not reversible")));
    }

    #[test]
    fn a_hostile_target_is_refused_before_any_process_runs() {
        let runner = Arc::new(FakeRunner::new().on("pacman -Qi btop", QI));
        let adapter = PacmanAdapter::new(Arc::clone(&runner) as Arc<dyn CommandRunner>);
        let hostile = OperationRequest::new(
            ComponentId::new("pacman", "btop; rm -rf /"),
            OperationKind::Remove,
        );
        let error = adapter.plan_operation(&hostile, &component()).expect_err("must refuse");
        assert!(matches!(error, AdapterError::InvalidId { .. }), "unexpected: {error}");
        assert!(runner.calls().is_empty(), "no process may run for a hostile identifier");
    }

    #[test]
    fn a_package_that_vanished_cannot_be_planned() {
        let runner = FakeRunner::new().on_exit(
            "pacman -Qi btop",
            1,
            "",
            "error: package 'btop' was not found\n",
        );
        let adapter = PacmanAdapter::new(Arc::new(runner));
        let error = adapter.plan_operation(&request(), &component()).expect_err("must fail");
        assert!(matches!(error, AdapterError::Command { .. }), "unexpected: {error}");
    }

    #[test]
    fn execution_runs_exactly_the_confirmed_command() {
        let runner = Arc::new(
            FakeRunner::new().on_exit("pacman -R btop", 0, "removing btop...\n", ""),
        );
        let adapter = PacmanAdapter::new(Arc::clone(&runner) as Arc<dyn CommandRunner>);
        let plan = OperationPlan {
            adapter: "pacman".to_string(),
            target: ComponentId::new("pacman", "btop"),
            target_name: "btop".to_string(),
            target_version: Some("1.4.0-1".to_string()),
            kind: OperationKind::Remove,
            commands: vec![PlannedCommand {
                program: "pacman".to_string(),
                args: vec!["-R".to_string(), "btop".to_string()],
                requires_root: true,
                requires_tty: false,
                rationale: "remove".to_string(),
            }],
            affected: Vec::new(),
            blocking: Vec::new(),
            unknown: Vec::new(),
            notes: Vec::new(),
        };
        let outcome = adapter.execute(&plan).expect("execute");
        assert!(outcome.success);
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(runner.calls(), vec!["pacman -R btop".to_string()]);
    }

    #[test]
    fn a_failing_removal_is_never_reported_as_success() {
        let runner = Arc::new(FakeRunner::new().on_exit(
            "pacman -R btop",
            1,
            "",
            "error: could not remove package: permission denied\n",
        ));
        let adapter = PacmanAdapter::new(Arc::clone(&runner) as Arc<dyn CommandRunner>);
        let plan = OperationPlan {
            adapter: "pacman".to_string(),
            target: ComponentId::new("pacman", "btop"),
            target_name: "btop".to_string(),
            target_version: None,
            kind: OperationKind::Remove,
            commands: vec![PlannedCommand {
                program: "pacman".to_string(),
                args: vec!["-R".to_string(), "btop".to_string()],
                requires_root: true,
                requires_tty: false,
                rationale: "remove".to_string(),
            }],
            affected: Vec::new(),
            blocking: Vec::new(),
            unknown: Vec::new(),
            notes: Vec::new(),
        };
        let outcome = adapter.execute(&plan).expect("execute returns the outcome");
        assert!(!outcome.success);
        assert_eq!(outcome.exit_code, Some(1));
        assert!(outcome.stderr_tail.contains("permission denied"));
        assert!(outcome.summary.contains("did not complete"));
    }

    #[test]
    fn a_tampered_plan_cannot_make_this_adapter_run_another_program() {
        let runner = Arc::new(FakeRunner::new().on("rm -rf /", ""));
        let adapter = PacmanAdapter::new(Arc::clone(&runner) as Arc<dyn CommandRunner>);
        let plan = OperationPlan {
            adapter: "pacman".to_string(),
            target: ComponentId::new("pacman", "btop"),
            target_name: "btop".to_string(),
            target_version: None,
            kind: OperationKind::Remove,
            commands: vec![PlannedCommand {
                program: "rm".to_string(),
                args: vec!["-rf".to_string(), "/".to_string()],
                requires_root: true,
                requires_tty: true,
                rationale: "tampered".to_string(),
            }],
            affected: Vec::new(),
            blocking: Vec::new(),
            unknown: Vec::new(),
            notes: Vec::new(),
        };
        let error = adapter.execute(&plan).expect_err("must refuse");
        assert!(matches!(error, AdapterError::InvalidId { .. }), "unexpected: {error}");
        assert!(runner.calls().is_empty(), "nothing may run");
    }

    #[test]
    fn a_plan_from_another_adapter_is_refused() {
        let runner = Arc::new(FakeRunner::new());
        let adapter = PacmanAdapter::new(Arc::clone(&runner) as Arc<dyn CommandRunner>);
        let mut plan = OperationPlan {
            adapter: "flatpak".to_string(),
            target: ComponentId::new("pacman", "btop"),
            target_name: "btop".to_string(),
            target_version: None,
            kind: OperationKind::Remove,
            commands: vec![PlannedCommand {
                program: "pacman".to_string(),
                args: vec!["-R".to_string(), "btop".to_string()],
                requires_root: true,
                requires_tty: true,
                rationale: "remove".to_string(),
            }],
            affected: Vec::new(),
            blocking: Vec::new(),
            unknown: Vec::new(),
            notes: Vec::new(),
        };
        plan.adapter = "flatpak".to_string();
        assert!(adapter.execute(&plan).is_err());
        assert!(runner.calls().is_empty());
    }
}
