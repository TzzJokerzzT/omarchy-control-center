//! Hyprland configuration inspection.
//!
//! This adapter runs **no command**: it reads the files that make up the running
//! configuration and reports what they reference. Hyprland on Omarchy 4.5 configures
//! itself through Lua modules (`require("hypr.bindings")`) and starts programs through
//! helpers (`o.launch_on_start("...")`); older layouts used `.conf` with `exec-once`.
//! Both shapes are read, because both can be present while a user migrates.
//!
//! The rule that shapes this module: a configuration line is a *reference*, never a
//! package. Nothing here claims ownership of a file, and nothing here is removable —
//! deleting configuration because a line mentions a name is exactly the inference the
//! specification forbids.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::domain::{
    AdapterAvailability, AdapterError, AdapterStatus, Component, ComponentAdapter,
    ComponentCapability, ComponentCategory, ComponentId, ComponentSource, ComponentStatus,
    DetailField, DetectionConfidence, DetectionMethod, OperationKind, OperationOutcome,
    OperationPlan, OperationRequest,
};
use crate::infrastructure::process::CommandRunner;
use crate::infrastructure::reclaim::Reclaimer;

const ADAPTER_ID: &str = "hyprland";

/// File names that mark a Hyprland configuration as a leftover rather than the
/// active configuration. Omarchy's own upgrade and backup tools produce all three.
const BACKUP_MARKERS: &[&str] = &[".bak.", ".lacquer-backup-", ".omarchy-upgrade-to-"];

pub struct HyprlandAdapter {
    runner: Arc<dyn CommandRunner>,
    reclaimer: Arc<dyn Reclaimer>,
    config_dir: PathBuf,
}

impl HyprlandAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>, reclaimer: Arc<dyn Reclaimer>) -> Self {
        let config_dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".config/hypr");
        Self {
            runner,
            reclaimer,
            config_dir,
        }
    }

    #[cfg(test)]
    pub fn with_config_dir(
        runner: Arc<dyn CommandRunner>,
        reclaimer: Arc<dyn Reclaimer>,
        config_dir: PathBuf,
    ) -> Self {
        Self {
            runner,
            reclaimer,
            config_dir,
        }
    }

    fn config_files(&self) -> Vec<(String, PathBuf)> {
        let Ok(entries) = std::fs::read_dir(&self.config_dir) else {
            return Vec::new();
        };
        let mut files: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                // A backup copy loses its extension (`input.lua.bak.1786…`), so the
                // markers decide, not the extension.
                let is_config = matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("lua") | Some("conf")
                ) || is_backup(&name);
                (path.is_file() && is_config).then_some((name, path))
            })
            .collect();
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files
    }

    fn module_component(&self, name: &str, path: &Path) -> Component {
        let lines = std::fs::read_to_string(path)
            .map(|text| text.lines().count())
            .unwrap_or(0);
        Component {
            id: ComponentId::new(ADAPTER_ID, name.to_string()),
            name: name.to_string(),
            version: None,
            description: Some("part of the active Hyprland configuration".to_string()),
            category: ComponentCategory::Configuration,
            source: ComponentSource::Hyprland {
                reference_kind: "config module".to_string(),
            },
            status: ComponentStatus::Installed,
            confidence: DetectionConfidence::Authoritative,
            capabilities: vec![ComponentCapability::Inspect],
            detection: DetectionMethod::scan(&self.config_dir)
                .with_note("read from the Hyprland configuration directory"),
            installed_size: None,
            location: Some(path.to_path_buf()),
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: vec![
                DetailField::new("Lines", lines.to_string()),
                DetailField::new(
                    "Kind",
                    if path.extension().and_then(|value| value.to_str()) == Some("lua") {
                        "lua module"
                    } else {
                        "legacy conf"
                    },
                ),
            ],
        }
    }

    fn backup_component(&self, name: &str, path: &Path) -> Component {
        Component {
            id: ComponentId::new(ADAPTER_ID, name.to_string()),
            name: name.to_string(),
            version: None,
            description: Some(
                "leftover copy of a Hyprland configuration file; not read by Hyprland".to_string(),
            ),
            category: ComponentCategory::Configuration,
            source: ComponentSource::Hyprland {
                reference_kind: "backup file".to_string(),
            },
            status: ComponentStatus::Orphan,
            confidence: DetectionConfidence::Low,
            // A backup is a leftover nobody reads, so it can be moved into quarantine.
            // A live configuration file never can, and nothing is deleted.
            capabilities: vec![ComponentCapability::Inspect, ComponentCapability::Reclaim],
            detection: DetectionMethod::scan(&self.config_dir),
            installed_size: None,
            location: Some(path.to_path_buf()),
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: vec![DetailField::new("Kind", "backup")],
        }
    }

    /// One component per program the configuration starts. The name is the command as
    /// written, and the program it resolves to is reported as a fact instead of an
    /// assumption.
    fn launch_component(&self, reference: &LaunchReference, path: &Path) -> Component {
        let program = reference.program();
        let resolved = self.resolves(&program);
        Component {
            id: ComponentId::new(ADAPTER_ID, format!("{}:{}", reference.file, reference.line)),
            name: program.clone(),
            version: None,
            description: Some(format!(
                "started by the configuration: {}",
                reference.command
            )),
            category: ComponentCategory::Configuration,
            source: ComponentSource::Hyprland {
                reference_kind: "launch target".to_string(),
            },
            status: ComponentStatus::Installed,
            confidence: DetectionConfidence::Medium,
            capabilities: vec![ComponentCapability::Inspect],
            detection: DetectionMethod::scan(&self.config_dir)
                .with_note("reference found in the Hyprland configuration"),
            installed_size: None,
            location: Some(path.to_path_buf()),
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: vec![
                DetailField::new(
                    "Declared in",
                    format!("{}:{}", reference.file, reference.line),
                ),
                DetailField::new("Command", reference.command.clone()),
                DetailField::new("Program resolves", resolved),
            ],
        }
    }

    /// Whether the first token of a launch command names something that exists. A
    /// reference that does not resolve is reported as such, never silently dropped.
    fn resolves(&self, program: &str) -> String {
        if program.is_empty() {
            return "no: empty command".to_string();
        }
        if program.starts_with('/') || program.starts_with('~') {
            let expanded = program.replace('~', &std::env::var("HOME").unwrap_or_default());
            return if Path::new(&expanded).exists() {
                "yes: absolute path".to_string()
            } else {
                "no: path does not exist".to_string()
            };
        }
        match self.runner.resolve(program) {
            Some(path) => format!("yes: {}", path.display()),
            None => "no: not found in the executable search path".to_string(),
        }
    }

    fn components(&self) -> Vec<Component> {
        let mut components = Vec::new();
        for (name, path) in self.config_files() {
            if is_backup(&name) {
                components.push(self.backup_component(&name, &path));
                continue;
            }
            components.push(self.module_component(&name, &path));

            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let is_lua = path.extension().and_then(|extension| extension.to_str()) == Some("lua");
            for reference in launch_references(&name, &text, is_lua) {
                components.push(self.launch_component(&reference, &path));
            }
        }
        components
    }
}

