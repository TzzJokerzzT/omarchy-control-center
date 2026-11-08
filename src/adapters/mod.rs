//! Adapter implementations.
//!
//! Each adapter is independent: it detects its own tool, refuses to run when the
//! tool is missing, and never assumes another adapter succeeded.

use std::sync::Arc;

use crate::application::{AdapterRegistry, RegistryError};
use crate::infrastructure::process::CommandRunner;

pub mod aur;
pub mod flatpak;
pub mod pacman;

/// Builds the registry for this run. New integrations are added here and nowhere
/// else, so the UI and the application layer never change when one is added.
///
/// `pacman` comes first because it owns every pacman-managed component;
/// `aur` contributes metadata and routing for the foreign ones it owns rather
/// than listing them again (decision D1); `flatpak` is optional by design
/// (decision D6) and reports itself unavailable when its tool is missing.
pub fn build_registry(runner: Arc<dyn CommandRunner>) -> Result<AdapterRegistry, RegistryError> {
    AdapterRegistry::new(vec![
        Box::new(pacman::PacmanAdapter::new(Arc::clone(&runner))),
        Box::new(aur::AurAdapter::new(Arc::clone(&runner))),
        Box::new(flatpak::FlatpakAdapter::new(Arc::clone(&runner))),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::test_support::FakeRunner;

    #[test]
    fn registers_pacman_aur_and_flatpak() {
        let registry = build_registry(Arc::new(FakeRunner::new())).expect("registry");
        for expected in ["pacman", "aur", "flatpak"] {
            assert!(
                registry.get(expected).is_some(),
                "{expected} must be registered"
            );
        }
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn a_machine_without_any_tool_degrades_instead_of_failing() {
        // Every tool is missing: the inventory is empty and explained, not broken.
        let registry = build_registry(Arc::new(FakeRunner::new())).expect("registry");
        let snapshot = registry.load_all();
        assert!(snapshot.components.is_empty());
        assert!(
            snapshot.failures.is_empty(),
            "missing tools are not failures: {:?}",
            snapshot.failures
        );
        assert!(snapshot.statuses.iter().all(|status| !status.available()));
        assert_eq!(snapshot.statuses.len(), 3);
    }
}
