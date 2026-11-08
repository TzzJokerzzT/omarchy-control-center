//! The adapter contract. Adapters are synchronous on purpose: every one of them
//! is a `Command`-in / typed-model-out translation that must stay testable
//! without a runtime, and the application layer decides when to run it in the
//! background.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::domain::{
    AdapterError, Component, ComponentCapability, OperationKind, OperationOutcome, OperationPlan,
    OperationRequest,
};

/// Whether an integration can actually be used on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AdapterAvailability {
    Available,
    Unavailable { reason: String },
}

impl AdapterAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// Everything the UI needs to explain an integration, including why it is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterStatus {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub availability: AdapterAvailability,
    pub tool: Option<PathBuf>,
    pub tool_version: Option<String>,
    pub capabilities: Vec<ComponentCapability>,
    /// Commands this adapter may execute, for auditing and documentation.
    pub commands: Vec<String>,
    pub notes: Vec<String>,
}

impl AdapterStatus {
    pub fn available(&self) -> bool {
        self.availability.is_available()
    }

    pub fn unavailable_reason(&self) -> Option<&str> {
        match &self.availability {
            AdapterAvailability::Available => None,
            AdapterAvailability::Unavailable { reason } => Some(reason),
        }
    }
}

pub trait ComponentAdapter: Send + Sync {
    /// Stable identifier used in every component identity.
    fn id(&self) -> &'static str;

    fn display_name(&self) -> &'static str;

    fn description(&self) -> &'static str;

    fn capabilities(&self) -> Vec<ComponentCapability>;

    /// Availability, resolved tool and every command this adapter may run.
    fn status(&self) -> AdapterStatus;

    /// Full list of components this adapter owns.
    fn list(&self) -> Result<Vec<Component>, AdapterError>;

    /// Refresh one component from the source. `None` means it no longer exists,
    /// which must cancel any pending operation instead of proceeding.
    fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError>;

    /// Whether this adapter is the one that must run `kind` for `component`.
    /// Exactly one adapter may claim a given (component, kind) pair; the
    /// registry reports an ambiguity instead of guessing.
    fn supports(&self, component: &Component, kind: OperationKind) -> bool;

    /// Build the plan shown before confirming. Recomputed right before execution
    /// so the fingerprint can detect drift.
    fn plan_operation(
        &self,
        request: &OperationRequest,
        component: &Component,
    ) -> Result<OperationPlan, AdapterError>;

    /// Execute exactly the commands in `plan`. Never re-derives arguments.
    fn execute(&self, plan: &OperationPlan) -> Result<OperationOutcome, AdapterError>;
}
