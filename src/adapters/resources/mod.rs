//! Fonts, themes and icon themes: an informative inventory.
//!
//! Sources:
//! * `fc-list` — every font fontconfig knows about, with its file, family and style.
//! * the Omarchy theme roots — `~/.config/omarchy/themes` (installed by the user) and
//!   `/usr/share/omarchy/themes` (shipped in the package). Omarchy's own
//!   `omarchy theme set` decides which one is active; this adapter never switches or
//!   removes one.
//! * the standard icon-theme roots — `~/.local/share/icons`, `~/.icons` and
//!   `/usr/share/icons`.
//!
//! Nothing here is removable. These are shared resources: deleting a font or an icon
//! theme because a name matched is exactly the heuristic the specification forbids, and
//! a theme directory is owned by Omarchy, not by us.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::domain::{
    AdapterAvailability, AdapterError, AdapterStatus, Component, ComponentAdapter,
    ComponentCapability, ComponentCategory, ComponentId, ComponentSource, ComponentStatus,
    DetailField, DetectionConfidence, DetectionMethod, InstallScope, OperationKind,
    OperationOutcome, OperationPlan, OperationRequest, ResourceKind,
};
use crate::infrastructure::process::{CommandOutput, CommandRunner, CommandSpec, ProcessError};

const ADAPTER_ID: &str = "resources";
const TIMEOUT: Duration = Duration::from_secs(20);
const COMMANDS: &[&str] = &["fc-list"];

/// One `fc-list` row: `file: Family:style=Style`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontEntry {
    pub path: String,
    pub family: String,
    pub style: Option<String>,
    /// How many `fc-list` rows mentioned this file. fontconfig reports one row per
    /// style and language variant of the same file.
    pub occurrences: usize,
}

pub struct ResourcesAdapter {
    runner: Arc<dyn CommandRunner>,
    user_themes: PathBuf,
    system_themes: PathBuf,
    user_icons: Vec<PathBuf>,
    system_icons: PathBuf,
}

