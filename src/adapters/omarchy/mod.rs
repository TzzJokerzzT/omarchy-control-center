//! Omarchy shell plugins.
//!
//! Commands used, and why:
//! * `omarchy plugin list --json` — Omarchy's own catalogue walker, returning id,
//!   name, kinds, enabled state, first-party flag and clone origin for every plugin
//!   it discovers. We read the manager's answer instead of scanning directories
//!   ourselves, so the inventory cannot disagree with `omarchy plugin list`.
//! * `omarchy plugin remove <id> --yes` — removal, delegated to the manager that
//!   owns the plugin directory and the shell state around it.
//!
//! `--yes` only suppresses the manager's own prompt: the confirmation that matters
//! has already happened in the interface, with the exact command on screen.
//!
//! Two facts are read from disk, because the catalogue does not report them:
//! whether a plugin directory is a git clone (which decides whether the manager
//! deletes it or moves it aside), and leftover `.bak` directories from earlier
//! removals, which the catalogue hides and nothing ever reconciles.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::domain::{
    AdapterAvailability, AdapterError, AdapterStatus, Component, ComponentAdapter,
    ComponentCapability, ComponentCategory, ComponentId, ComponentSource, ComponentStatus,
    DetailField, DetectionConfidence, DetectionMethod, OperationKind, OperationOutcome,
    OperationPlan, OperationRequest, PlannedCommand, tail_text,
};
use crate::infrastructure::process::{CommandOutput, CommandRunner, CommandSpec, ProcessError};
use crate::infrastructure::reclaim::Reclaimer;
use crate::infrastructure::validate::validate_plugin_id;

const ADAPTER_ID: &str = "omarchy";
const LIST_TIMEOUT: Duration = Duration::from_secs(20);

const COMMANDS: &[&str] = &[
    "omarchy plugin list --json",
    "omarchy plugin remove <id> --yes",
];

/// One row of `omarchy plugin list --json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginEntry {
    id: String,
    name: String,
    #[serde(default)]
    kinds: Vec<String>,
    enabled: bool,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    can_disable: bool,
    #[serde(default)]
    first_party: bool,
    #[serde(default)]
    cloned_from: String,
}

/// The parts of `manifest.json` the detail view shows. Everything is optional: a
/// manifest that omits a field leaves it `No disponible` instead of filling it in.
#[derive(Debug, Default, Deserialize)]
struct Manifest {
    version: Option<String>,
    author: Option<String>,
    license: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    #[serde(default)]
    kinds: Vec<String>,
}

pub struct OmarchyPluginAdapter {
    runner: Arc<dyn CommandRunner>,
    reclaimer: Arc<dyn Reclaimer>,
    plugins_dir: PathBuf,
}

