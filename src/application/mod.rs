//! Application layer: orchestration that the presentation layer calls into.
//!
//! Nothing here draws, and nothing here spawns a process directly — adapters do
//! that through the infrastructure layer.

pub mod operations;
mod registry;
pub mod search;

pub use operations::{ConfirmDecision, OperationRecord, PendingOperation, PrepareError, decide};
pub use registry::{
    AdapterFailure, AdapterRegistry, DuplicateIdentity, InventorySnapshot, ProviderError,
    RegistryError,
};
