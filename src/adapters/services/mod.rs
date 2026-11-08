//! systemd services, in the system and the user scope.
//!
//! Commands used, and why:
//! * `systemctl list-units --type=service --all --no-pager --no-legend` — every loaded
//!   service unit with its ACTIVE and SUB columns and its description, in one call.
//! * `systemctl list-unit-files --type=service --no-pager --no-legend` — every installed
//!   unit file with its state (`enabled`, `disabled`, `static`, `masked`), in one call.
//!   A unit that is installed but not loaded only appears here.
//! * `systemctl show -p FragmentPath,ActiveState,UnitFileState,Description <unit>` — the
//!   detail block of one unit, run only when the details view is opened. Calling it
//!   while listing would mean one process per unit.
//!
//! The user scope takes `--user` before the verb (`systemctl --user list-units …`), and
//! the two scopes are queried independently: a failing scope is reported and the other
//! one still loads (spec S2).
//!
//! Inspection only. Stopping, disabling and removing are different actions and none of
//! them is implemented here (task T20); the only capability is inspection, so nothing in
//! this program can stop a service by accident.
//!
//! Identity includes the scope (`user:voxtype.service`) because the same unit name can
//! exist in both scopes, and a bare name would collide (spec S4).

pub mod parse;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::domain::{
    AdapterAvailability, AdapterError, AdapterStatus, Component, ComponentAdapter,
    ComponentCapability, ComponentCategory, ComponentId, ComponentSource, ComponentStatus,
    DetailField, DetectionConfidence, DetectionMethod, InstallScope, OperationKind,
    OperationOutcome, OperationPlan, OperationRequest, UNAVAILABLE,
};
use crate::infrastructure::process::{CommandOutput, CommandRunner, CommandSpec, ProcessError};

use self::parse::{UnitRow, parse_show, parse_unit_files, parse_units};

const ADAPTER_ID: &str = "systemd";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

/// The properties the detail view needs, in one `systemctl show` call.
const SHOW_PROPERTIES: &str = "FragmentPath,ActiveState,UnitFileState,Description";

const LIST_UNITS: &[&str] = &[
    "list-units",
    "--type=service",
    "--all",
    "--no-pager",
    "--no-legend",
];
const LIST_UNIT_FILES: &[&str] = &[
    "list-unit-files",
    "--type=service",
    "--no-pager",
    "--no-legend",
];

const COMMANDS: &[&str] = &[
    "systemctl list-units --type=service --all --no-pager --no-legend",
    "systemctl list-unit-files --type=service --no-pager --no-legend",
    "systemctl --user list-units --type=service --all --no-pager --no-legend",
    "systemctl --user list-unit-files --type=service --no-pager --no-legend",
    "systemctl show -p FragmentPath,ActiveState,UnitFileState,Description <unit>",
    "systemctl --user show -p FragmentPath,ActiveState,UnitFileState,Description <unit>",
];

/// Scope is passed to `systemctl` before the verb, and only for the user scope.
fn scope_args(scope: InstallScope, verb: &[&str]) -> Vec<String> {
    let mut args = Vec::new();
    if scope == InstallScope::User {
        args.push("--user".to_string());
    }
    args.extend(verb.iter().map(|argument| (*argument).to_string()));
    args
}

fn display(program: &str, args: &[String]) -> String {
    let mut out = program.to_string();
    for arg in args {
        out.push(' ');
        out.push_str(arg);
    }
    out
}

/// A unit name reaches `systemctl`'s argv and is joined onto nothing, but it still comes
/// from a tool and is shown to a human: it must be a unit name, never an option.
fn validate_unit_name(value: &str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("must not be empty");
    }
    if value.len() > 255 {
        return Err("is longer than 255 bytes");
    }
    if value.trim() != value {
        return Err("has leading or trailing whitespace");
    }
    if value.starts_with('-') {
        return Err("must not start with '-' (that would be read as an option)");
    }
    if value.contains("..") {
        return Err("contains a parent-directory segment");
    }
    if value
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err("contains whitespace or a control character");
    }
    if value.chars().any(|character| {
        // `\` is part of systemd's escaped unit names (`foo\x2dbar.service`).
        !(character.is_ascii_alphanumeric()
            || matches!(character, '.' | '_' | '-' | '@' | ':' | '\\'))
    }) {
        return Err("contains a character outside the allowed set");
    }
    Ok(())
}

