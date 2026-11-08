//! lazy.nvim plugins for Neovim.
//!
//! This adapter runs no command at all: every fact comes from two files.
//!
//! * `~/.config/nvim/lazy-lock.json` — lazy.nvim's own lock file. It has one entry
//!   per plugin the Neovim configuration references, with the branch and the commit
//!   that is locked. It is authoritative for "configured".
//! * `~/.local/share/nvim/lazy/<plugin>` — the directory lazy.nvim installs that
//!   plugin into. It is authoritative for "downloaded". The two counts disagree on a
//!   real machine (a lock entry can survive an undownloaded plugin), which is exactly
//!   the state this adapter has to show instead of hiding.
//!
//! Removal is not offered (spec S17, task T16): lazy.nvim exposes no command-line
//! interface, and the plugin is still referenced by the Neovim configuration, so
//! deleting its directory would leave the configuration pointing at nothing. The
//! only capability is inspection.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::domain::{
    AdapterAvailability, AdapterError, AdapterStatus, Component, ComponentAdapter,
    ComponentCapability, ComponentCategory, ComponentId, ComponentSource, ComponentStatus,
    DetailField, DetectionConfidence, DetectionMethod, OperationKind, OperationOutcome,
    OperationPlan, OperationRequest, shown,
};

const ADAPTER_ID: &str = "lazy";

/// No command is ever run: the adapter reads files (see the module documentation).
const COMMANDS: &[&str] = &[];

/// One entry of `lazy-lock.json`. Both fields stay optional: a lock file that omits
/// the branch must show "No disponible" rather than a filled-in default.
#[derive(Debug, Clone, Default, Deserialize)]
struct LockEntry {
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    commit: Option<String>,
}

pub struct LazyNvimAdapter {
    /// `~/.config/nvim`.
    config_dir: PathBuf,
    /// `~/.local/share/nvim`.
    data_dir: PathBuf,
}

impl LazyNvimAdapter {
    /// Resolves the Neovim directories the way Neovim itself does.
    pub fn new() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".local/share"));
        Self::with_roots(config_home.join("nvim"), data_home.join("nvim"))
    }

    /// Points the adapter at another pair of Neovim directories, so tests never
    /// depend on the developer's own configuration.
    pub fn with_roots(config_dir: PathBuf, data_dir: PathBuf) -> Self {
        Self {
            config_dir,
            data_dir,
        }
    }

    fn lock_path(&self) -> PathBuf {
        self.config_dir.join("lazy-lock.json")
    }

    fn install_root(&self) -> PathBuf {
        self.data_dir.join("lazy")
    }

    fn plugin_dir(&self, name: &str) -> PathBuf {
        self.install_root().join(name)
    }

    /// Reads the lock file. `Ok(None)` means the file does not exist, which is the
    /// "lazy.nvim is not used here" case, not an error. A file that exists but does
    /// not parse is an error: reporting an empty inventory would claim that no
    /// plugin is configured.
    fn read_lock(&self) -> Result<Option<BTreeMap<String, LockEntry>>, AdapterError> {
        let path = self.lock_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(AdapterError::Parse {
                    context: path.display().to_string(),
                    detail: error.to_string(),
                });
            }
        };
        serde_json::from_str::<BTreeMap<String, LockEntry>>(&text)
            .map(Some)
            .map_err(|error| AdapterError::Parse {
                context: path.display().to_string(),
                detail: error.to_string(),
            })
    }

    fn component(&self, name: &str, entry: &LockEntry) -> Component {
        let dir = self.plugin_dir(name);
        let downloaded = dir.is_dir();
        let git_clone = downloaded && dir.join(".git").exists();

        Component {
            id: ComponentId::new(ADAPTER_ID, name.to_string()),
            name: name.to_string(),
            // The locked commit is the only version lazy.nvim records.
            version: entry.commit.clone(),
            description: None,
            category: ComponentCategory::Plugin,
            source: ComponentSource::LazyNvim { downloaded },
            status: if downloaded {
                ComponentStatus::Installed
            } else {
                // Configured in the lock file but never downloaded.
                ComponentStatus::Missing
            },
            confidence: DetectionConfidence::Authoritative,
            // No manager and a configuration that still references the plugin: the
            // adapter offers inspection only (S17).
            capabilities: vec![ComponentCapability::Inspect],
            detection: DetectionMethod::scan(self.lock_path()).with_note(
                "plugin entry read from lazy-lock.json; presence read from the lazy.nvim \
                 install directory",
            ),
            installed_size: None,
            location: downloaded.then_some(dir),
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: vec![
                DetailField::new("Branch", shown(entry.branch.as_deref())),
                DetailField::new("Commit", shown(entry.commit.as_deref())),
                DetailField::new("Downloaded", if downloaded { "yes" } else { "no" }),
                DetailField::new(
                    "Directory is a git clone",
                    if git_clone { "yes" } else { "no" },
                ),
                DetailField::new("Install path", self.plugin_dir(name).display().to_string()),
            ],
        }
    }
}

