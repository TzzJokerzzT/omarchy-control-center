//! Zsh plugins: what the user's `.zshrc` configures, and which manager owns each.
//!
//! This adapter runs no command. Every fact comes from files:
//!
//! * `$ZDOTDIR/.zshrc` (or `~/.zshrc`) — the configuration, whose `plugins=(...)`
//!   assignment names the plugins zsh loads.
//! * `~/.oh-my-zsh/plugins/<name>` — a plugin that ships with the framework.
//! * `~/.oh-my-zsh/custom/plugins/<name>` — a plugin the user cloned themselves.
//!
//! A configured name that exists in neither is reported as `Missing` and owned by no
//! manager, because the configuration still references it (spec S9: never invent the
//! fact that it is installed).
//!
//! No file is ever modified: spec S17 forbids editing `.zshrc` or anything related to
//! it without showing a diff and asking for authorisation, and this adapter does not
//! implement that flow, so its only capability is inspection. A configuration line is
//! not treated as a package: only the `plugins=(...)` assignment is read, and a
//! commented-out plugin is not a configured plugin.

use std::path::PathBuf;

use crate::domain::{
    AdapterAvailability, AdapterError, AdapterStatus, Component, ComponentAdapter,
    ComponentCapability, ComponentCategory, ComponentId, ComponentSource, ComponentStatus,
    DetailField, DetectionConfidence, DetectionMethod, OperationKind, OperationOutcome,
    OperationPlan, OperationRequest, ZshManager,
};
use crate::infrastructure::validate::validate_plugin_id;

const ADAPTER_ID: &str = "zsh";

/// No command is ever run: the adapter reads files (see the module documentation).
const COMMANDS: &[&str] = &[];

/// The `plugins=(...)` assignment of a `.zshrc`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Assignment {
    names: Vec<String>,
    /// 1-based line of the `plugins` keyword, shown in the detail view.
    line: usize,
}

pub struct ZshAdapter {
    zshrc: PathBuf,
    /// `~/.oh-my-zsh`; absent on a hand-rolled zsh setup.
    framework_root: PathBuf,
}

impl ZshAdapter {
    /// Resolves the configuration the way zsh does: `$ZDOTDIR/.zshrc` when `ZDOTDIR`
    /// is set, `~/.zshrc` otherwise.
    pub fn new() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let zshrc = std::env::var_os("ZDOTDIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|dir| dir.join(".zshrc"))
            .unwrap_or_else(|| home.join(".zshrc"));
        Self::with_roots(zshrc, home.join(".oh-my-zsh"))
    }

    /// Points the adapter at another configuration and framework root, so tests never
    /// depend on the developer's own dotfiles.
    pub fn with_roots(zshrc: PathBuf, framework_root: PathBuf) -> Self {
        Self {
            zshrc,
            framework_root,
        }
    }

    fn configured(&self) -> Option<Assignment> {
        let text = std::fs::read_to_string(&self.zshrc).ok()?;
        parse_plugins_assignment(&text)
    }

    /// Where the plugin lives, when it lives anywhere.
    fn resolve(&self, name: &str) -> Option<(ZshManager, PathBuf)> {
        let framework_plugin = self.framework_root.join("plugins").join(name);
        if framework_plugin.is_dir() {
            return Some((ZshManager::OhMyZsh, framework_plugin));
        }
        let custom_plugin = self.framework_root.join("custom/plugins").join(name);
        if custom_plugin.is_dir() {
            return Some((ZshManager::CustomClone, custom_plugin));
        }
        None
    }