impl ResourcesAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>) -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        Self {
            runner,
            user_themes: home.join(".config/omarchy/themes"),
            system_themes: PathBuf::from("/usr/share/omarchy/themes"),
            user_icons: vec![home.join(".local/share/icons"), home.join(".icons")],
            system_icons: PathBuf::from("/usr/share/icons"),
        }
    }

    #[cfg(test)]
    pub fn with_roots(
        runner: Arc<dyn CommandRunner>,
        user_themes: PathBuf,
        system_themes: PathBuf,
        user_icons: Vec<PathBuf>,
        system_icons: PathBuf,
    ) -> Self {
        Self {
            runner,
            user_themes,
            system_themes,
            user_icons,
            system_icons,
        }
    }

    fn fonts(&self) -> Result<Vec<Component>, AdapterError> {
        let spec = CommandSpec::capture("fc-list", Vec::new()).with_timeout(TIMEOUT);
        let output: CommandOutput = self.runner.run(&spec).map_err(|error| match error {
            ProcessError::NotFound { .. } => AdapterError::ToolUnavailable {
                tool: "fc-list".to_string(),
                reason: "not found in the executable search path".to_string(),
            },
            other => AdapterError::command("fc-list", "fc-list", other.to_string()),
        })?;
        if !output.success() {
            return Err(AdapterError::command(
                "fc-list",
                "fc-list",
                format!("exit {:?}: {}", output.exit_code, output.stderr.trim()),
            ));
        }

        let entries = parse_fc_list(&output.stdout);
        let mut per_family: BTreeMap<String, usize> = BTreeMap::new();
        for entry in &entries {
            *per_family.entry(entry.family.clone()).or_default() += 1;
        }

        Ok(entries
            .iter()
            .map(|entry| {
                let mut extra = vec![
                    DetailField::new("File", entry.path.clone()),
                    DetailField::new(
                        "Family files",
                        per_family
                            .get(&entry.family)
                            .copied()
                            .unwrap_or(1)
                            .to_string(),
                    ),
                ];
                if let Some(style) = &entry.style {
                    extra.push(DetailField::new("Style", style.clone()));
                }
                Component {
                    id: ComponentId::new(ADAPTER_ID, entry.path.clone()),
                    name: entry.family.clone(),
                    version: None,
                    description: None,
                    category: ComponentCategory::Font,
                    source: ComponentSource::Resource {
                        resource_kind: ResourceKind::Font,
                    },
                    status: ComponentStatus::Installed,
                    confidence: DetectionConfidence::Authoritative,
                    capabilities: vec![ComponentCapability::Inspect],
                    detection: DetectionMethod::command("fc-list", Vec::new())
                        .with_note("reported by fontconfig"),
                    installed_size: None,
                    location: Some(PathBuf::from(&entry.path)),
                    installed_at: None,
                    depends_on: Vec::new(),
                    required_by: Vec::new(),
                    extra,
                }
            })
            .collect())
    }

    fn theme_components(&self) -> Vec<Component> {
        let mut components = Vec::new();
        for (root, scope) in [
            (&self.user_themes, InstallScope::User),
            (&self.system_themes, InstallScope::System),
        ] {
            for (name, path) in directories(root) {
                components.push(Component {
                    id: ComponentId::new(ADAPTER_ID, format!("theme:{}:{name}", scope.label())),
                    name: name.clone(),
                    version: None,
                    description: Some(format!("Omarchy theme in the {} theme root", scope.label())),
                    category: ComponentCategory::Theme,
                    source: ComponentSource::Resource {
                        resource_kind: ResourceKind::Theme,
                    },
                    status: ComponentStatus::Installed,
                    confidence: DetectionConfidence::Authoritative,
                    capabilities: vec![ComponentCapability::Inspect],
                    detection: DetectionMethod::scan(root)
                        .with_note("a theme is a directory in an Omarchy theme root"),
                    installed_size: None,
                    location: Some(path.clone()),
                    installed_at: None,
                    depends_on: Vec::new(),
                    required_by: Vec::new(),
                    extra: vec![
                        DetailField::new("Root", scope.label()),
                        DetailField::new("Path", path.display().to_string()),
                        DetailField::new(
                            "Git clone",
                            if path.join(".git").exists() {
                                "yes"
                            } else {
                                "no"
                            },
                        ),
                        DetailField::new("Activate with", "omarchy theme set <name>".to_string()),
                    ],
                });
            }
        }
        components
    }

    fn icon_components(&self) -> Vec<Component> {
        let mut components = Vec::new();
        let mut roots: Vec<(&PathBuf, InstallScope)> = self
            .user_icons
            .iter()
            .map(|path| (path, InstallScope::User))
            .collect();
        roots.push((&self.system_icons, InstallScope::System));

        for (root, scope) in roots {
            for (name, path) in directories(root) {
                components.push(Component {
                    id: ComponentId::new(ADAPTER_ID, format!("icons:{}:{name}", scope.label())),
                    name,
                    version: None,
                    description: None,
                    category: ComponentCategory::Icon,
                    source: ComponentSource::Resource {
                        resource_kind: ResourceKind::IconTheme,
                    },
                    status: ComponentStatus::Installed,
                    confidence: DetectionConfidence::Authoritative,
                    capabilities: vec![ComponentCapability::Inspect],
                    detection: DetectionMethod::scan(root),
                    installed_size: None,
                    location: Some(path.clone()),
                    installed_at: None,
                    depends_on: Vec::new(),
                    required_by: Vec::new(),
                    extra: vec![
                        DetailField::new("Root", scope.label()),
                        DetailField::new("Path", path.display().to_string()),
                    ],
                });
            }
        }
        components
    }
}

/// Direct child directories, sorted, so the inventory is stable between runs.
fn directories(root: &PathBuf) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut directories: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            // Hidden entries are not themes or icon themes.
            (path.is_dir() && !name.starts_with('.')).then_some((name, path))
        })
        .collect();
    directories.sort_by(|left, right| left.0.cmp(&right.0));
    directories
}

/// Parses `fc-list` output: `path: Family:style=Style`.
///
/// Rows are collapsed per file: fontconfig lists the same file once per language and
/// style variant, and one file is one font.
pub fn parse_fc_list(text: &str) -> Vec<FontEntry> {
    let mut fonts: Vec<FontEntry> = Vec::new();
    for entry in parse_fc_list_rows(text) {
        match fonts.iter_mut().find(|known| known.path == entry.path) {
            Some(known) => known.occurrences += 1,
            None => fonts.push(entry),
        }
    }
    fonts
}

fn parse_fc_list_rows(text: &str) -> Vec<FontEntry> {
    text.lines()
        .filter_map(|line| {
            let (path, rest) = match line.split_once(": ") {
                Some(parts) => parts,
                None => (line, ""),
            };
            let path = path.trim();
            if path.is_empty() {
                return None;
            }
            let (family, style) = match rest.split_once(":style=") {
                Some((family, style)) => (family.trim(), Some(style.trim().to_string())),
                None => (rest.trim(), None),
            };
            Some(FontEntry {
                path: path.to_string(),
                family: if family.is_empty() {
                    path.to_string()
                } else {
                    family.to_string()
                },
                style,
                occurrences: 1,
            })
        })
        .collect()
}