impl Default for LazyNvimAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// A plugin name is joined onto the install root, so a name that could step outside
/// it is refused instead of being listed. This is a file-only boundary, but it is
/// still the boundary where untrusted file content meets a path.
fn validate_plugin_name(value: &str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("must not be empty");
    }
    if value.len() > 255 {
        return Err("is longer than 255 bytes");
    }
    if value.trim() != value {
        return Err("has leading or trailing whitespace");
    }
    if value.contains("..") {
        return Err("contains a parent-directory segment");
    }
    if value
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+')))
    {
        return Err("contains a character outside the allowed set");
    }
    Ok(())
}

impl ComponentAdapter for LazyNvimAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn display_name(&self) -> &'static str {
        "Neovim plugins"
    }

    fn description(&self) -> &'static str {
        "Plugins managed by lazy.nvim: the lock file against the downloaded directories"
    }

    fn capabilities(&self) -> Vec<ComponentCapability> {
        vec![ComponentCapability::Inspect]
    }

    fn status(&self) -> AdapterStatus {
        let lock_path = self.lock_path();
        let availability = if lock_path.is_file() {
            AdapterAvailability::Available
        } else {
            AdapterAvailability::Unavailable {
                reason: format!(
                    "{} does not exist, so lazy.nvim is not used with this Neovim configuration",
                    lock_path.display()
                ),
            }
        };

        AdapterStatus {
            id: ADAPTER_ID.to_string(),
            display_name: self.display_name().to_string(),
            description: self.description().to_string(),
            availability,
            // No tool is involved: the facts come from files alone.
            tool: None,
            tool_version: None,
            capabilities: self.capabilities(),
            commands: COMMANDS
                .iter()
                .map(|command| (*command).to_string())
                .collect(),
            notes: vec![
                format!("lock file: {}", lock_path.display()),
                format!("install directory: {}", self.install_root().display()),
                "facts come from files: this adapter runs no command".to_string(),
                "removal is not offered: lazy.nvim has no removal command and the Neovim \
                 configuration still references these plugins"
                    .to_string(),
            ],
        }
    }

    fn list(&self) -> Result<Vec<Component>, AdapterError> {
        let Some(lock) = self.read_lock()? else {
            return Ok(Vec::new());
        };

        // Directories without a lock entry are deliberately not listed: the lock file
        // is lazy.nvim's own statement of what the configuration uses, and a leftover
        // directory is not a configured plugin. Inventing one would contradict S9.
        let mut components = Vec::with_capacity(lock.len());
        for (name, entry) in &lock {
            if let Err(reason) = validate_plugin_name(name) {
                tracing::warn!(plugin = %name, reason, "skipping an unusable lazy-lock entry");
                continue;
            }
            components.push(self.component(name, entry));
        }
        Ok(components)
    }

    fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError> {
        if let Err(reason) = validate_plugin_name(native_id) {
            return Err(AdapterError::InvalidId {
                adapter: ADAPTER_ID.to_string(),
                value: native_id.to_string(),
                reason: reason.to_string(),
            });
        }

        // Both sources are re-read, so the answer describes the files as they are now.
        let Some(lock) = self.read_lock()? else {
            return Ok(None);
        };
        // Only a lock entry is a component (see `list`): a directory on its own has no
        // identity to report, so it has no details either.
        Ok(lock
            .get(native_id)
            .map(|entry| self.component(native_id, entry)))
    }

    /// Nothing here can be modified: the plugin belongs to a configuration file that
    /// this program must not edit, and lazy.nvim has no manager command to delegate to.
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

    const LOCK: &str = include_str!("../../../tests/fixtures/lazy/lazy-lock.json");

    struct Sandbox {
        dir: PathBuf,
    }

    impl Sandbox {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("occ-lazy-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("config")).expect("config dir");
            std::fs::create_dir_all(dir.join("data/lazy")).expect("data dir");
            Self { dir }
        }

        fn adapter(&self) -> LazyNvimAdapter {
            LazyNvimAdapter::with_roots(self.dir.join("config"), self.dir.join("data"))
        }

        /// Writes the realistic lock file fixture: four configured plugins.
        fn write_lock(&self, contents: &str) {
            std::fs::write(self.dir.join("config/lazy-lock.json"), contents).expect("lock file");
        }

        /// Creates an installed plugin directory, optionally a git clone.
        fn install(&self, name: &str, git: bool) {
            let dir = self.dir.join("data/lazy").join(name);
            std::fs::create_dir_all(&dir).expect("plugin dir");
            if git {
                std::fs::create_dir_all(dir.join(".git")).expect("git dir");
            }
        }

        fn lock_path(&self) -> String {
            self.dir.join("config/lazy-lock.json").display().to_string()
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn field(component: &Component, label: &str) -> Option<String> {
        component
            .extra
            .iter()
            .find(|field| field.label == label)
            .map(|field| field.value.clone())
    }

    #[test]
    fn without_a_lock_file_the_plugin_manager_is_unavailable_and_nothing_is_listed() {
        let sandbox = Sandbox::new("no-lock");
        let adapter = sandbox.adapter();

        let status = adapter.status();
        assert!(!status.available());
        assert_eq!(
            status.availability,
            AdapterAvailability::Unavailable {
                reason: format!(
                    "{} does not exist, so lazy.nvim is not used with this Neovim configuration",
                    sandbox.lock_path()
                )
            }
        );
        assert!(status.commands.is_empty(), "this adapter runs no command");
        assert!(status.tool.is_none());
        assert!(
            status
                .notes
                .iter()
                .any(|note| note.contains("runs no command"))
        );
        assert!(adapter.list().expect("list").is_empty());
    }

    #[test]
    fn a_lock_entry_with_a_directory_is_installed_with_its_locked_commit() {
        let sandbox = Sandbox::new("installed");
        sandbox.write_lock(LOCK);
        sandbox.install("plenary.nvim", true);
        sandbox.install("nvim-treesitter", false);

        let components = sandbox.adapter().list().expect("list");
        assert_eq!(components.len(), 4, "the fixture holds four entries");

        let plenary = components
            .iter()
            .find(|component| component.id.native_id == "plenary.nvim")
            .expect("plenary");
        assert_eq!(
            plenary.id,
            ComponentId::new("lazy", "plenary.nvim"),
            "identity is adapter plus the plugin name"
        );
        assert_eq!(plenary.name, "plenary.nvim");
        assert_eq!(plenary.category, ComponentCategory::Plugin);
        assert_eq!(
            plenary.source,
            ComponentSource::LazyNvim { downloaded: true }
        );
        assert_eq!(plenary.status, ComponentStatus::Installed);
        assert_eq!(plenary.confidence, DetectionConfidence::Authoritative);
        assert_eq!(
            plenary.capabilities,
            vec![ComponentCapability::Inspect],
            "there is no manager to remove through"
        );
        assert_eq!(
            plenary.version.as_deref(),
            Some("2d9b17e1819a1a4c6e8b2f9f6f2c1b0a1c2d3e4f")
        );
        assert_eq!(
            plenary.location,
            Some(sandbox.dir.join("data/lazy/plenary.nvim"))
        );
        assert_eq!(field(plenary, "Branch").as_deref(), Some("master"));
        assert_eq!(
            field(plenary, "Commit").as_deref(),
            Some("2d9b17e1819a1a4c6e8b2f9f6f2c1b0a1c2d3e4f")
        );
        assert_eq!(field(plenary, "Downloaded").as_deref(), Some("yes"));
        assert_eq!(
            field(plenary, "Directory is a git clone").as_deref(),
            Some("yes")
        );
        assert_eq!(
            plenary.detection,
            DetectionMethod::scan(sandbox.dir.join("config/lazy-lock.json")).with_note(
                "plugin entry read from lazy-lock.json; presence read from the lazy.nvim \
                     install directory",
            )
        );

        let treesitter = components
            .iter()
            .find(|component| component.id.native_id == "nvim-treesitter")
            .expect("treesitter");
        assert_eq!(
            field(treesitter, "Directory is a git clone").as_deref(),
            Some("no")
        );
    }

    #[test]
    fn a_lock_entry_without_a_directory_is_missing_and_the_list_says_so() {
        let sandbox = Sandbox::new("missing");
        sandbox.write_lock(LOCK);
        sandbox.install("plenary.nvim", true);

        let components = sandbox.adapter().list().expect("list");
        let telescope = components
            .iter()
            .find(|component| component.id.native_id == "telescope.nvim")
            .expect("telescope");

        assert_eq!(telescope.status, ComponentStatus::Missing);
        assert_eq!(
            telescope.source,
            ComponentSource::LazyNvim { downloaded: false }
        );
        assert_eq!(field(telescope, "Downloaded").as_deref(), Some("no"));
        assert_eq!(
            telescope.location, None,
            "there is no directory to point at"
        );
        assert_eq!(
            telescope.version.as_deref(),
            Some("a0bbec21143c7bc5f8bb02e0ae5e8d2f2c0f3f3a"),
            "the lock still knows which commit is configured"
        );
    }

    #[test]
    fn a_directory_without_a_lock_entry_is_not_invented_as_a_component() {
        let sandbox = Sandbox::new("lost-dir");
        sandbox.write_lock(LOCK);
        sandbox.install("some-leftover-plugin", true);

        let components = sandbox.adapter().list().expect("list");
        assert!(
            !components
                .iter()
                .any(|component| component.id.native_id == "some-leftover-plugin"),
            "the lock file is the authority on what is configured: {components:?}"
        );
        assert_eq!(components.len(), 4);
    }

    #[test]
    fn a_malformed_lock_file_is_a_parse_error_not_an_empty_inventory() {
        let sandbox = Sandbox::new("malformed");
        sandbox.write_lock("{ \"plenary.nvim\": [ this is not json");

        let error = sandbox.adapter().list().expect_err("must not be silent");
        match error {
            AdapterError::Parse { context, .. } => assert_eq!(context, sandbox.lock_path()),
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    #[test]
    fn details_re_read_both_sources_for_one_plugin() {
        let sandbox = Sandbox::new("details");
        sandbox.write_lock(LOCK);
        sandbox.install("plenary.nvim", true);
        let adapter = sandbox.adapter();
        assert_eq!(
            adapter
                .details("plenary.nvim")
                .expect("details")
                .expect("present")
                .status,
            ComponentStatus::Installed
        );

        // The directory disappears between listing and opening the details.
        std::fs::remove_dir_all(sandbox.dir.join("data/lazy/plenary.nvim")).expect("remove dir");
        let refreshed = adapter
            .details("plenary.nvim")
            .expect("details")
            .expect("still configured");
        assert_eq!(refreshed.status, ComponentStatus::Missing);

        assert!(
            adapter
                .details("not-in-the-lock")
                .expect("details")
                .is_none(),
            "a plugin in neither source does not exist"
        );
    }

    #[test]
    fn refuses_every_operation_without_running_anything() {
        let sandbox = Sandbox::new("operations");
        sandbox.write_lock(LOCK);
        sandbox.install("plenary.nvim", true);
        let adapter = sandbox.adapter();
        let component = adapter
            .list()
            .expect("list")
            .into_iter()
            .find(|component| component.id.native_id == "plenary.nvim")
            .expect("plenary");

        for kind in [OperationKind::Remove, OperationKind::Update] {
            assert!(!adapter.supports(&component, kind));
        }

        let request = OperationRequest::new(component.id.clone(), OperationKind::Remove);
        assert_eq!(
            adapter
                .plan_operation(&request, &component)
                .expect_err("no plan"),
            AdapterError::unsupported(ADAPTER_ID, "remove")
        );
    }

    #[test]
    fn a_hostile_plugin_name_cannot_escape_the_install_root() {
        let sandbox = Sandbox::new("hostile");
        assert!(validate_plugin_name("../escape").is_err());
        assert!(validate_plugin_name("a/b").is_err());
        assert!(validate_plugin_name("").is_err());
        let error = sandbox
            .adapter()
            .details("../../etc/passwd")
            .expect_err("must be refused");
        assert!(matches!(error, AdapterError::InvalidId { .. }), "{error:?}");
    }
}