fn status_from_active(active: Option<&str>) -> ComponentStatus {
    match active {
        Some("active") => ComponentStatus::Active,
        // `failed`, `inactive` and `activating` are all "not active": the exact word
        // stays visible in the ACTIVE field instead of being flattened away.
        Some(_) => ComponentStatus::Inactive,
        None => ComponentStatus::Unknown,
    }
}

pub struct ServicesAdapter {
    runner: Arc<dyn CommandRunner>,
}

impl ServicesAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }

    fn run(&self, args: Vec<String>, context: &str) -> Result<CommandOutput, AdapterError> {
        let spec = CommandSpec::capture("systemctl", args).with_timeout(COMMAND_TIMEOUT);
        self.runner.run(&spec).map_err(|error| match error {
            ProcessError::NotFound { .. } => AdapterError::ToolUnavailable {
                tool: "systemctl".to_string(),
                reason: "not found in the executable search path".to_string(),
            },
            other => AdapterError::command(context, spec.display(), other.to_string()),
        })
    }

    /// Lists one scope. Both invocations are bulk calls; the detail properties are never
    /// fetched while listing.
    fn load_scope(&self, scope: InstallScope) -> Result<Vec<Component>, AdapterError> {
        let units_args = scope_args(scope, LIST_UNITS);
        let units = self.run(units_args.clone(), "systemctl list-units")?;
        if !units.success() {
            return Err(AdapterError::command(
                "systemctl list-units",
                display("systemctl", &units_args),
                format!("exit {:?}: {}", units.exit_code, units.stderr.trim()),
            ));
        }
        let rows = parse_units(&units.stdout);

        // The installed/loaded state is a second fact from a second source. If it is
        // missing, the active state is still true and worth showing, so this degrades
        // instead of failing the scope.
        let files_args = scope_args(scope, LIST_UNIT_FILES);
        let file_states = match self.run(files_args.clone(), "systemctl list-unit-files") {
            Ok(output) if output.success() => parse_unit_files(&output.stdout),
            Ok(output) => {
                tracing::warn!(
                    exit = ?output.exit_code,
                    stderr = output.stderr.trim(),
                    "systemctl list-unit-files failed; the enabled state stays unreported"
                );
                BTreeMap::new()
            }
            Err(error) => {
                tracing::warn!(%error, "systemctl list-unit-files failed; the enabled state stays unreported");
                BTreeMap::new()
            }
        };

        // Merge by unit name: a unit that is installed but not loaded exists only in the
        // unit-file list, and hiding it would hide a disabled service.
        let mut names: BTreeSet<&str> = rows.keys().map(String::as_str).collect();
        names.extend(file_states.keys().map(String::as_str));

        let mut components = Vec::with_capacity(names.len());
        for name in names {
            if let Err(reason) = validate_unit_name(name) {
                tracing::warn!(unit = %name, reason, "skipping an unusable systemd unit name");
                continue;
            }
            components.push(self.component(
                scope,
                name,
                rows.get(name),
                file_states.get(name).map(String::as_str),
            ));
        }
        Ok(components)
    }

    fn component(
        &self,
        scope: InstallScope,
        unit: &str,
        row: Option<&UnitRow>,
        file_state: Option<&str>,
    ) -> Component {
        let active = row.map(|row| row.active.as_str());
        let description = row
            .map(|row| row.description.clone())
            .filter(|description| !description.is_empty());

        Component {
            id: ComponentId::new(ADAPTER_ID, format!("{}:{unit}", scope.label())),
            name: unit.to_string(),
            // systemd units carry no version of their own.
            version: None,
            description: description.clone(),
            category: ComponentCategory::Service,
            source: ComponentSource::Systemd { scope },
            status: status_from_active(active),
            confidence: DetectionConfidence::Authoritative,
            capabilities: vec![ComponentCapability::Inspect],
            detection: DetectionMethod::command("systemctl", scope_args(scope, LIST_UNITS))
                .with_note(
                    "state from systemctl list-units, unit file state from \
                     systemctl list-unit-files",
                ),
            installed_size: None,
            // The unit file lives in `FragmentPath`, which needs `systemctl show`: it is
            // fetched for one unit in the details view, never for every unit here.
            location: None,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: vec![
                DetailField::new(
                    "ACTIVE",
                    active.unwrap_or("not reported by systemctl list-units --all"),
                ),
                DetailField::new(
                    "SUB",
                    row.map(|row| row.sub.clone())
                        .unwrap_or_else(|| UNAVAILABLE.to_string()),
                ),
                DetailField::new(
                    "UNIT FILE STATE",
                    file_state.unwrap_or("not reported by systemctl list-unit-files"),
                ),
                DetailField::new(
                    "DESCRIPTION",
                    description.unwrap_or(UNAVAILABLE.to_string()),
                ),
            ],
        }
    }

    fn split_native_id<'a>(
        &self,
        native_id: &'a str,
    ) -> Result<(InstallScope, &'a str), AdapterError> {
        let invalid = |reason: &str| AdapterError::InvalidId {
            adapter: ADAPTER_ID.to_string(),
            value: native_id.to_string(),
            reason: reason.to_string(),
        };
        let (scope, unit) = native_id
            .split_once(':')
            .ok_or_else(|| invalid("expected <scope>:<unit>, for example user:voxtype.service"))?;
        let scope = match scope {
            "system" => InstallScope::System,
            "user" => InstallScope::User,
            other => return Err(invalid(&format!("unknown scope {other:?}"))),
        };
        validate_unit_name(unit).map_err(invalid)?;
        Ok((scope, unit))
    }
}