    fn component(&self, name: &str, line: usize) -> Component {
        let resolved = self.resolve(name);
        let (manager, location) = match resolved {
            Some((manager, path)) => (manager, Some(path)),
            None => (ZshManager::Manual, None),
        };
        let git_clone = location
            .as_ref()
            .is_some_and(|path| path.join(".git").exists());

        let mut extra = vec![
            DetailField::new("Configured at", format!("{}:{line}", self.zshrc.display())),
            DetailField::new("Managed by", manager_label(manager)),
        ];
        if let Some(path) = &location {
            extra.push(DetailField::new("Path", path.display().to_string()));
            extra.push(DetailField::new(
                "Directory is a git clone",
                if git_clone { "yes" } else { "no" },
            ));
        }

        Component {
            id: ComponentId::new(ADAPTER_ID, name.to_string()),
            name: name.to_string(),
            // A name in a shell configuration carries no version, and neither
            // directory reports one: guessing would invent it.
            version: None,
            description: None,
            category: ComponentCategory::Plugin,
            source: ComponentSource::Zsh {
                managed_by: manager,
            },
            status: if location.is_some() {
                ComponentStatus::Installed
            } else {
                ComponentStatus::Missing
            },
            // Read from files rather than reported by a manager: oh-my-zsh has no
            // machine-readable interface.
            confidence: DetectionConfidence::Low,
            // S17: the configuration must never be edited automatically.
            capabilities: vec![ComponentCapability::Inspect],
            detection: DetectionMethod::scan(&self.zshrc).with_note(
                "name from the plugins=(...) assignment in the zsh configuration; ownership \
                 from the oh-my-zsh directory tree",
            ),
            installed_size: None,
            location,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra,
        }
    }
}

impl Default for ZshAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn manager_label(manager: ZshManager) -> &'static str {
    match manager {
        ZshManager::OhMyZsh => "oh-my-zsh framework plugin",
        ZshManager::CustomClone => "custom clone in oh-my-zsh/custom/plugins",
        ZshManager::Manual => "no manager: the configuration names it but no directory exists",
    }
}