/// A launch reference found in a configuration file, with its exact position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchReference {
    pub file: String,
    pub line: usize,
    pub command: String,
}

impl LaunchReference {
    /// The program a command names: its first token.
    pub fn program(&self) -> String {
        self.command
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

fn is_backup(name: &str) -> bool {
    BACKUP_MARKERS.iter().any(|marker| name.contains(marker))
}

/// Extracts the programs a configuration file starts. Commented lines are ignored:
/// they would otherwise report a service the user removed years ago.
pub fn launch_references(file: &str, text: &str, is_lua: bool) -> Vec<LaunchReference> {
    let mut references = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = strip_comment(raw, is_lua);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(command) = quoted_argument(line, "launch_on_start(")
            .or_else(|| quoted_argument(line, "exec_on_start("))
        {
            references.push(LaunchReference {
                file: file.to_string(),
                line: index + 1,
                command,
            });
            continue;
        }

        // Legacy layout: `exec-once = command`.
        if let Some(rest) = line.strip_prefix("exec-once") {
            let command = rest.trim_start().trim_start_matches('=').trim().to_string();
            if !command.is_empty() {
                references.push(LaunchReference {
                    file: file.to_string(),
                    line: index + 1,
                    command,
                });
            }
        }
    }
    references
}

/// The first string literal inside `call(`, if the line contains that call.
fn quoted_argument(line: &str, call: &str) -> Option<String> {
    let start = line.find(call)? + call.len();
    let rest = &line[start..];
    let open = rest.find('"')? + 1;
    let tail = &rest[open..];
    let close = tail.find('"')?;
    let command = tail[..close].trim().to_string();
    (!command.is_empty()).then_some(command)
}

/// Removes a trailing comment without being fooled by string literals: the real
/// configuration starts `"/usr/bin/fix-ultrawide-mode --now"`, where `--` is an
/// argument and not a comment.
fn strip_comment(line: &str, is_lua: bool) -> &str {
    let bytes = line.as_bytes();
    let mut in_double = false;
    let mut in_single = false;
    let mut index = 0;
    while index < bytes.len() {
        let character = bytes[index] as char;
        if in_double || in_single {
            if character == '\\' {
                index += 2;
                continue;
            }
            if in_double && character == '"' {
                in_double = false;
            } else if in_single && character == '\'' {
                in_single = false;
            }
            index += 1;
            continue;
        }
        match character {
            '"' => in_double = true,
            '\'' => in_single = true,
            // Lua: `--` starts a comment. `#` is not a comment in Lua.
            '-' if is_lua && bytes.get(index + 1) == Some(&b'-') => return &line[..index],
            '#' if !is_lua => return &line[..index],
            _ => {}
        }
        index += 1;
    }
    line
}

impl ComponentAdapter for HyprlandAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn display_name(&self) -> &'static str {
        "Hyprland configuration"
    }

    fn description(&self) -> &'static str {
        "Modules, launch targets and leftover copies found in the Hyprland configuration"
    }

    fn capabilities(&self) -> Vec<ComponentCapability> {
        vec![ComponentCapability::Inspect]
    }

    fn status(&self) -> AdapterStatus {
        let mut status = AdapterStatus {
            id: ADAPTER_ID.to_string(),
            display_name: self.display_name().to_string(),
            description: self.description().to_string(),
            availability: AdapterAvailability::Available,
            tool: None,
            tool_version: None,
            capabilities: self.capabilities(),
            commands: Vec::new(),
            notes: vec![
                "reads configuration files only; it runs no command".to_string(),
                "a reference is not ownership: nothing here is removable".to_string(),
            ],
        };

        if !self.config_dir.is_dir() {
            status.availability = AdapterAvailability::Unavailable {
                reason: format!("{} does not exist", self.config_dir.display()),
            };
        } else {
            status
                .notes
                .push(format!("configuration: {}", self.config_dir.display()));
        }
        status
    }

    fn list(&self) -> Result<Vec<Component>, AdapterError> {
        let mut components = self.components();
        components.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
        Ok(components)
    }

    fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError> {
        Ok(self
            .components()
            .into_iter()
            .find(|component| component.id.native_id == native_id))
    }

    /// Only a backup copy may be reclaimed: a live configuration file is never touched,
    /// and a reference is not an object at all.
    fn supports(&self, component: &Component, kind: OperationKind) -> bool {
        kind == OperationKind::Reclaim
            && component.status == ComponentStatus::Orphan
            && component.supports(ComponentCapability::Reclaim)
            && component
                .location
                .as_ref()
                .is_some_and(|path| path.parent() == Some(self.config_dir.as_path()))
    }

    fn plan_operation(
        &self,
        request: &OperationRequest,
        component: &Component,
    ) -> Result<OperationPlan, AdapterError> {
        if request.kind != OperationKind::Reclaim {
            return Err(AdapterError::unsupported(
                ADAPTER_ID,
                "operations (Hyprland configuration is inspected, never modified)",
            ));
        }

        let Some(location) = component.location.clone() else {
            return Err(AdapterError::Other(format!(
                "{} has no file recorded",
                component.name
            )));
        };
        if location.parent() != Some(self.config_dir.as_path()) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: location.display().to_string(),
                reason: "the file is not a direct child of the configuration directory".to_string(),
            });
        }
        if !location.exists() {
            return Err(AdapterError::command(
                "quarantine",
                location.display().to_string(),
                "the backup file is no longer there".to_string(),
            ));
        }

        let size = crate::domain::path_size(&location);
        Ok(OperationPlan {
            adapter: ADAPTER_ID.to_string(),
            target: request.target.clone(),
            target_name: component.name.clone(),
            target_version: None,
            kind: OperationKind::Reclaim,
            commands: Vec::new(),
            reclaim: vec![crate::domain::ReclaimTarget::quarantine(
                location.clone(),
                self.config_dir.clone(),
                size,
            )],
            affected: vec![location.display().to_string()],
            blocking: Vec::new(),
            unknown: vec![
                "whether any script reads this backup is unknown; Hyprland itself does not"
                    .to_string(),
            ],
            notes: vec![
                "the file is moved into the quarantine area, not deleted, and can be restored"
                    .to_string(),
                "the space is freed only when the quarantine entry is purged".to_string(),
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
        let stamp = crate::infrastructure::reclaim::now_unix();
        let mut moved = 0usize;
        let mut freed = 0u64;
        let mut error_text = String::new();

        for target in &plan.reclaim {
            match self.reclaimer.quarantine(target, stamp) {
                Ok(entry) => {
                    moved += 1;
                    freed += entry.size;
                }
                Err(error) => {
                    error_text.push_str(&format!("{}: {error}\n", target.path.display()));
                    break;
                }
            }
        }

        let success = error_text.is_empty() && moved == plan.reclaim.len();
        Ok(OperationOutcome {
            success,
            exit_code: None,
            command_display: format!("quarantine {} file(s)", plan.reclaim.len()),
            stdout_tail: String::new(),
            stderr_tail: error_text,
            summary: if success {
                format!(
                    "moved {moved} backup(s) totalling {} into quarantine",
                    crate::domain::format_size(freed)
                )
            } else {
                format!(
                    "moved {moved} of {} item(s) before stopping",
                    plan.reclaim.len()
                )
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::test_support::FakeRunner;

    struct Sandbox {
        dir: PathBuf,
    }

    impl Sandbox {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("occ-hypr-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("dir");
            std::fs::write(
                dir.join("hyprland.lua"),
                "require(\"hypr.monitors\")\nrequire(\"hypr.bindings\")\n",
            )
            .expect("hyprland.lua");
            std::fs::write(
                dir.join("autostart.lua"),
                "-- Extra autostart processes.\n-- o.launch_on_start(\"my-service\")\n\
                 o.launch_on_start(\"present-tool --flag\")\no.launch_on_start(\"missing-tool\")\n",
            )
            .expect("autostart.lua");
            std::fs::write(
                dir.join("legacy.conf"),
                "# exec-once = commented-out\nexec-once = legacy-daemon --now\n",
            )
            .expect("legacy.conf");
            std::fs::write(dir.join("input.lua.bak.1786584854"), "old\n").expect("backup");
            std::fs::write(dir.join("notes.txt"), "not a config\n").expect("notes");
            Self { dir }
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn adapter(dir: &Path) -> HyprlandAdapter {
        let runner = FakeRunner::new().with_program("present-tool");
        let reclaimer = std::sync::Arc::new(crate::infrastructure::reclaim::FileReclaimer::new(
            dir.join(".quarantine-state"),
        ));
        HyprlandAdapter::with_config_dir(Arc::new(runner), reclaimer, dir.to_path_buf())
    }

    #[test]
    fn lists_config_modules_and_ignores_unrelated_files() {
        let sandbox = Sandbox::new("modules");
        let components = adapter(&sandbox.dir).list().expect("list");

        let names: Vec<&str> = components
            .iter()
            .map(|component| component.name.as_str())
            .collect();
        assert!(names.contains(&"hyprland.lua"));
        assert!(names.contains(&"autostart.lua"));
        assert!(names.contains(&"legacy.conf"));
        assert!(
            !names.contains(&"notes.txt"),
            "a non-config file is not a module"
        );
    }

    #[test]
    fn a_configuration_line_is_a_reference_and_never_removable() {
        let sandbox = Sandbox::new("references");
        let adapter = adapter(&sandbox.dir);
        let listed = adapter.list().expect("list");

        // Nothing in a Hyprland configuration is removable: there is no package behind a
        // line. Only a leftover backup can be reclaimed, and that is not a removal.
        assert!(
            listed
                .iter()
                .all(|component| !component.supports(ComponentCapability::Remove))
        );
        assert!(
            listed
                .iter()
                .all(|component| !adapter.supports(component, OperationKind::Remove))
        );
        for component in &listed {
            if component.status == ComponentStatus::Orphan {
                assert!(component.supports(ComponentCapability::Reclaim));
            } else {
                assert_eq!(
                    component.capabilities,
                    vec![ComponentCapability::Inspect],
                    "{} must not offer any action",
                    component.name
                );
            }
        }
    }
    #[test]
    fn extracts_uncommented_launch_targets_from_lua_and_conf() {
        let sandbox = Sandbox::new("launch");
        let components = adapter(&sandbox.dir).list().expect("list");
        let launch: Vec<&Component> = components
            .iter()
            .filter(|component| matches!(component.source, ComponentSource::Hyprland { .. }))
            .filter(|component| component.extra.iter().any(|field| field.label == "Command"))
            .collect();

        let commands: Vec<String> = launch
            .iter()
            .map(|component| {
                component
                    .extra
                    .iter()
                    .find(|field| field.label == "Command")
                    .map(|field| field.value.clone())
                    .unwrap_or_default()
            })
            .collect();

        assert!(
            commands
                .iter()
                .any(|command| command.starts_with("present-tool"))
        );
        assert!(
            commands
                .iter()
                .any(|command| command.starts_with("missing-tool"))
        );
        assert!(
            commands
                .iter()
                .any(|command| command.starts_with("legacy-daemon"))
        );
        assert!(
            !commands
                .iter()
                .any(|command| command.contains("my-service")),
            "a commented launch target must not be reported: {commands:?}"
        );
        assert!(
            !commands
                .iter()
                .any(|command| command.contains("commented-out")),
            "a commented exec-once must not be reported: {commands:?}"
        );
    }

    #[test]
    fn reports_whether_each_launch_target_resolves() {
        let sandbox = Sandbox::new("resolve");
        let components = adapter(&sandbox.dir).list().expect("list");
        let resolves = |program: &str| {
            components
                .iter()
                .filter(|component| component.name == program)
                .filter_map(|component| {
                    component
                        .extra
                        .iter()
                        .find(|field| field.label == "Program resolves")
                        .map(|field| field.value.clone())
                })
                .next()
                .unwrap_or_default()
        };
        assert!(resolves("present-tool").starts_with("yes"));
        assert!(resolves("missing-tool").starts_with("no"));
    }

    #[test]
    fn backup_copies_are_orphans_and_not_read_as_configuration() {
        let sandbox = Sandbox::new("backup");
        let components = adapter(&sandbox.dir).list().expect("list");
        let backup = components
            .iter()
            .find(|component| component.id.native_id == "input.lua.bak.1786584854")
            .expect("backup");

        assert_eq!(backup.status, ComponentStatus::Orphan);
        assert_eq!(backup.confidence, DetectionConfidence::Low);
        assert!(!components.iter().any(|component| {
            component.name == "input.lua.bak.1786584854"
                && matches!(component.source, ComponentSource::Hyprland { ref reference_kind } if reference_kind == "config module")
        }));
    }

    #[test]
    fn reports_itself_unavailable_when_there_is_no_hyprland_directory() {
        let missing = std::env::temp_dir().join(format!("occ-hypr-absent-{}", std::process::id()));
        let runner = FakeRunner::new();
        let reclaimer = std::sync::Arc::new(crate::infrastructure::reclaim::FileReclaimer::new(
            std::env::temp_dir().join("occ-hypr-quarantine"),
        ));
        let adapter = HyprlandAdapter::with_config_dir(Arc::new(runner), reclaimer, missing);
        assert!(!adapter.status().available());
        assert!(adapter.list().expect("list").is_empty());
    }

    #[test]
    fn a_backup_can_be_reclaimed_but_a_live_module_cannot() {
        let sandbox = Sandbox::new("reclaim");
        let adapter = adapter(&sandbox.dir);
        let components = adapter.list().expect("list");

        let backup = components
            .iter()
            .find(|component| component.status == ComponentStatus::Orphan)
            .expect("backup")
            .clone();
        assert!(adapter.supports(&backup, OperationKind::Reclaim));
        let request = OperationRequest::new(backup.id.clone(), OperationKind::Reclaim);
        let plan = adapter.plan_operation(&request, &backup).expect("plan");
        assert_eq!(plan.kind, OperationKind::Reclaim);
        assert_eq!(plan.reclaim.len(), 1);
        assert!(plan.commands.is_empty());
        assert!(plan.is_executable());
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("moved into the quarantine area"))
        );

        let live = components
            .iter()
            .find(|component| component.name == "hyprland.lua")
            .expect("live module")
            .clone();
        assert!(!adapter.supports(&live, OperationKind::Reclaim));
    }

    #[test]
    fn reclaiming_a_backup_moves_it_out_of_the_configuration_directory() {
        let sandbox = Sandbox::new("reclaim-run");
        let adapter = adapter(&sandbox.dir);
        let backup = adapter
            .list()
            .expect("list")
            .into_iter()
            .find(|component| component.status == ComponentStatus::Orphan)
            .expect("backup");
        let path = backup.location.clone().expect("path");
        let request = OperationRequest::new(backup.id.clone(), OperationKind::Reclaim);
        let plan = adapter.plan_operation(&request, &backup).expect("plan");

        let outcome = adapter.execute(&plan).expect("execute");
        assert!(outcome.success, "unexpected: {}", outcome.stderr_tail);
        assert!(!path.exists(), "it left the configuration directory");
        assert!(outcome.summary.contains("quarantine"));
    }

    #[test]
    fn operations_are_refused_explicitly() {
        let sandbox = Sandbox::new("ops");
        let adapter = adapter(&sandbox.dir);
        let component = adapter
            .list()
            .expect("list")
            .into_iter()
            .next()
            .expect("one");
        let request = OperationRequest::new(component.id.clone(), OperationKind::Remove);
        assert!(adapter.plan_operation(&request, &component).is_err());
    }
}
