//! Domain model: language shared by the UI, the application layer and every adapter.
//!
//! Nothing in this module runs a command or touches the terminal. Optional facts
//! stay `None` and are rendered through [`UNAVAILABLE`] — a source that does not
//! provide a value must never see one invented for it.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Placeholder used everywhere a source cannot provide a value.
pub const UNAVAILABLE: &str = "No disponible";

/// Renders an optional value using [`UNAVAILABLE`] when it is absent.
pub fn shown(value: Option<&str>) -> &str {
    value.unwrap_or(UNAVAILABLE)
}

/// Formats a byte count without inventing precision for the smallest unit.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Stable identity inside one ecosystem: adapter id plus the native identifier
/// the manager itself uses. The same name may exist in several ecosystems.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ComponentId {
    pub adapter: String,
    pub native_id: String,
}

impl ComponentId {
    pub fn new(adapter: impl Into<String>, native_id: impl Into<String>) -> Self {
        Self {
            adapter: adapter.into(),
            native_id: native_id.into(),
        }
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.adapter, self.native_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentCategory {
    Package,
    Application,
    Runtime,
}

impl ComponentCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Package => "Package",
            Self::Application => "Application",
            Self::Runtime => "Runtime",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallScope {
    User,
    System,
}

impl InstallScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::System => "system",
        }
    }
}

/// Where a pacman-managed package came from. `Foreign` means "not from a synced
/// repository" — usually AUR, but the label stays honest about what pacman says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum PacmanOrigin {
    /// `repo` stays `None` until a source reports it: `pacman -Qi` does not, and
    /// guessing a repository would be inventing a fact.
    Official {
        repo: Option<String>,
    },
    Foreign,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ComponentSource {
    Pacman { origin: PacmanOrigin },
    Aur { helper: String },
    Flatpak { scope: InstallScope },
}

impl ComponentSource {
    /// Human label combining manager and origin, e.g. `pacman / extra`.
    pub fn label(&self) -> String {
        match self {
            Self::Pacman {
                origin: PacmanOrigin::Official { repo: Some(repo) },
            } => {
                format!("pacman / {repo}")
            }
            Self::Pacman {
                origin: PacmanOrigin::Official { repo: None },
            } => "pacman / official".to_string(),
            Self::Pacman {
                origin: PacmanOrigin::Foreign,
            } => "pacman / foreign".to_string(),
            Self::Aur { helper } => format!("AUR / {helper}"),
            Self::Flatpak { scope } => format!("flatpak / {}", scope.label()),
        }
    }

    /// Group used by the sidebar to filter the inventory.
    pub fn group(&self) -> SourceGroup {
        match self {
            Self::Pacman {
                origin: PacmanOrigin::Official { .. },
            } => SourceGroup::Pacman,
            Self::Pacman {
                origin: PacmanOrigin::Foreign,
            } => SourceGroup::Foreign,
            Self::Aur { .. } => SourceGroup::Foreign,
            Self::Flatpak { .. } => SourceGroup::Flatpak,
        }
    }
}

/// Sidebar group. `Foreign` is pacman's own word for "not from a repository";
/// AUR components are a view over it, never a second component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceGroup {
    Pacman,
    Foreign,
    Flatpak,
}

impl SourceGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pacman => "Pacman",
            Self::Foreign => "AUR / foreign",
            Self::Flatpak => "Flatpak",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStatus {
    Installed,
    Orphan,
    Unknown,
}

/// How much a detection can be trusted, stated by the adapter that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionConfidence {
    /// The owning manager reported it directly.
    Authoritative,
    /// The manager reported it but the fact came from a remote or cached index.
    Medium,
    /// Derived from files or references rather than from the manager.
    Low,
}

impl DetectionConfidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Authoritative => "authoritative",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// What the application actually did to detect a fact, recorded so the user can
/// audit it instead of trusting a label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectionMethod {
    pub program: String,
    pub args: Vec<String>,
    pub note: Option<String>,
}

impl DetectionMethod {
    pub fn command(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            note: None,
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn label(&self) -> String {
        let mut out = format!("{} {}", self.program, self.args.join(" "));
        if let Some(note) = &self.note {
            out.push_str(" — ");
            out.push_str(note);
        }
        out.trim_end().to_string()
    }
}

/// Operations a component genuinely supports, decided by its owning adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentCapability {
    Inspect,
    Update,
    Remove,
}

impl ComponentCapability {
    pub fn label(self) -> &'static str {
        match self {
            Self::Inspect => "Inspect",
            Self::Update => "Update",
            Self::Remove => "Remove",
        }
    }
}

/// An extra line of detail whose label comes from the source itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailField {
    pub label: String,
    pub value: String,
}

impl DetailField {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component {
    pub id: ComponentId,
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub category: ComponentCategory,
    pub source: ComponentSource,
    pub status: ComponentStatus,
    pub confidence: DetectionConfidence,
    pub capabilities: Vec<ComponentCapability>,
    pub detection: DetectionMethod,
    pub installed_size: Option<u64>,
    pub location: Option<PathBuf>,
    /// Raw install date as printed by the source; never re-derived here.
    pub installed_at: Option<String>,
    pub depends_on: Vec<String>,
    pub required_by: Vec<String>,
    pub extra: Vec<DetailField>,
}

impl Component {
    pub fn supports(&self, capability: ComponentCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    /// Stable ordering for the list pane: name, then identity to break ties.
    pub fn sort_key(&self) -> (&str, &str) {
        (self.name.as_str(), self.id.native_id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes_without_fake_precision() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KiB");
        assert_eq!(format_size(152_333_516), "145.3 MiB");
        assert_eq!(format_size(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    #[test]
    fn renders_missing_values_as_no_disponible() {
        assert_eq!(shown(None), "No disponible");
        assert_eq!(shown(Some("1.2.3")), "1.2.3");
    }

    #[test]
    fn identical_names_in_different_ecosystems_are_distinct_ids() {
        let pacman = ComponentId::new("pacman", "firefox");
        let flatpak = ComponentId::new("flatpak", "firefox");
        assert_ne!(pacman, flatpak);
        assert_eq!(pacman.to_string(), "pacman:firefox");
    }

    #[test]
    fn foreign_packages_group_with_aur_without_duplicating_identity() {
        let source = ComponentSource::Pacman {
            origin: PacmanOrigin::Foreign,
        };
        assert_eq!(source.group(), SourceGroup::Foreign);
        let aur = ComponentSource::Aur {
            helper: "yay".to_string(),
        };
        assert_eq!(aur.group(), SourceGroup::Foreign);
    }

    #[test]
    fn detection_label_records_the_exact_command() {
        let method =
            DetectionMethod::command("pacman", vec!["-Q".into()]).with_note("installed packages");
        assert_eq!(method.label(), "pacman -Q — installed packages");
    }
}