impl OmarchyPluginAdapter {
    /// Uses the real user plugin directory.
    pub fn new(runner: Arc<dyn CommandRunner>, reclaimer: Arc<dyn Reclaimer>) -> Self {
        let plugins_dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".config/omarchy/plugins");
        Self {
            runner,
            reclaimer,
            plugins_dir,
        }
    }

    /// Points the adapter at another plugin directory.
    #[cfg(test)]
    pub fn with_plugins_dir(
        runner: Arc<dyn CommandRunner>,
        reclaimer: Arc<dyn Reclaimer>,
        plugins_dir: PathBuf,
    ) -> Self {
        Self {
            runner,
            reclaimer,
            plugins_dir,
        }
    }

    fn run(&self, args: &[&str], context: &str) -> Result<CommandOutput, AdapterError> {
        let spec = CommandSpec::capture(
            "omarchy",
            args.iter().map(|arg| (*arg).to_string()).collect(),
        )
        .with_timeout(LIST_TIMEOUT);
        self.runner.run(&spec).map_err(|error| match error {
            ProcessError::NotFound { .. } => AdapterError::ToolUnavailable {
                tool: "omarchy".to_string(),
                reason: "the omarchy command was not found in the executable search path"
                    .to_string(),
            },
            other => AdapterError::command(context, spec.display(), other.to_string()),
        })
    }

    /// Asks Omarchy for its catalogue. A parse failure is an error: returning an
    /// empty list would look like "no plugins installed", which would be a lie.
    fn catalogue(&self) -> Result<Vec<PluginEntry>, AdapterError> {
        let output = self.run(&["plugin", "list", "--json"], "omarchy plugin list --json")?;
        if !output.success() {
            return Err(AdapterError::command(
                "omarchy plugin list --json",
                "omarchy plugin list --json",
                format!("exit {:?}: {}", output.exit_code, output.stderr.trim()),
            ));
        }
        serde_json::from_str::<Vec<PluginEntry>>(&output.stdout).map_err(|error| {
            AdapterError::Parse {
                context: "omarchy plugin list --json".to_string(),
                detail: error.to_string(),
            }
        })
    }

    fn plugin_dir(&self, id: &str) -> PathBuf {
        self.plugins_dir.join(id)
    }

    /// A user plugin directory the manager can act on. First-party plugins live
    /// inside the `omarchy` package and have no directory here.
    fn has_user_directory(&self, id: &str) -> bool {
        self.plugin_dir(id).is_dir()
    }

    fn is_git_clone(&self, id: &str) -> bool {
        self.plugin_dir(id).join(".git").exists()
    }

    fn manifest(&self, id: &str) -> Option<Manifest> {
        let path = self.plugin_dir(id).join("manifest.json");
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<Manifest>(&text).ok()
    }

    fn capabilities_for(&self, entry: &PluginEntry) -> Vec<ComponentCapability> {
        let removable = !entry.first_party && self.has_user_directory(&entry.id);
        if removable {
            vec![ComponentCapability::Inspect, ComponentCapability::Remove]
        } else {
            vec![ComponentCapability::Inspect]
        }
    }

    fn status_for(entry: &PluginEntry) -> ComponentStatus {
        if entry.enabled {
            ComponentStatus::Enabled
        } else {
            ComponentStatus::Disabled
        }
    }

    fn component(&self, entry: &PluginEntry) -> Component {
        let manifest = self.manifest(&entry.id);
        let mut extra = vec![
            DetailField::new("Kind", entry.kinds.join(", ")),
            DetailField::new(
                "Source",
                if entry.first_party {
                    "first-party"
                } else {
                    "third-party"
                },
            ),
            DetailField::new("Enabled", if entry.enabled { "yes" } else { "no" }),
            DetailField::new("Active now", if entry.active { "yes" } else { "no" }),
            DetailField::new(
                "Can be disabled",
                if entry.can_disable { "yes" } else { "no" },
            ),
        ];
        if !entry.cloned_from.is_empty() {
            extra.push(DetailField::new("Cloned from", entry.cloned_from.clone()));
        }
        if let Some(manifest) = &manifest {
            if let Some(author) = &manifest.author {
                extra.push(DetailField::new("Author", author));
            }
            if let Some(license) = &manifest.license {
                extra.push(DetailField::new("License", license));
            }
            if let Some(homepage) = &manifest.homepage {
                extra.push(DetailField::new("Homepage", homepage));
            }
            if !manifest.kinds.is_empty() {
                extra.push(DetailField::new(
                    "Manifest kinds",
                    manifest.kinds.join(", "),
                ));
            }
        }
        if !entry.first_party {
            extra.push(DetailField::new(
                "Managed as",
                if self.is_git_clone(&entry.id) {
                    "git clone"
                } else {
                    "plain directory"
                },
            ));
        }

        let location = if entry.first_party {
            None
        } else {
            Some(self.plugin_dir(&entry.id))
        };

        Component {
            id: ComponentId::new(ADAPTER_ID, entry.id.clone()),
            name: entry.name.clone(),
            version: manifest.and_then(|manifest| manifest.version),
            description: self
                .manifest(&entry.id)
                .and_then(|manifest| manifest.description),
            category: ComponentCategory::Plugin,
            source: ComponentSource::OmarchyPlugin {
                first_party: entry.first_party,
            },
            status: Self::status_for(entry),
            confidence: DetectionConfidence::Authoritative,
            capabilities: self.capabilities_for(entry),
            detection: DetectionMethod::command(
                "omarchy",
                vec![
                    "plugin".to_string(),
                    "list".to_string(),
                    "--json".to_string(),
                ],
            )
            .with_note("reported by Omarchy's own plugin catalogue"),
            installed_size: None,
            location,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra,
        }
    }

    /// `.bak` directories are what `omarchy plugin remove` leaves behind when a
    /// plugin is not a git clone. The catalogue hides them (`.*` is excluded), so
    /// nothing else in the system reports them: they are genuine orphans, shown for
    /// inspection only because deleting them by heuristic is exactly what this
    /// program refuses to do.
    fn orphan_components(&self, known: &[PluginEntry]) -> Vec<Component> {
        let Ok(entries) = std::fs::read_dir(&self.plugins_dir) else {
            return Vec::new();
        };

        let mut orphans = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix('.') else {
                continue;
            };
            let Some((id, _stamp)) = rest.split_once(".bak.") else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            let path = entry.path();
            let still_installed = known.iter().any(|plugin| plugin.id == id);
            orphans.push(Component {
                id: ComponentId::new(ADAPTER_ID, name.clone()),
                name: id.to_string(),
                version: None,
                description: Some(
                    "leftover of an earlier plugin removal: Omarchy moved the directory aside \
                     instead of deleting it"
                        .to_string(),
                ),
                category: ComponentCategory::Plugin,
                source: ComponentSource::OmarchyPlugin { first_party: false },
                status: ComponentStatus::Orphan,
                confidence: DetectionConfidence::Low,
                // No manager owns these. They can be moved into quarantine, which keeps
                // a way back, and that is the only action offered.
                capabilities: vec![ComponentCapability::Inspect, ComponentCapability::Reclaim],
                detection: DetectionMethod::scan(&self.plugins_dir).with_note(
                    "not in the Omarchy catalogue; found as a hidden leftover directory",
                ),
                installed_size: None,
                location: Some(path),
                installed_at: None,
                depends_on: Vec::new(),
                required_by: Vec::new(),
                extra: vec![
                    DetailField::new("Kind", "removal backup"),
                    DetailField::new(
                        "Same plugin installed",
                        if still_installed { "yes" } else { "no" },
                    ),
                ],
            });
        }
        orphans.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
        orphans
    }
}