impl ComponentAdapter for ResourcesAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn display_name(&self) -> &'static str {
        "Fonts, themes & icons"
    }

    fn description(&self) -> &'static str {
        "Installed fonts, Omarchy themes and icon themes, with their paths"
    }

    fn capabilities(&self) -> Vec<ComponentCapability> {
        vec![ComponentCapability::Inspect]
    }

    fn status(&self) -> AdapterStatus {
        let tool = self.runner.resolve("fc-list");
        let theme_roots = [self.user_themes.exists(), self.system_themes.exists()]
            .iter()
            .filter(|exists| **exists)
            .count();
        let icon_root =
            self.user_icons.iter().any(|path| path.exists()) || self.system_icons.exists();

        let mut notes =
            vec!["informative: nothing here is removable, these files are shared".to_string()];
        match (&tool, theme_roots, icon_root) {
            (None, 0, false) => {
                return AdapterStatus {
                    id: ADAPTER_ID.to_string(),
                    display_name: self.display_name().to_string(),
                    description: self.description().to_string(),
                    availability: AdapterAvailability::Unavailable {
                        reason: "no font, theme or icon source was found on this machine"
                            .to_string(),
                    },
                    tool: None,
                    tool_version: None,
                    capabilities: self.capabilities(),
                    commands: COMMANDS
                        .iter()
                        .map(|command| (*command).to_string())
                        .collect(),
                    notes,
                };
            }
            (None, _, _) => notes.push("fc-list is missing: fonts are not listed".to_string()),
            (Some(_), _, _) => {
                notes.push("fonts come from fc-list (fontconfig)".to_string());
                notes.push("the active theme is decided by 'omarchy theme set'".to_string());
            }
        }
        if theme_roots == 0 {
            notes.push("no Omarchy theme root exists here".to_string());
        }

        AdapterStatus {
            id: ADAPTER_ID.to_string(),
            display_name: self.display_name().to_string(),
            description: self.description().to_string(),
            availability: AdapterAvailability::Available,
            tool,
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
        // One source failing must not hide the others: a missing fc-list still leaves
        // themes and icon themes to report.
        let mut components = match self.fonts() {
            Ok(fonts) => fonts,
            Err(error) => {
                tracing::warn!(%error, "font listing unavailable; themes and icons still load");
                Vec::new()
            }
        };
        components.extend(self.theme_components());
        components.extend(self.icon_components());
        components.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
        Ok(components)
    }

    fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError> {
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
            "operations (fonts, themes and icons are shared resources; removal is not offered)",
        ))
    }

    fn execute(&self, _plan: &OperationPlan) -> Result<OperationOutcome, AdapterError> {
        Err(AdapterError::unsupported(ADAPTER_ID, "operations"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::test_support::FakeRunner;

    const FC_LIST: &str = include_str!("../../../tests/fixtures/resources/fc-list.txt");

    struct Roots {
        dir: PathBuf,
    }

    impl Roots {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("occ-res-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("user-themes/cyberpunk")).expect("theme");
            std::fs::create_dir_all(dir.join("user-themes/cyberpunk/.git")).expect("git");
            std::fs::create_dir_all(dir.join("user-themes/.hidden")).expect("hidden");
            std::fs::create_dir_all(dir.join("system-themes/catppuccin")).expect("theme");
            std::fs::create_dir_all(dir.join("user-icons/Yaru")).expect("icons");
            std::fs::create_dir_all(dir.join("system-icons/Adwaita")).expect("icons");
            Self { dir }
        }

        fn adapter(&self, runner: FakeRunner) -> ResourcesAdapter {
            ResourcesAdapter::with_roots(
                Arc::new(runner),
                self.dir.join("user-themes"),
                self.dir.join("system-themes"),
                vec![self.dir.join("user-icons")],
                self.dir.join("system-icons"),
            )
        }
    }

    impl Drop for Roots {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn collapses_the_same_file_reported_once_per_style() {
        let repeated = "/usr/share/fonts/x/AdwaitaSans.ttf: Adwaita:style=Italic\n\
                        /usr/share/fonts/x/AdwaitaSans.ttf: Adwaita:style=Italic\n\
                        /usr/share/fonts/x/AdwaitaSans.ttf: Adwaita:style=Italic\n";
        let entries = parse_fc_list(repeated);
        assert_eq!(entries.len(), 1, "one file is one font");
        assert_eq!(entries[0].occurrences, 3);
    }

    #[test]
    fn parses_fc_list_rows_into_family_and_style() {
        let entries = parse_fc_list(FC_LIST);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].family, "Noto Sans Tai Le");
        assert_eq!(entries[0].style.as_deref(), Some("Regular"));
        assert!(entries[0].path.ends_with("NotoSansTaiLe-Regular.ttf"));
    }

    #[test]
    fn lists_fonts_themes_and_icon_themes_together() {
        let roots = Roots::new("list");
        let components = roots
            .adapter(FakeRunner::new().on("fc-list", FC_LIST))
            .list()
            .expect("list");

        assert_eq!(
            components
                .iter()
                .filter(|component| component.category == ComponentCategory::Font)
                .count(),
            4
        );
        assert_eq!(
            components
                .iter()
                .filter(|component| component.category == ComponentCategory::Theme)
                .count(),
            2
        );
        assert_eq!(
            components
                .iter()
                .filter(|component| component.category == ComponentCategory::Icon)
                .count(),
            2
        );
    }

    #[test]
    fn a_family_with_several_files_shows_how_many_there_are() {
        let roots = Roots::new("family");
        let components = roots
            .adapter(FakeRunner::new().on("fc-list", FC_LIST))
            .list()
            .expect("list");
        let jetbrains: Vec<&Component> = components
            .iter()
            .filter(|component| component.name == "JetBrainsMono Nerd Font")
            .collect();
        assert_eq!(jetbrains.len(), 2, "one component per font file");
        assert!(jetbrains.iter().all(|component| {
            component
                .extra
                .iter()
                .any(|field| field.label == "Family files" && field.value == "2")
        }));
    }

    #[test]
    fn hidden_directories_are_not_themes() {
        let roots = Roots::new("hidden");
        let components = roots
            .adapter(FakeRunner::new().on("fc-list", FC_LIST))
            .list()
            .expect("list");
        assert!(
            !components
                .iter()
                .any(|component| component.name == ".hidden")
        );
    }

    #[test]
    fn a_user_and_a_packaged_theme_with_the_same_name_stay_distinct() {
        let roots = Roots::new("scope");
        std::fs::create_dir_all(roots.dir.join("user-themes/catppuccin")).expect("theme");
        let components = roots
            .adapter(FakeRunner::new().on("fc-list", FC_LIST))
            .list()
            .expect("list");
        let catppuccin: Vec<&Component> = components
            .iter()
            .filter(|component| component.name == "catppuccin")
            .collect();
        assert_eq!(catppuccin.len(), 2);
        assert_ne!(catppuccin[0].id, catppuccin[1].id);
    }

    #[test]
    fn a_theme_reports_whether_it_is_a_git_clone() {
        let roots = Roots::new("git");
        let components = roots
            .adapter(FakeRunner::new().on("fc-list", FC_LIST))
            .list()
            .expect("list");
        let cyberpunk = components
            .iter()
            .find(|component| component.name == "cyberpunk")
            .expect("theme");
        assert!(
            cyberpunk
                .extra
                .iter()
                .any(|field| field.label == "Git clone" && field.value == "yes")
        );
    }

    #[test]
    fn a_missing_fc_list_still_lists_themes_and_icons() {
        let roots = Roots::new("no-fc");
        let adapter = roots.adapter(FakeRunner::new());
        let components = adapter.list().expect("themes must still load");
        assert!(
            components
                .iter()
                .all(|component| component.category != ComponentCategory::Font)
        );
        assert!(
            components
                .iter()
                .any(|component| component.category == ComponentCategory::Theme)
        );
        assert!(
            components
                .iter()
                .any(|component| component.category == ComponentCategory::Icon)
        );
        assert!(
            adapter
                .status()
                .notes
                .iter()
                .any(|note| note.contains("fc-list is missing"))
        );
    }

    #[test]
    fn nothing_here_is_ever_removable() {
        let roots = Roots::new("ops");
        let adapter = roots.adapter(FakeRunner::new().on("fc-list", FC_LIST));
        let components = adapter.list().expect("list");
        assert!(
            components
                .iter()
                .all(|component| component.capabilities == vec![ComponentCapability::Inspect])
        );
        assert!(
            components
                .iter()
                .all(|component| !adapter.supports(component, OperationKind::Remove))
        );
        let component = components.first().expect("one").clone();
        let request = OperationRequest::new(component.id.clone(), OperationKind::Remove);
        assert!(adapter.plan_operation(&request, &component).is_err());
    }

    #[test]
    fn reports_itself_unavailable_when_no_source_exists_at_all() {
        let missing = std::env::temp_dir().join(format!("occ-res-absent-{}", std::process::id()));
        let adapter = ResourcesAdapter::with_roots(
            Arc::new(FakeRunner::new()),
            missing.join("themes"),
            missing.join("system-themes"),
            vec![missing.join("icons")],
            missing.join("system-icons"),
        );
        assert!(!adapter.status().available());
    }
}
