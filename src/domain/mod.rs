//! Domain layer: models and rules that never spawn a process or touch the terminal.

mod adapter;
mod component;
mod error;
mod operation;

pub use adapter::{AdapterAvailability, AdapterStatus, ComponentAdapter};
pub use component::{
    Component, ComponentCapability, ComponentCategory, ComponentId, ComponentSource,
    ComponentStatus, DetailField, DetectionConfidence, DetectionMethod, InstallScope, PacmanOrigin,
    SourceGroup, UNAVAILABLE, format_size, shown,
};
pub use error::AdapterError;
pub use operation::{
    OperationKind, OperationOutcome, OperationPlan, OperationRequest, PlannedCommand,
};