impl OmarchyPluginAdapter {
    /// A leftover has no manager: the plan states exactly which directory moves and how
    /// much space that takes out of the plugin root.
    fn plan_reclaim(
        &self,
        request: &OperationRequest,
        component: &Component,
    ) -> Result<OperationPlan, AdapterError> {
        let Some(location) = component.location.clone() else {
            return Err(AdapterError::Other(format!(
                "{} has no directory recorded, so nothing can be moved",
                component.name
            )));
        };
        if location.parent() != Some(self.plugins_dir.as_path()) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: location.display().to_string(),
                reason: "the directory is not a direct child of the plugin root".to_string(),
            });
        }
        if !location.exists() {
            return Err(AdapterError::command(
                "quarantine",
                location.display().to_string(),
                "the leftover is no longer there".to_string(),
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
                self.plugins_dir.clone(),
                size,
            )],
            affected: vec![location.display().to_string()],
            blocking: Vec::new(),
            unknown: vec![
                "whether anything still refers to this backup is unknown: the plugin catalogue \
                 hides hidden directories"
                    .to_string(),
            ],
            notes: vec![
                "the directory is moved into the quarantine area, not deleted, and can be \
                 restored from there"
                    .to_string(),
                "the space is freed only when the quarantine entry is purged, which is a \
                 separate action"
                    .to_string(),
            ],
        })
    }

    fn execute_reclaim(&self, plan: &OperationPlan) -> Result<OperationOutcome, AdapterError> {
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
            command_display: format!("quarantine {} item(s)", plan.reclaim.len()),
            stdout_tail: String::new(),
            stderr_tail: error_text,
            summary: if success {
                format!(
                    "moved {moved} leftover(s) totalling {} into quarantine",
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

impl ComponentAdapter for OmarchyPluginAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn display_name(&self) -> &'static str {
        "Omarchy plugins"
    }

    fn description(&self) -> &'static str {
        "Omarchy shell plugins and bar widgets: first-party and installed from git"
    }

    fn capabilities(&self) -> Vec<ComponentCapability> {
        vec![ComponentCapability::Inspect, ComponentCapability::Remove]
    }

    fn status(&self) -> AdapterStatus {
        let Some(tool) = self.runner.resolve("omarchy") else {
            return AdapterStatus {
                id: ADAPTER_ID.to_string(),
                display_name: self.display_name().to_string(),
                description: self.description().to_string(),
                availability: AdapterAvailability::Unavailable {
                    reason: "the omarchy command was not found in the executable search path"
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

        AdapterStatus {
            id: ADAPTER_ID.to_string(),
            display_name: self.display_name().to_string(),
            description: self.description().to_string(),
            availability: AdapterAvailability::Available,
            tool: Some(tool),
            // Omarchy exposes no stable `--version` for the plugin subsystem, so the
            // version stays unavailable rather than being invented from a file.
            tool_version: None,
            capabilities: self.capabilities(),
            commands: COMMANDS
                .iter()
                .map(|command| (*command).to_string())
                .collect(),
            notes: vec![format!("user plugins: {}", self.plugins_dir.display())],
        }
    }

    fn list(&self) -> Result<Vec<Component>, AdapterError> {
        let catalogue = self.catalogue()?;
        let mut components: Vec<Component> = catalogue
            .iter()
            .map(|entry| self.component(entry))
            .collect();
        components.extend(self.orphan_components(&catalogue));
        Ok(components)
    }

    fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError> {
        if let Err(reason) = validate_plugin_id(native_id) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: native_id.to_string(),
                reason: reason.to_string(),
            });
        }

        let catalogue = self.catalogue()?;
        match catalogue.iter().find(|entry| entry.id == native_id) {
            Some(entry) => Ok(Some(self.component(entry))),
            None => Ok(self
                .orphan_components(&catalogue)
                .into_iter()
                .find(|component| component.id.native_id == native_id)),
        }
    }

    /// Removal is offered only for third-party plugins with a user directory: a
    /// first-party plugin ships inside the `omarchy` package and would come back on
    /// the next update.
    fn supports(&self, component: &Component, kind: OperationKind) -> bool {
        if kind == OperationKind::Reclaim {
            return component.supports(ComponentCapability::Reclaim)
                && component.status == ComponentStatus::Orphan
                && component
                    .location
                    .as_ref()
                    .is_some_and(|path| path.parent() == Some(self.plugins_dir.as_path()));
        }
        kind == OperationKind::Remove
            && component.supports(ComponentCapability::Remove)
            && matches!(
                component.source,
                ComponentSource::OmarchyPlugin { first_party: false }
            )
            && component.status != ComponentStatus::Orphan
    }

    fn plan_operation(
        &self,
        request: &OperationRequest,
        component: &Component,
    ) -> Result<OperationPlan, AdapterError> {
        if request.kind == OperationKind::Reclaim {
            return self.plan_reclaim(request, component);
        }
        if request.kind != OperationKind::Remove {
            return Err(AdapterError::unsupported(ADAPTER_ID, request.kind.label()));
        }
        let id = &request.target.native_id;
        if let Err(reason) = validate_plugin_id(id) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: id.clone(),
                reason: reason.to_string(),
            });
        }

        // Re-read the catalogue: the plan must describe the shell as it is now.
        let catalogue = self.catalogue()?;
        let Some(entry) = catalogue.iter().find(|entry| &entry.id == id) else {
            return Err(AdapterError::command(
                "omarchy plugin list --json",
                "omarchy plugin list --json",
                format!("{id} is not in the plugin catalogue any more"),
            ));
        };
        if entry.first_party {
            return Err(AdapterError::unsupported(
                ADAPTER_ID,
                "removal of a first-party plugin (it is part of the omarchy package)",
            ));
        }
        if !self.has_user_directory(id) {
            return Err(AdapterError::command(
                "plugin directory",
                self.plugin_dir(id).display().to_string(),
                "the plugin has no directory in the user plugin root".to_string(),
            ));
        }

        let git_managed = self.is_git_clone(id);
        let mut notes = vec![
            if git_managed {
                "this plugin is a git clone: Omarchy deletes its directory, and the removal is \
                 not reversible"
                    .to_string()
            } else {
                "this plugin is not a git clone: Omarchy moves the directory to a hidden \
                 .bak directory instead of deleting it, so it can be recovered by hand"
                    .to_string()
            },
            "the shell keeps its layout and enabled list outside the plugin directory, so a \
             stale reference may remain"
                .to_string(),
        ];
        if !entry.cloned_from.is_empty() {
            notes.push(format!("cloned from {}", entry.cloned_from));
        }

        Ok(OperationPlan {
            adapter: ADAPTER_ID.to_string(),
            target: request.target.clone(),
            target_name: component.name.clone(),
            target_version: component.version.clone(),
            kind: OperationKind::Remove,
            commands: vec![PlannedCommand {
                program: "omarchy".to_string(),
                args: vec![
                    "plugin".to_string(),
                    "remove".to_string(),
                    id.clone(),
                    "--yes".to_string(),
                ],
                // The plugin lives in the user's configuration directory.
                requires_root: false,
                requires_tty: false,
                rationale: "ask Omarchy's own manager to remove the plugin and its directory"
                    .to_string(),
            }],
            reclaim: Vec::new(),
            affected: vec![format!(
                "the plugin directory {}",
                self.plugin_dir(id).display()
            )],
            blocking: Vec::new(),
            unknown: vec![
                "which bar layout entries reference this plugin is not reported by the plugin \
                 catalogue"
                    .to_string(),
                "whether the plugin wrote data outside its own directory is not reported"
                    .to_string(),
            ],
            notes,
        })
    }

    fn execute(&self, plan: &OperationPlan) -> Result<OperationOutcome, AdapterError> {
        if plan.kind == OperationKind::Reclaim {
            return self.execute_reclaim(plan);
        }
        if plan.adapter != ADAPTER_ID {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: plan.adapter.clone(),
                reason: "this plan belongs to another adapter".to_string(),
            });
        }
        if let Err(reason) = validate_plugin_id(&plan.target.native_id) {
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
            if command.program != "omarchy" {
                return Err(AdapterError::InvalidId {
                    adapter: ADAPTER_ID.to_string(),
                    value: command.program.clone(),
                    reason: "this adapter only runs the omarchy command".to_string(),
                });
            }
            command_display = command.display();
            let spec = CommandSpec::capture(command.program.clone(), command.args.clone())
                .with_timeout(LIST_TIMEOUT);
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
                format!("removed plugin {}", plan.target_name)
            } else {
                format!("omarchy did not complete the removal (exit {exit_code:?})")
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const CATALOGUE: &str = include_str!("../../../tests/fixtures/omarchy/plugin-list.json");

    struct Sandbox {
        dir: PathBuf,
    }

    impl Sandbox {
        /// A plugin root shaped like the real one: one git clone, one plain
        /// directory, and one leftover `.bak` directory from an earlier removal.
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("occ-omarchy-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            for plugin in ["io.github.fatihaydost.houra", "denon.touchpad"] {
                let plugin_dir = dir.join(plugin);
                std::fs::create_dir_all(&plugin_dir).expect("plugin dir");
                std::fs::write(
                    plugin_dir.join("manifest.json"),
                    format!(
                        "{{\"schemaVersion\":1,\"id\":\"{plugin}\",\"name\":\"{plugin}\",\
                         \"version\":\"1.2.3\",\"author\":\"Someone\",\"license\":\"MIT\",\
                         \"description\":\"a plugin\",\"homepage\":\"https://example.invalid\",\
                         \"kinds\":[\"bar-widget\"]}}"
                    ),
                )
                .expect("manifest");
            }
            std::fs::create_dir_all(dir.join("io.github.fatihaydost.houra/.git")).expect("git dir");
            std::fs::create_dir_all(dir.join(".denon.touchpad.bak.1786584854")).expect("bak dir");
            Self { dir }
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn reclaimer(dir: &Path) -> Arc<dyn Reclaimer> {
        Arc::new(crate::infrastructure::reclaim::FileReclaimer::new(
            dir.join(".quarantine-state"),
        ))
    }

    fn adapter(runner: FakeRunner, dir: &Path) -> OmarchyPluginAdapter {
        OmarchyPluginAdapter::with_plugins_dir(Arc::new(runner), reclaimer(dir), dir.to_path_buf())
    }

    fn runner() -> FakeRunner {
        FakeRunner::new().on("omarchy plugin list --json", CATALOGUE)
    }

    use crate::infrastructure::test_support::FakeRunner;

    #[test]
    fn reports_itself_unavailable_without_the_omarchy_command() {
        let sandbox = Sandbox::new("unavailable");
        let status = adapter(FakeRunner::new(), &sandbox.dir).status();
        assert!(!status.available());
        assert_eq!(
            status.availability,
            AdapterAvailability::Unavailable {
                reason: "the omarchy command was not found in the executable search path"
                    .to_string()
            }
        );
    }

    #[test]
    fn lists_the_catalogue_with_state_and_ownership() {
        let sandbox = Sandbox::new("list");
        let components = adapter(runner(), &sandbox.dir).list().expect("list");

        let bar = components
            .iter()
            .find(|component| component.id.native_id == "omarchy.bar")
            .expect("bar");
        assert_eq!(bar.name, "Bar");
        assert_eq!(bar.status, ComponentStatus::Enabled);
        assert_eq!(bar.category, ComponentCategory::Plugin);
        assert_eq!(
            bar.source,
            ComponentSource::OmarchyPlugin { first_party: true }
        );
        assert_eq!(bar.capabilities, vec![ComponentCapability::Inspect]);
        assert!(
            bar.extra
                .iter()
                .any(|field| field.label == "Active now" && field.value == "yes")
        );

        let battery = components
            .iter()
            .find(|component| component.id.native_id == "omarchy.battery")
            .expect("battery");
        assert_eq!(battery.status, ComponentStatus::Disabled);
    }

    #[test]
    fn a_third_party_plugin_exposes_its_manifest_and_can_be_removed() {
        let sandbox = Sandbox::new("third-party");
        let components = adapter(runner(), &sandbox.dir).list().expect("list");
        let houra = components
            .iter()
            .find(|component| component.id.native_id == "io.github.fatihaydost.houra")
            .expect("houra");

        assert_eq!(houra.version.as_deref(), Some("1.2.3"));
        assert_eq!(houra.description.as_deref(), Some("a plugin"));
        assert_eq!(
            houra.source,
            ComponentSource::OmarchyPlugin { first_party: false }
        );
        assert_eq!(
            houra.capabilities,
            vec![ComponentCapability::Inspect, ComponentCapability::Remove]
        );
        assert_eq!(
            houra
                .extra
                .iter()
                .find(|field| field.label == "Managed as")
                .map(|field| field.value.as_str()),
            Some("git clone")
        );
        assert!(houra.location.is_some());
    }

    #[test]
    fn a_plain_directory_is_reported_as_managed_by_hand() {
        let sandbox = Sandbox::new("plain");
        let components = adapter(runner(), &sandbox.dir).list().expect("list");
        let touchpad = components
            .iter()
            .find(|component| component.id.native_id == "denon.touchpad")
            .expect("touchpad");
        assert_eq!(
            touchpad
                .extra
                .iter()
                .find(|field| field.label == "Managed as")
                .map(|field| field.value.as_str()),
            Some("plain directory")
        );
    }

    #[test]
    fn hidden_removal_backups_are_orphans_that_can_be_reclaimed_but_never_removed() {
        let sandbox = Sandbox::new("orphan");
        let components = adapter(runner(), &sandbox.dir).list().expect("list");
        let orphan = components
            .iter()
            .find(|component| component.status == ComponentStatus::Orphan)
            .expect("orphan");

        assert!(orphan.id.native_id.starts_with(".denon.touchpad.bak."));
        assert_eq!(orphan.name, "denon.touchpad");
        assert_eq!(
            orphan.capabilities,
            vec![ComponentCapability::Inspect, ComponentCapability::Reclaim]
        );
        assert_eq!(orphan.confidence, DetectionConfidence::Low);
        assert!(orphan.detection.program == "readdir");

        let adapter_instance = adapter(runner(), &sandbox.dir);
        // No manager owns a leftover, so it is never *removed*; it can only be moved into
        // quarantine, which keeps a way back.
        assert!(!adapter_instance.supports(orphan, OperationKind::Remove));
        assert!(adapter_instance.supports(orphan, OperationKind::Reclaim));
    }

    #[test]
    fn the_catalogue_is_never_called_a_list_when_it_fails_to_parse() {
        let sandbox = Sandbox::new("unparseable");
        let broken = FakeRunner::new().on("omarchy plugin list --json", "not json at all");
        let error = adapter(broken, &sandbox.dir).list().expect_err("must fail");
        assert!(
            matches!(error, AdapterError::Parse { .. }),
            "unexpected: {error}"
        );
    }

    #[test]
    fn a_first_party_plugin_cannot_be_removed() {
        let sandbox = Sandbox::new("first-party");
        let components = adapter(runner(), &sandbox.dir).list().expect("list");
        let bar = components
            .iter()
            .find(|component| component.id.native_id == "omarchy.bar")
            .expect("bar");
        assert!(!adapter(runner(), &sandbox.dir).supports(bar, OperationKind::Remove));
    }

    #[test]
    fn a_removal_plan_delegates_to_the_manager_and_states_what_happens_to_the_files() {
        let sandbox = Sandbox::new("plan");
        let components = adapter(runner(), &sandbox.dir).list().expect("list");
        let houra = components
            .iter()
            .find(|component| component.id.native_id == "io.github.fatihaydost.houra")
            .expect("houra")
            .clone();

        let request = OperationRequest::new(houra.id.clone(), OperationKind::Remove);
        let plan = adapter(runner(), &sandbox.dir)
            .plan_operation(&request, &houra)
            .expect("plan");

        assert_eq!(plan.commands.len(), 1);
        assert_eq!(
            plan.commands[0].display(),
            "omarchy plugin remove io.github.fatihaydost.houra --yes"
        );
        assert!(
            !plan.commands[0].requires_root,
            "plugins live in the user's config"
        );
        assert!(plan.is_executable());
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("deletes its directory")),
            "a git clone must be described as irreversible: {:?}",
            plan.notes
        );
        assert!(plan.unknown.iter().any(|fact| fact.contains("bar layout")));
    }

    #[test]
    fn a_plan_for_a_plain_directory_states_that_it_can_be_recovered() {
        let sandbox = Sandbox::new("plan-plain");
        let components = adapter(runner(), &sandbox.dir).list().expect("list");
        let touchpad = components
            .iter()
            .find(|component| component.id.native_id == "denon.touchpad")
            .expect("touchpad")
            .clone();

        let request = OperationRequest::new(touchpad.id.clone(), OperationKind::Remove);
        let plan = adapter(runner(), &sandbox.dir)
            .plan_operation(&request, &touchpad)
            .expect("plan");
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("moves the directory to a hidden"))
        );
    }

    #[test]
    fn a_hostile_plugin_id_is_refused_before_any_process_runs() {
        let sandbox = Sandbox::new("hostile");
        let runner = runner();
        let hostile = OperationRequest::new(
            ComponentId::new("omarchy", "houra; rm -rf /"),
            OperationKind::Remove,
        );
        let error = adapter(runner, &sandbox.dir)
            .plan_operation(&hostile, &hostile_placeholder())
            .expect_err("must refuse");
        assert!(
            matches!(error, AdapterError::InvalidId { .. }),
            "unexpected: {error}"
        );
    }

    #[test]
    fn execution_runs_exactly_the_confirmed_command() {
        let sandbox = Sandbox::new("execute");
        let runner = runner().on_exit(
            "omarchy plugin remove io.github.fatihaydost.houra --yes",
            0,
            "removed\n",
            "",
        );
        let runner = Arc::new(runner);
        let adapter_instance = OmarchyPluginAdapter::with_plugins_dir(
            Arc::clone(&runner) as Arc<dyn CommandRunner>,
            reclaimer(&sandbox.dir),
            sandbox.dir.clone(),
        );

        let components = adapter_instance.list().expect("list");
        let houra = components
            .iter()
            .find(|component| component.id.native_id == "io.github.fatihaydost.houra")
            .expect("houra")
            .clone();
        let request = OperationRequest::new(houra.id.clone(), OperationKind::Remove);
        let plan = adapter_instance
            .plan_operation(&request, &houra)
            .expect("plan");

        let outcome = adapter_instance.execute(&plan).expect("execute");
        assert!(outcome.success);
        assert_eq!(outcome.exit_code, Some(0));
        assert!(
            runner
                .calls()
                .contains(&"omarchy plugin remove io.github.fatihaydost.houra --yes".to_string())
        );
    }

    #[test]
    fn a_failing_removal_is_never_reported_as_success() {
        let sandbox = Sandbox::new("fail");
        let runner = runner().on_exit(
            "omarchy plugin remove io.github.fatihaydost.houra --yes",
            1,
            "",
            "error: plugin not found\n",
        );
        let runner = Arc::new(runner);
        let adapter_instance = OmarchyPluginAdapter::with_plugins_dir(
            Arc::clone(&runner) as Arc<dyn CommandRunner>,
            reclaimer(&sandbox.dir),
            sandbox.dir.clone(),
        );
        let components = adapter_instance.list().expect("list");
        let houra = components
            .iter()
            .find(|component| component.id.native_id == "io.github.fatihaydost.houra")
            .expect("houra")
            .clone();
        let request = OperationRequest::new(houra.id.clone(), OperationKind::Remove);
        let plan = adapter_instance
            .plan_operation(&request, &houra)
            .expect("plan");

        let outcome = adapter_instance.execute(&plan).expect("execute");
        assert!(!outcome.success);
        assert_eq!(outcome.exit_code, Some(1));
        assert!(outcome.stderr_tail.contains("plugin not found"));
    }

    #[test]
    fn a_tampered_plan_cannot_make_this_adapter_run_anything_else() {
        let sandbox = Sandbox::new("tampered");
        let adapter_instance = adapter(runner(), &sandbox.dir);
        let plan = OperationPlan {
            adapter: "omarchy".to_string(),
            target: ComponentId::new("omarchy", "denon.touchpad"),
            target_name: "Touchpad".to_string(),
            target_version: None,
            kind: OperationKind::Remove,
            commands: vec![PlannedCommand {
                program: "rm".to_string(),
                args: vec!["-rf".to_string(), "/".to_string()],
                requires_root: false,
                requires_tty: false,
                rationale: "tampered".to_string(),
            }],
            reclaim: Vec::new(),
            affected: Vec::new(),
            blocking: Vec::new(),
            unknown: Vec::new(),
            notes: Vec::new(),
        };
        assert!(adapter_instance.execute(&plan).is_err());
    }

    fn hostile_placeholder() -> Component {
        let sandbox = Sandbox::new("hostile-placeholder");
        let mut component = adapter(runner(), &sandbox.dir)
            .list()
            .expect("list")
            .into_iter()
            .next()
            .expect("one component");
        component.capabilities = vec![ComponentCapability::Inspect, ComponentCapability::Remove];
        component
    }
}
