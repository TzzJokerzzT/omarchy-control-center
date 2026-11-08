//! Adapter implementations.
//!
//! Each adapter is independent: it detects its own tool, refuses to run when the
//! tool is missing, and never assumes another adapter succeeded.

use std::sync::Arc;

use crate::application::{AdapterRegistry, RegistryError};
use crate::infrastructure::process::CommandRunner;

pub mod pacman;

/// Builds the registry for this run. New integrations are added here and nowhere
/// else, so the UI and the application layer never change when one is added.
pub fn build_registry(runner: Arc<dyn CommandRunner>) -> Result<AdapterRegistry, RegistryError> {
    AdapterRegistry::new(vec![Box::new(pacman::PacmanAdapter::new(Arc::clone(&runner)))])
}
