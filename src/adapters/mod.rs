//! Adapter implementations.
//!
//! Each adapter is independent: it detects its own tool, refuses to run when the
//! tool is missing, and never assumes another adapter succeeded.

use std::sync::Arc;

use crate::application::{AdapterRegistry, RegistryError};
use crate::infrastructure::process::CommandRunner;
use crate::infrastructure::reclaim::Reclaimer;

pub mod aur;
pub mod cargo_tools;
pub mod flatpak;
pub mod hyprland;
pub mod lazy_nvim;
pub mod omarchy;
pub mod pacman;
pub mod python_tools;
pub mod resources;
pub mod services;
pub mod zsh;

/// Builds the registry for this run. New integrations are added here and nowhere
/// else, so the UI and the application layer never change when one is added.
///
/// `pacman` comes first because it owns every pacman-managed component;
/// `omarchy` owns the shell plugin catalogue; `aur` contributes metadata and
/// routing for the foreign packages it owns rather than listing them again
/// (decision D1); `flatpak` is optional by design (decision D6) and reports
/// itself unavailable when its tool is missing.
pub fn build_registry(
    runner: Arc<dyn CommandRunner>,
    reclaimer: Arc<dyn Reclaimer>,
) -> Result<AdapterRegistry, RegistryError> {
    AdapterRegistry::new(vec![
        Box::new(pacman::PacmanAdapter::new(Arc::clone(&runner))),
        Box::new(omarchy::OmarchyPluginAdapter::new(
            Arc::clone(&runner),
            Arc::clone(&reclaimer),
        )),
        Box::new(aur::AurAdapter::new(Arc::clone(&runner))),
        Box::new(flatpak::FlatpakAdapter::new(Arc::clone(&runner))),
        Box::new(lazy_nvim::LazyNvimAdapter::new()),
        Box::new(services::ServicesAdapter::new(Arc::clone(&runner))),
        Box::new(zsh::ZshAdapter::new()),
        Box::new(hyprland::HyprlandAdapter::new(
            Arc::clone(&runner),
            Arc::clone(&reclaimer),
        )),
        Box::new(cargo_tools::CargoToolsAdapter::new(Arc::clone(&runner))),
        Box::new(python_tools::PythonToolsAdapter::new(Arc::clone(&runner))),
        Box::new(resources::ResourcesAdapter::new(Arc::clone(&runner))),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::test_support::FakeRunner;

    /// Adapters whose availability comes from files, not from a tool in `PATH`.
    const FILE_BASED: &[&str] = &["lazy", "zsh", "hyprland", "resources"];

    #[test]
    fn registers_every_available_integration() {
        let registry = build_registry(
            Arc::new(FakeRunner::new()),
            Arc::new(crate::infrastructure::reclaim::FileReclaimer::new(
                std::env::temp_dir(),
            )),
        )
        .expect("registry");
        for expected in ["pacman", "omarchy", "aur", "flatpak", "lazy", "systemd"] {
            assert!(
                registry.get(expected).is_some(),
                "{expected} must be registered"
            );
        }
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn a_machine_without_any_tool_degrades_instead_of_failing() {
        // Every tool is missing. The adapters that need one report themselves
        // unavailable and contribute nothing; the ones that read files (lazy.nvim, zsh,
        // resources) keep working, because their facts never came from a tool. Nothing
        // may fail, and no adapter may take the run down with it.
        let registry = build_registry(
            Arc::new(FakeRunner::new()),
            Arc::new(crate::infrastructure::reclaim::FileReclaimer::new(
                std::env::temp_dir(),
            )),
        )
        .expect("registry");
        let snapshot = registry.load_all();
        for status in &snapshot.statuses {
            if FILE_BASED.contains(&status.id.as_str()) {
                continue;
            }
            assert!(
                !status.available(),
                "{} needs a tool and has none here",
                status.id
            );
        }
        for component in &snapshot.components {
            assert!(
                FILE_BASED.contains(&component.id.adapter.as_str()),
                "only file-based adapters can list anything without a tool: {}",
                component.id
            );
        }
        assert!(
            snapshot.failures.is_empty(),
            "missing tools are not failures: {:?}",
            snapshot.failures
        );
        assert_eq!(snapshot.statuses.len(), 11);
    }
}
