//! Adapter implementations.
//!
//! Each adapter is independent: it detects its own tool, refuses to run when the
//! tool is missing, and never assumes another adapter succeeded.

use crate::application::{AdapterRegistry, RegistryError};

/// Builds the registry for this run. New integrations are added here and nowhere
/// else, so the UI and the application layer never change when one is added.
pub fn build_registry() -> Result<AdapterRegistry, RegistryError> {
    AdapterRegistry::new(Vec::new())
}