impl ComponentAdapter for ServicesAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn display_name(&self) -> &'static str {
        "Services"
    }

    fn description(&self) -> &'static str {
        "systemd service units in the system and user scope, with their state and whether \
         they are enabled"
    }

    fn capabilities(&self) -> Vec<ComponentCapability> {
        vec![ComponentCapability::Inspect]
    }

    fn status(&self) -> AdapterStatus {
        let notes = vec![
            "system and user scope are queried independently: a scope whose command fails is \
             reported and the other one still loads"
                .to_string(),
            "stopping, disabling and removing a service are different actions, and none of \
             them is implemented in this version"
                .to_string(),
        ];

        match self.runner.resolve("systemctl") {
            Some(tool) => AdapterStatus {
                id: ADAPTER_ID.to_string(),
                display_name: self.display_name().to_string(),
                description: self.description().to_string(),
                availability: AdapterAvailability::Available,
                tool: Some(tool),
                tool_version: None,
                capabilities: self.capabilities(),
                commands: COMMANDS
                    .iter()
                    .map(|command| (*command).to_string())
                    .collect(),
                notes,
            },
            None => AdapterStatus {
                id: ADAPTER_ID.to_string(),
                display_name: self.display_name().to_string(),
                description: self.description().to_string(),
                availability: AdapterAvailability::Unavailable {
                    reason: "systemctl was not found in the executable search path".to_string(),
                },
                tool: None,
                tool_version: None,
                capabilities: self.capabilities(),
                commands: COMMANDS
                    .iter()
                    .map(|command| (*command).to_string())
                    .collect(),
                notes,
            },
        }
    }

    fn list(&self) -> Result<Vec<Component>, AdapterError> {
        let mut components = Vec::new();
        let mut first_error: Option<AdapterError> = None;
        let mut loaded_any_scope = false;

        for scope in [InstallScope::System, InstallScope::User] {
            match self.load_scope(scope) {
                Ok(mut loaded) => {
                    loaded_any_scope = true;
                    components.append(&mut loaded);
                }
                Err(error) => {
                    // One scope failing is a fact about that scope, not about the other
                    // one (spec S2): keep what could be read and remember the failure.
                    tracing::warn!(
                        scope = scope.label(),
                        %error,
                        "a systemd scope failed; the other scope still loads"
                    );
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }

        if !loaded_any_scope {
            return Err(first_error.unwrap_or_else(|| {
                AdapterError::Other("no systemd scope could be read".to_string())
            }));
        }

        // `list-unit-files` also names units that no scope has loaded. They are real
        // installed units, so they stay in the inventory, but `Unknown` would be a
        // non-answer: the unit file itself says whether they are enabled or disabled.
        for component in &mut components {
            if component.status != ComponentStatus::Unknown {
                continue;
            }
            let unit_file_state = component
                .extra
                .iter()
                .find(|field| field.label == "Unit file state")
                .map(|field| field.value.as_str())
                .unwrap_or_default();
            component.status = if unit_file_state.starts_with("enabled") {
                ComponentStatus::Enabled
            } else {
                ComponentStatus::Disabled
            };
        }
        Ok(components)
    }

    fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError> {
        let (scope, unit) = self.split_native_id(native_id)?;
        let args = scope_args(scope, &["show", "-p", SHOW_PROPERTIES, unit]);
        let output = self.run(args.clone(), "systemctl show")?;

        if !output.success() {
            // systemd answers a unit it cannot resolve with a command failure. Treating
            // that as "gone" cancels the pending operation instead of proceeding (S15).
            tracing::warn!(
                unit,
                exit = ?output.exit_code,
                stderr = output.stderr.trim(),
                "systemctl show failed; reporting the unit as gone"
            );
            return Ok(None);
        }

        let properties = parse_show(&output.stdout);
        // A unit that is installed has a fragment or a unit-file state. Without both it
        // is only a name systemd echoed back, so there is nothing to report.
        if !properties.contains_key("FragmentPath") && !properties.contains_key("UnitFileState") {
            return Ok(None);
        }

        let active = properties.get("ActiveState").map(String::as_str);
        let fragment = properties.get("FragmentPath");
        let description = properties.get("Description").cloned();

        Ok(Some(Component {
            id: ComponentId::new(ADAPTER_ID, native_id.to_string()),
            name: unit.to_string(),
            version: None,
            description: description.clone(),
            category: ComponentCategory::Service,
            source: ComponentSource::Systemd { scope },
            status: status_from_active(active),
            confidence: DetectionConfidence::Authoritative,
            capabilities: vec![ComponentCapability::Inspect],
            detection: DetectionMethod::command("systemctl", args)
                .with_note("properties from systemctl show"),
            installed_size: None,
            location: fragment.map(PathBuf::from),
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: vec![
                DetailField::new("ACTIVE", active.unwrap_or(UNAVAILABLE)),
                DetailField::new(
                    "UNIT FILE STATE",
                    properties
                        .get("UnitFileState")
                        .map(String::as_str)
                        .unwrap_or(UNAVAILABLE),
                ),
                DetailField::new(
                    "FRAGMENT PATH",
                    fragment.map(String::as_str).unwrap_or(UNAVAILABLE),
                ),
                DetailField::new(
                    "DESCRIPTION",
                    description.unwrap_or(UNAVAILABLE.to_string()),
                ),
            ],
        }))
    }

    /// Nothing can be stopped, disabled or removed in this version (task T20), so no
    /// operation is ever claimed.
    fn supports(&self, component: &Component, kind: OperationKind) -> bool {
        let _ = (component, kind);
        false
    }

    fn plan_operation(
        &self,
        request: &OperationRequest,
        component: &Component,
    ) -> Result<OperationPlan, AdapterError> {
        let _ = component;
        Err(AdapterError::unsupported(ADAPTER_ID, request.kind.label()))
    }

    fn execute(&self, plan: &OperationPlan) -> Result<OperationOutcome, AdapterError> {
        let _ = plan;
        Err(AdapterError::unsupported(ADAPTER_ID, "execute"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::process::ProcessError;
    use crate::infrastructure::test_support::FakeRunner;

    const UNITS_SYSTEM: &str = include_str!("../../../tests/fixtures/systemd/units-system.txt");
    const UNITS_USER: &str = include_str!("../../../tests/fixtures/systemd/units-user.txt");
    const UNIT_FILES_SYSTEM: &str =
        include_str!("../../../tests/fixtures/systemd/unit-files-system.txt");
    const UNIT_FILES_USER: &str =
        include_str!("../../../tests/fixtures/systemd/unit-files-user.txt");
    const SHOW: &str = include_str!("../../../tests/fixtures/systemd/show.txt");

    const SYSTEM_UNITS_CMD: &str =
        "systemctl list-units --type=service --all --no-pager --no-legend";
    const SYSTEM_UNIT_FILES_CMD: &str =
        "systemctl list-unit-files --type=service --no-pager --no-legend";
    const USER_UNITS_CMD: &str =
        "systemctl --user list-units --type=service --all --no-pager --no-legend";
    const USER_UNIT_FILES_CMD: &str =
        "systemctl --user list-unit-files --type=service --no-pager --no-legend";
    const USER_SHOW_CMD: &str = "systemctl --user show -p \
                                  FragmentPath,ActiveState,UnitFileState,Description voxtype.service";

    fn runner() -> FakeRunner {
        FakeRunner::new()
            .on(SYSTEM_UNITS_CMD, UNITS_SYSTEM)
            .on(SYSTEM_UNIT_FILES_CMD, UNIT_FILES_SYSTEM)
            .on(USER_UNITS_CMD, UNITS_USER)
            .on(USER_UNIT_FILES_CMD, UNIT_FILES_USER)
    }

    fn adapter(runner: FakeRunner) -> ServicesAdapter {
        ServicesAdapter::new(Arc::new(runner))
    }

    fn find<'a>(components: &'a [Component], id: &str) -> &'a Component {
        components
            .iter()
            .find(|component| component.id.native_id == id)
            .unwrap_or_else(|| panic!("{id} must be listed"))
    }

    fn field(component: &Component, label: &str) -> Option<String> {
        component
            .extra
            .iter()
            .find(|field| field.label == label)
            .map(|field| field.value.clone())
    }

    #[test]
    fn system_and_user_units_are_merged_and_keep_their_scope_in_the_identity() {
        let runner = runner();
        let components = adapter(runner).list().expect("list");

        // 8 system units plus 5 user units, and `voxtype.service` exists in both scopes.
        assert_eq!(components.len(), 13);

        let system = find(&components, "system:NetworkManager.service");
        assert_eq!(system.name, "NetworkManager.service");
        assert_eq!(system.category, ComponentCategory::Service);
        assert_eq!(
            system.source,
            ComponentSource::Systemd {
                scope: InstallScope::System
            }
        );
        assert_eq!(system.status, ComponentStatus::Active);
        assert_eq!(
            system.description.as_deref(),
            Some("Network Manager"),
            "the description is free text and must survive"
        );
        assert_eq!(
            system.capabilities,
            vec![ComponentCapability::Inspect],
            "inspection only"
        );
        assert_eq!(field(system, "SUB").as_deref(), Some("running"));

        let user = find(&components, "user:dbus.service");
        assert_eq!(
            user.source,
            ComponentSource::Systemd {
                scope: InstallScope::User
            }
        );

        // The same unit name in two scopes must not collide (spec S4).
        assert_eq!(
            find(&components, "system:voxtype.service").status,
            ComponentStatus::Active
        );
        assert_eq!(
            find(&components, "user:voxtype.service").status,
            ComponentStatus::Inactive,
            "the user unit is installed but not running"
        );
    }

    #[test]
    fn the_two_bulk_calls_per_scope_are_the_only_ones_made_while_listing() {
        let shared = Arc::new(runner());
        let adapter = ServicesAdapter::new(Arc::clone(&shared) as Arc<dyn CommandRunner>);
        adapter.list().expect("list");
        assert_eq!(
            shared.calls(),
            vec![
                SYSTEM_UNITS_CMD.to_string(),
                SYSTEM_UNIT_FILES_CMD.to_string(),
                USER_UNITS_CMD.to_string(),
                USER_UNIT_FILES_CMD.to_string(),
            ],
            "listing must not call systemctl show once per unit"
        );
    }

    #[test]
    fn an_inactive_unit_is_not_reported_as_active() {
        let components = adapter(runner()).list().expect("list");
        let init = find(&components, "system:pacman-init.service");
        assert_eq!(init.status, ComponentStatus::Inactive);
        assert_eq!(field(init, "ACTIVE").as_deref(), Some("inactive"));
        assert_eq!(field(init, "SUB").as_deref(), Some("dead"));

        let cups = find(&components, "system:cups.service");
        assert_eq!(
            cups.status,
            ComponentStatus::Inactive,
            "a failed unit is not active, and the exact word stays visible"
        );
        assert_eq!(field(cups, "ACTIVE").as_deref(), Some("failed"));
    }

    #[test]
    fn a_disabled_unit_keeps_its_unit_file_state_separate_from_being_active() {
        let components = adapter(runner()).list().expect("list");
        let bluetooth = find(&components, "system:bluetooth.service");
        assert_eq!(bluetooth.status, ComponentStatus::Active);
        assert_eq!(
            field(bluetooth, "UNIT FILE STATE").as_deref(),
            Some("disabled"),
            "enabled/disabled is a different fact from running"
        );

        let journald = find(&components, "system:systemd-journald.service");
        assert_eq!(
            field(journald, "UNIT FILE STATE").as_deref(),
            Some("static")
        );
    }

    #[test]
    fn a_unit_that_exists_only_as_a_unit_file_reports_its_unit_file_state() {
        // Only the unit-file list is registered: the installed units still have to show
        // up, with their active state honestly unreported.
        let runner = FakeRunner::new()
            .on(
                SYSTEM_UNITS_CMD,
                "  other.service loaded active running Something else\n",
            )
            .on(
                SYSTEM_UNIT_FILES_CMD,
                "other.service enabled enabled\nidle-only.service disabled enabled\n",
            )
            .on(USER_UNITS_CMD, "")
            .on(USER_UNIT_FILES_CMD, "");

        let components = adapter(runner).list().expect("list");
        let idle = find(&components, "system:idle-only.service");
        // A unit that is not loaded but is disabled by its unit file says so, instead of
        // reporting "unknown".
        assert_eq!(idle.status, ComponentStatus::Disabled);
        assert_eq!(
            field(idle, "ACTIVE").as_deref(),
            Some("not reported by systemctl list-units --all")
        );
        assert_eq!(field(idle, "UNIT FILE STATE").as_deref(), Some("disabled"));
    }

    #[test]
    fn a_failing_user_scope_leaves_the_system_scope_intact() {
        let runner = FakeRunner::new()
            .on(SYSTEM_UNITS_CMD, UNITS_SYSTEM)
            .on(SYSTEM_UNIT_FILES_CMD, UNIT_FILES_SYSTEM)
            .on_exit(
                USER_UNITS_CMD,
                1,
                "",
                "Failed to connect to bus: No such file or directory\n",
            );

        let components = adapter(runner)
            .list()
            .expect("one failed scope is not fatal");
        assert_eq!(components.len(), 8);
        assert!(
            components
                .iter()
                .all(|component| component.id.native_id.starts_with("system:"))
        );
    }

    #[test]
    fn a_missing_systemctl_makes_the_integration_unavailable() {
        let status = adapter(FakeRunner::new()).status();
        assert_eq!(
            status.availability,
            AdapterAvailability::Unavailable {
                reason: "systemctl was not found in the executable search path".to_string()
            }
        );
        assert!(
            status
                .commands
                .iter()
                .any(|command| command.contains("list-units"))
        );
        assert!(status.notes.iter().any(|note| {
            note.contains("stopping, disabling and removing a service are different actions")
        }));
    }

    #[test]
    fn details_reads_one_unit_with_a_single_show_call() {
        let runner = FakeRunner::new().on(USER_SHOW_CMD, SHOW);
        let details = adapter(runner)
            .details("user:voxtype.service")
            .expect("details")
            .expect("present");

        assert_eq!(
            details.id,
            ComponentId::new("systemd", "user:voxtype.service")
        );
        assert_eq!(details.name, "voxtype.service");
        assert_eq!(details.status, ComponentStatus::Inactive);
        assert_eq!(
            details.description.as_deref(),
            Some("Voxtype dictation daemon")
        );
        assert_eq!(
            details.location,
            Some(PathBuf::from(
                "/home/alex/.config/systemd/user/voxtype.service"
            ))
        );
        assert_eq!(field(&details, "ACTIVE").as_deref(), Some("inactive"));
        assert_eq!(
            field(&details, "UNIT FILE STATE").as_deref(),
            Some("disabled")
        );
        assert_eq!(
            field(&details, "FRAGMENT PATH").as_deref(),
            Some("/home/alex/.config/systemd/user/voxtype.service")
        );
        assert_eq!(
            details.source,
            ComponentSource::Systemd {
                scope: InstallScope::User
            }
        );
    }

    #[test]
    fn details_reports_a_unit_that_is_gone_as_none() {
        let missing = FakeRunner::new().on_exit(
            "systemctl show -p FragmentPath,ActiveState,UnitFileState,Description nope.service",
            1,
            "",
            "Failed to get properties: Unit nope.service not found.\n",
        );
        assert!(
            adapter(missing)
                .details("system:nope.service")
                .expect("details")
                .is_none()
        );

        // systemd also answers an unknown unit with exit 0 and empty properties.
        let empty = FakeRunner::new().on(
            "systemctl show -p FragmentPath,ActiveState,UnitFileState,Description gone.service",
            "Description=gone.service\nActiveState=inactive\nFragmentPath=\nUnitFileState=\n",
        );
        assert!(
            adapter(empty)
                .details("system:gone.service")
                .expect("details")
                .is_none()
        );
    }

    #[test]
    fn an_identity_from_another_adapter_or_a_hostile_name_is_refused() {
        let adapter = adapter(FakeRunner::new());
        for hostile in [
            "voxtype.service",
            "user:--now",
            "user:voxtype service",
            "root:voxtype.service",
            "",
        ] {
            assert!(
                matches!(
                    adapter.details(hostile).expect_err("must be refused"),
                    AdapterError::InvalidId { .. }
                ),
                "{hostile:?} must be refused"
            );
        }
    }

    #[test]
    fn nothing_can_be_stopped_disabled_or_removed_here() {
        let components = adapter(runner()).list().expect("list");
        let component = find(&components, "user:pipewire.service");
        let adapter = adapter(FakeRunner::new());

        for kind in [OperationKind::Remove, OperationKind::Update] {
            assert!(!adapter.supports(component, kind));
        }
        let request = OperationRequest::new(component.id.clone(), OperationKind::Remove);
        assert_eq!(
            adapter
                .plan_operation(&request, component)
                .expect_err("no plan"),
            AdapterError::unsupported(ADAPTER_ID, "remove")
        );
    }

    #[test]
    fn a_transport_failure_is_an_error_rather_than_an_empty_inventory() {
        let runner = FakeRunner::new().on_error(
            SYSTEM_UNITS_CMD,
            ProcessError::Timeout {
                program: "systemctl".to_string(),
                seconds: 20,
            },
        );
        // Both scopes fail: there is nothing honest to return, so it is an error.
        assert!(adapter(runner).list().is_err());
    }
}