/// Removes `#` comments, keeping the line structure so offsets and line numbers stay
/// meaningful. A plugin name never contains `#`, so a simple cut is faithful here.
fn strip_comments(text: &str) -> String {
    text.lines()
        .map(|line| match line.find('#') {
            Some(index) => &line[..index],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_word_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// Finds `plugins` as a whole word followed by `=` and `(`, returning the keyword
/// offset and the offset just after the opening parenthesis.
fn find_assignment_start(text: &str) -> Option<(usize, usize)> {
    const KEYWORD: &str = "plugins";
    let mut offset = 0;
    while let Some(found) = text[offset..].find(KEYWORD) {
        let start = offset + found;
        let end = start + KEYWORD.len();
        offset = end;

        let word_before = text[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !is_word_char(character));
        let word_after = text[end..]
            .chars()
            .next()
            .is_none_or(|character| !is_word_char(character));
        if !word_before || !word_after {
            continue;
        }

        let after_keyword = text[end..].trim_start_matches([' ', '\t', '\n']);
        let Some(after_equals) = after_keyword.strip_prefix('=') else {
            continue;
        };
        let list = after_equals.trim_start_matches([' ', '\t', '\n']);
        if !list.starts_with('(') {
            continue;
        }
        // `list` is a suffix of `text`, so its start is where the '(' sits.
        return Some((start, text.len() - list.len() + 1));
    }
    None
}

/// Parses the first `plugins=(...)` assignment. The list may span several lines and
/// may quote its names. `None` means the configuration declares no plugins at all,
/// which is reported as an empty inventory instead of an invented one.
fn parse_plugins_assignment(text: &str) -> Option<Assignment> {
    let stripped = strip_comments(text);
    let (keyword_at, body_start) = find_assignment_start(&stripped)?;
    let body_end = body_start + stripped[body_start..].find(')')?;
    let names = stripped[body_start..body_end]
        .split_whitespace()
        .map(|token| token.trim_matches(|character| character == '"' || character == '\''))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();

    Some(Assignment {
        names,
        line: stripped[..keyword_at].matches('\n').count() + 1,
    })
}

impl ComponentAdapter for ZshAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn display_name(&self) -> &'static str {
        "Zsh plugins"
    }

    fn description(&self) -> &'static str {
        "Zsh plugins declared in .zshrc, with the oh-my-zsh framework and custom clones"
    }

    fn capabilities(&self) -> Vec<ComponentCapability> {
        vec![ComponentCapability::Inspect]
    }

    fn status(&self) -> AdapterStatus {
        let mut notes = vec![
            format!("configuration: {}", self.zshrc.display()),
            "facts come from files: this adapter runs no command".to_string(),
            "no file is ever modified: spec S17 forbids editing .zshrc without showing a diff \
             and asking for authorisation, so this adapter only reads it"
                .to_string(),
        ];
        if self.framework_root.is_dir() {
            notes.push(format!("oh-my-zsh: {}", self.framework_root.display()));
        } else {
            notes.push(format!(
                "oh-my-zsh was not found at {}: every configured plugin is reported as manual",
                self.framework_root.display()
            ));
        }
        if self.zshrc.is_file() && self.configured().is_none() {
            notes.push(format!(
                "no plugins=(...) assignment was found in {}: no zsh plugin is configured there",
                self.zshrc.display()
            ));
        }

        AdapterStatus {
            id: ADAPTER_ID.to_string(),
            display_name: self.display_name().to_string(),
            description: self.description().to_string(),
            availability: if self.zshrc.is_file() {
                AdapterAvailability::Available
            } else {
                AdapterAvailability::Unavailable {
                    reason: format!(
                        "{} does not exist, so no zsh plugin is configured on this machine",
                        self.zshrc.display()
                    ),
                }
            },
            // No tool is involved: the facts come from files alone.
            tool: None,
            tool_version: None,
            capabilities: self.capabilities(),
            commands: COMMANDS
                .iter()
                .map(|command| (*command).to_string())
                .collect(),
            notes,
        }
    }

    fn list(&self) -> Result<Vec<Component>, AdapterError> {
        let Some(assignment) = self.configured() else {
            // Either there is no `.zshrc` (the adapter is unavailable) or it declares no
            // plugins. Both are honestly empty; the status notes say which one it is.
            return Ok(Vec::new());
        };

        let mut components = Vec::with_capacity(assignment.names.len());
        for name in &assignment.names {
            // The name is joined onto the framework roots, so it must stay inside them.
            if let Err(reason) = validate_plugin_id(name) {
                tracing::warn!(
                    plugin = %name,
                    reason,
                    "skipping an unusable name in the plugins=(...) assignment"
                );
                continue;
            }
            components.push(self.component(name, assignment.line));
        }
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

        // The configuration and the directories are re-read for this one plugin.
        let Some(assignment) = self.configured() else {
            return Ok(None);
        };
        if !assignment.names.iter().any(|name| name == native_id) {
            return Ok(None);
        }
        Ok(Some(self.component(native_id, assignment.line)))
    }

    /// Nothing here can be modified: the plugin list lives in a configuration file that
    /// this program must not edit, and no manager command exists to delegate to.
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

    const ZSHRC: &str = include_str!("../../../tests/fixtures/zsh/zshrc");
    const ZSHRC_MULTILINE: &str = include_str!("../../../tests/fixtures/zsh/zshrc-multiline");

    struct Sandbox {
        dir: PathBuf,
    }

    impl Sandbox {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("occ-zsh-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("sandbox");
            Self { dir }
        }

        fn write_zshrc(&self, contents: &str) {
            std::fs::write(self.dir.join(".zshrc"), contents).expect("zshrc");
        }

        /// Creates a plugin directory under `<root>/plugins` or `<root>/custom/plugins`.
        fn framework_plugin(&self, relative: &str, git: bool) {
            let plugin = self.dir.join(".oh-my-zsh").join(relative);
            std::fs::create_dir_all(&plugin).expect("plugin dir");
            if git {
                std::fs::create_dir_all(plugin.join(".git")).expect("git dir");
            }
        }

        fn adapter(&self) -> ZshAdapter {
            ZshAdapter::with_roots(self.dir.join(".zshrc"), self.dir.join(".oh-my-zsh"))
        }

        fn zshrc_path(&self) -> String {
            self.dir.join(".zshrc").display().to_string()
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn names(components: &[Component]) -> Vec<String> {
        components
            .iter()
            .map(|component| component.id.native_id.clone())
            .collect()
    }

    fn field(component: &Component, label: &str) -> Option<String> {
        component
            .extra
            .iter()
            .find(|field| field.label == label)
            .map(|field| field.value.clone())
    }

    #[test]
    fn the_assignment_is_read_from_one_line_and_comments_are_ignored() {
        let parsed = parse_plugins_assignment(ZSHRC).expect("assignment");
        assert_eq!(
            parsed.names,
            vec!["git", "zsh-autosuggestions", "zsh-syntax-highlighting"],
            "the commented-out assignment must not contribute names"
        );
        assert_eq!(parsed.line, 10, "the line of the plugins= keyword");
    }

    #[test]
    fn a_multi_line_assignment_is_read_across_lines() {
        let parsed = parse_plugins_assignment(ZSHRC_MULTILINE).expect("assignment");
        assert_eq!(
            parsed.names,
            vec![
                "git",
                "fzf-tab",
                "zsh-autosuggestions",
                "zsh-syntax-highlighting",
                "zoxide",
            ],
            "inline comments must not become plugin names"
        );
        assert_eq!(parsed.line, 7);
    }

    #[test]
    fn a_framework_plugin_and_a_custom_clone_are_told_apart() {
        let sandbox = Sandbox::new("managers");
        sandbox.write_zshrc(ZSHRC);
        sandbox.framework_plugin("plugins/git", true);
        sandbox.framework_plugin("custom/plugins/zsh-autosuggestions", true);

        let components = sandbox.adapter().list().expect("list");
        assert_eq!(
            names(&components),
            vec!["git", "zsh-autosuggestions", "zsh-syntax-highlighting"]
        );

        let git = &components[0];
        assert_eq!(
            git.id,
            ComponentId::new("zsh", "git"),
            "identity is adapter plus the configured name"
        );
        assert_eq!(git.category, ComponentCategory::Plugin);
        assert_eq!(
            git.source,
            ComponentSource::Zsh {
                managed_by: ZshManager::OhMyZsh
            }
        );
        assert_eq!(git.status, ComponentStatus::Installed);
        assert_eq!(
            git.capabilities,
            vec![ComponentCapability::Inspect],
            "the configuration must never be edited automatically"
        );
        assert_eq!(
            git.location,
            Some(sandbox.dir.join(".oh-my-zsh/plugins/git"))
        );
        assert_eq!(
            field(git, "Configured at").as_deref(),
            Some(format!("{}:10", sandbox.zshrc_path()).as_str())
        );
        assert_eq!(
            field(git, "Directory is a git clone").as_deref(),
            Some("yes")
        );

        let custom = &components[1];
        assert_eq!(
            custom.source,
            ComponentSource::Zsh {
                managed_by: ZshManager::CustomClone
            }
        );
        assert_eq!(custom.status, ComponentStatus::Installed);
        assert_eq!(
            custom.location,
            Some(
                sandbox
                    .dir
                    .join(".oh-my-zsh/custom/plugins/zsh-autosuggestions")
            )
        );
    }

    #[test]
    fn a_configured_plugin_that_exists_nowhere_is_missing_and_manual() {
        let sandbox = Sandbox::new("missing");
        sandbox.write_zshrc(ZSHRC);
        sandbox.framework_plugin("plugins/git", true);

        let components = sandbox.adapter().list().expect("list");
        let missing = components
            .iter()
            .find(|component| component.id.native_id == "zsh-syntax-highlighting")
            .expect("configured plugin is still listed");

        assert_eq!(missing.status, ComponentStatus::Missing);
        assert_eq!(
            missing.source,
            ComponentSource::Zsh {
                managed_by: ZshManager::Manual
            }
        );
        assert_eq!(missing.location, None);
        assert_eq!(field(missing, "Path"), None, "there is no path to report");
    }

    #[test]
    fn a_configuration_without_an_assignment_lists_nothing_and_says_why() {
        let sandbox = Sandbox::new("no-assignment");
        sandbox.write_zshrc("export ZSH=\"$HOME/.oh-my-zsh\"\nsource $ZSH/oh-my-zsh.sh\n");

        let adapter = sandbox.adapter();
        assert!(
            adapter.list().expect("list").is_empty(),
            "a configuration line is not a package"
        );
        assert!(
            adapter
                .status()
                .notes
                .iter()
                .any(|note| note.starts_with("no plugins=(...) assignment was found in")),
            "the empty list must be explained: {:?}",
            adapter.status().notes
        );
    }

    #[test]
    fn without_oh_my_zsh_every_plugin_is_a_manual_reference() {
        let sandbox = Sandbox::new("no-framework");
        sandbox.write_zshrc(ZSHRC);

        let adapter = sandbox.adapter();
        let components = adapter.list().expect("list");
        assert_eq!(components.len(), 3);
        for component in &components {
            assert_eq!(
                component.source,
                ComponentSource::Zsh {
                    managed_by: ZshManager::Manual
                }
            );
            assert_eq!(component.status, ComponentStatus::Missing);
        }
        assert!(
            adapter
                .status()
                .notes
                .iter()
                .any(|note| note.starts_with("oh-my-zsh was not found at")
                    && note.ends_with("every configured plugin is reported as manual")),
            "the missing framework must be explained: {:?}",
            adapter.status().notes
        );
    }

    #[test]
    fn without_a_zshrc_the_integration_is_unavailable() {
        let sandbox = Sandbox::new("no-zshrc");
        let adapter = sandbox.adapter();

        let status = adapter.status();
        assert_eq!(
            status.availability,
            AdapterAvailability::Unavailable {
                reason: format!(
                    "{} does not exist, so no zsh plugin is configured on this machine",
                    sandbox.zshrc_path()
                )
            }
        );
        assert!(status.commands.is_empty(), "this adapter runs no command");
        assert!(status.tool.is_none());
        assert!(adapter.list().expect("list").is_empty());
    }

    #[test]
    fn details_re_read_the_configuration_and_the_framework() {
        let sandbox = Sandbox::new("details");
        sandbox.write_zshrc(ZSHRC_MULTILINE);
        sandbox.framework_plugin("plugins/git", true);
        let adapter = sandbox.adapter();

        let details = adapter
            .details("git")
            .expect("details")
            .expect("configured plugin");
        assert_eq!(details.status, ComponentStatus::Installed);
        assert_eq!(
            field(&details, "Configured at").as_deref(),
            Some(format!("{}:7", sandbox.zshrc_path()).as_str())
        );

        // The plugin directory disappears, so the next refresh must say so.
        std::fs::remove_dir_all(sandbox.dir.join(".oh-my-zsh/plugins/git")).expect("remove dir");
        let refreshed = adapter
            .details("git")
            .expect("details")
            .expect("still configured");
        assert_eq!(refreshed.status, ComponentStatus::Missing);

        assert!(
            adapter
                .details("not-configured")
                .expect("details")
                .is_none(),
            "a plugin the configuration does not name has no component"
        );
        assert!(matches!(
            adapter.details("--force").expect_err("refused"),
            AdapterError::InvalidId { .. }
        ));
    }

    #[test]
    fn refuses_every_operation_without_touching_the_configuration() {
        let sandbox = Sandbox::new("operations");
        sandbox.write_zshrc(ZSHRC);
        sandbox.framework_plugin("plugins/git", true);
        let adapter = sandbox.adapter();
        let component = adapter.list().expect("list").remove(0);

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
}
