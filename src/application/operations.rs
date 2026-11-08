//! Operation lifecycle: prepare a plan, show it, confirm it, re-verify it, execute.
//!
//! The rules this module exists to enforce:
//! * a plan is the only input an executor accepts;
//! * a plan produced while the application is in read-only mode never exists;
//! * the plan is recomputed immediately before execution and compared by
//!   fingerprint, so a change between preview and confirmation cancels the action;
//! * a component that disappeared, or whose identity changed, cancels the action;
//! * the outcome keeps the real exit code, so a failure is never presented as
//!   success.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::application::{AdapterRegistry, ProviderError};
use crate::domain::{
    AdapterError, Component, ComponentId, OperationKind, OperationOutcome, OperationPlan,
    OperationRequest,
};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PrepareError {
    #[error("read-only mode: destructive actions are disabled in this session")]
    ReadOnly,

    #[error("{0}")]
    Provider(#[from] ProviderError),

    #[error("{0}")]
    Adapter(#[from] AdapterError),

    #[error("the plan changed since it was previewed; nothing was executed")]
    Drift,

    #[error("{0} is no longer installed; nothing was executed")]
    Vanished(String),

    #[error("this operation is blocked: {}", .0.join("; "))]
    Blocked(Vec<String>),
}

/// A plan the human is looking at, with the fingerprint captured when it was built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOperation {
    pub plan: OperationPlan,
    fingerprint: String,
}

impl PendingOperation {
    fn new(plan: OperationPlan) -> Self {
        let fingerprint = plan.fingerprint();
        Self { plan, fingerprint }
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The exact phrase the human must type. Never a generic "yes".
    pub fn confirmation_phrase(&self) -> &str {
        self.plan.confirmation_phrase()
    }

    pub fn confirmation_satisfied(&self, typed: &str) -> bool {
        typed.trim() == self.confirmation_phrase()
    }

    pub fn is_blocked(&self) -> bool {
        !self.plan.blocking.is_empty()
    }
}

/// What confirmation must lead to. Modelled as a value so the caller cannot
/// accidentally treat "not confirmed" as "execute anyway".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmDecision {
    Execute,
    Blocked(Vec<String>),
    PhraseMismatch { expected: String },
}

pub fn decide(pending: &PendingOperation, typed: &str) -> ConfirmDecision {
    if pending.is_blocked() {
        return ConfirmDecision::Blocked(pending.plan.blocking.clone());
    }
    if !pending.confirmation_satisfied(typed) {
        return ConfirmDecision::PhraseMismatch { expected: pending.confirmation_phrase().to_string() };
    }
    ConfirmDecision::Execute
}

/// Builds the preview. Refuses to build one in read-only mode, so no destructive
/// path can be reached from a session that asked not to have them.
pub fn prepare(
    registry: &AdapterRegistry,
    component: &Component,
    kind: OperationKind,
    read_only: bool,
) -> Result<PendingOperation, PrepareError> {
    if read_only {
        return Err(PrepareError::ReadOnly);
    }
    let provider = registry.operation_provider(component, kind)?;
    let request = OperationRequest::new(component.id.clone(), kind);
    let plan = provider.plan_operation(&request, component)?;
    Ok(PendingOperation::new(plan))
}

/// Recomputes the plan and refuses to continue if anything decision-relevant
/// changed. Returns the freshly read component so the caller routes the operation
/// exactly as it was routed while planning.
fn refreshed_and_verified(
    registry: &AdapterRegistry,
    pending: &PendingOperation,
    read_only: bool,
) -> Result<Component, PrepareError> {
    if read_only {
        return Err(PrepareError::ReadOnly);
    }

    let target: &ComponentId = &pending.plan.target;
    let refreshed = registry
        .reload(target)?
        .ok_or_else(|| PrepareError::Vanished(target.native_id.clone()))?;

    if refreshed.id != *target {
        return Err(PrepareError::Vanished(target.native_id.clone()));
    }

    let provider = registry.operation_provider(&refreshed, pending.plan.kind)?;
    let request = OperationRequest::new(target.clone(), pending.plan.kind);
    let fresh = provider.plan_operation(&request, &refreshed)?;

    if !fresh.blocking.is_empty() {
        return Err(PrepareError::Blocked(fresh.blocking));
    }
    if fresh.fingerprint() != pending.fingerprint || !fresh.is_executable() {
        return Err(PrepareError::Drift);
    }
    Ok(refreshed)
}

/// Recomputes the plan and refuses to continue if anything decision-relevant
/// changed. This is the check that makes a stale preview harmless.
pub fn revalidate(
    registry: &AdapterRegistry,
    pending: &PendingOperation,
    read_only: bool,
) -> Result<(), PrepareError> {
    refreshed_and_verified(registry, pending, read_only).map(|_| ())
}

/// Re-verifies and then executes exactly the confirmed plan.
pub fn execute_verified(
    registry: &AdapterRegistry,
    pending: &PendingOperation,
    read_only: bool,
) -> Result<OperationOutcome, PrepareError> {
    let refreshed = refreshed_and_verified(registry, pending, read_only)?;
    let provider = registry.operation_provider(&refreshed, pending.plan.kind)?;
    provider
        .execute(&pending.plan)
        .map_err(PrepareError::Adapter)
}

/// One line of the operation history. Contains no secrets and no configuration
/// content: only what is needed to audit who ran what and how it ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRecord {
    pub started_at_unix: u64,
    pub finished_at_unix: u64,
    pub adapter: String,
    pub component: String,
    pub component_name: String,
    pub kind: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub command: String,
    pub summary: String,
    pub error_tail: String,
    pub confirmed_by: String,
}

impl OperationRecord {
    pub fn from_outcome(plan: &OperationPlan, outcome: &OperationOutcome, started: u64) -> Self {
        Self {
            started_at_unix: started,
            finished_at_unix: now_unix(),
            adapter: plan.adapter.clone(),
            component: plan.target.native_id.clone(),
            component_name: plan.target_name.clone(),
            kind: plan.kind.label().to_string(),
            success: outcome.success,
            exit_code: outcome.exit_code,
            command: outcome.command_display.clone(),
            summary: outcome.summary.clone(),
            error_tail: outcome.stderr_tail.clone(),
            confirmed_by: format!("typed identifier {}", plan.confirmation_phrase()),
        }
    }

    /// RFC 3339 in UTC, derived from the stored timestamp.
    pub fn started_at(&self) -> String {
        format_timestamp(self.started_at_unix)
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// `YYYY-MM-DDTHH:MM:SSZ` from a Unix timestamp, using the civil-from-days
/// conversion so no date library is needed for one format.
pub fn format_timestamp(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let seconds_of_day = unix % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AdapterStatus, ComponentAdapter, ComponentCapability, ComponentSource, DetailField,
        PlannedCommand,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct State {
        installed: AtomicBool,
        dependents: AtomicUsize,
        blocked: AtomicBool,
        executed: AtomicUsize,
    }

    struct FakeAdapter {
        state: Arc<State>,
    }

    impl FakeAdapter {
        fn new() -> (Self, Arc<State>) {
            let state = Arc::new(State {
                installed: AtomicBool::new(true),
                ..State::default()
            });
            (
                Self {
                    state: Arc::clone(&state),
                },
                state,
            )
        }
    }

    impl ComponentAdapter for FakeAdapter {
        fn id(&self) -> &'static str {
            "pacman"
        }

        fn display_name(&self) -> &'static str {
            "Pacman"
        }

        fn description(&self) -> &'static str {
            "fake"
        }

        fn capabilities(&self) -> Vec<ComponentCapability> {
            vec![ComponentCapability::Inspect, ComponentCapability::Remove]
        }

        fn status(&self) -> AdapterStatus {
            AdapterStatus {
                id: "pacman".to_string(),
                display_name: "Pacman".to_string(),
                description: "fake".to_string(),
                availability: crate::domain::AdapterAvailability::Available,
                tool: None,
                tool_version: None,
                capabilities: self.capabilities(),
                commands: vec!["pacman -R <package>".to_string()],
                notes: Vec::new(),
            }
        }

        fn list(&self) -> Result<Vec<Component>, AdapterError> {
            Ok(Vec::new())
        }

        fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError> {
            if !self.state.installed.load(Ordering::SeqCst) {
                return Ok(None);
            }
            Ok(Some(component(native_id)))
        }

        fn supports(&self, _component: &Component, kind: OperationKind) -> bool {
            kind == OperationKind::Remove
        }

        fn plan_operation(
            &self,
            request: &OperationRequest,
            component: &Component,
        ) -> Result<OperationPlan, AdapterError> {
            let dependents = self.state.dependents.load(Ordering::SeqCst);
            let blocking = if self.state.blocked.load(Ordering::SeqCst) {
                vec!["required by other packages".to_string()]
            } else {
                Vec::new()
            };
            Ok(OperationPlan {
                adapter: "pacman".to_string(),
                target: request.target.clone(),
                target_name: component.name.clone(),
                target_version: component.version.clone(),
                kind: request.kind,
                commands: vec![PlannedCommand {
                    program: "pacman".to_string(),
                    args: vec!["-R".to_string(), request.target.native_id.clone()],
                    requires_root: true,
                    requires_tty: true,
                    rationale: "remove the package".to_string(),
                }],
                affected: (0..dependents)
                    .map(|index| format!("dependent-{index}"))
                    .collect(),
                blocking,
                unknown: vec!["files owned by the package were not listed".to_string()],
                notes: Vec::new(),
            })
        }

        fn execute(&self, plan: &OperationPlan) -> Result<OperationOutcome, AdapterError> {
            self.state.executed.fetch_add(1, Ordering::SeqCst);
            Ok(OperationOutcome {
                success: true,
                exit_code: Some(0),
                command_display: plan.commands[0].display(),
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                summary: "removed".to_string(),
            })
        }
    }

    fn component(native_id: &str) -> Component {
        Component {
            id: ComponentId::new("pacman", native_id),
            name: native_id.to_string(),
            version: Some("1.0-1".to_string()),
            description: None,
            category: crate::domain::ComponentCategory::Package,
            source: ComponentSource::Pacman {
                origin: crate::domain::PacmanOrigin::Foreign,
            },
            status: crate::domain::ComponentStatus::Installed,
            confidence: crate::domain::DetectionConfidence::Authoritative,
            capabilities: vec![ComponentCapability::Inspect, ComponentCapability::Remove],
            detection: crate::domain::DetectionMethod::command("pacman", vec!["-Q".to_string()]),
            installed_size: None,
            location: None,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: Vec::<DetailField>::new(),
        }
    }

    fn registry_with(adapter: FakeAdapter) -> AdapterRegistry {
        AdapterRegistry::new(vec![Box::new(adapter)]).expect("registry")
    }

    fn setup() -> (AdapterRegistry, Arc<State>) {
        let (adapter, state) = FakeAdapter::new();
        (registry_with(adapter), state)
    }

    #[test]
    fn read_only_mode_never_builds_a_plan() {
        let (registry, _state) = setup();
        let error =
            prepare(&registry, &component("btop"), OperationKind::Remove, true).unwrap_err();
        assert_eq!(error, PrepareError::ReadOnly);
    }

    #[test]
    fn a_prepared_plan_names_the_exact_command_and_the_phrase_to_type() {
        let (registry, _state) = setup();
        let pending =
            prepare(&registry, &component("btop"), OperationKind::Remove, false).expect("plan");
        assert_eq!(pending.plan.commands[0].display(), "pacman -R btop");
        assert_eq!(pending.confirmation_phrase(), "btop");
        assert!(pending.plan.requires_root());
        assert!(
            pending
                .plan
                .unknown
                .iter()
                .any(|fact| fact.contains("files"))
        );
    }

    #[test]
    fn the_confirmation_phrase_must_be_typed_exactly() {
        let (registry, _state) = setup();
        let pending =
            prepare(&registry, &component("btop"), OperationKind::Remove, false).expect("plan");
        assert!(pending.confirmation_satisfied("btop"));
        // Surrounding whitespace is tolerated: it cannot make the answer ambiguous.
        assert!(pending.confirmation_satisfied("  btop  "));
        assert!(pending.confirmation_satisfied("btop "));
        assert!(!pending.confirmation_satisfied(""));
        assert!(!pending.confirmation_satisfied("y"));
        assert!(!pending.confirmation_satisfied("yes"));
        assert!(!pending.confirmation_satisfied("btopx"));
    }

    #[test]
    fn a_plan_that_changed_after_the_preview_is_refused() {
        let (registry, state) = setup();
        let pending =
            prepare(&registry, &component("btop"), OperationKind::Remove, false).expect("plan");

        // The preview showed no dependents; the system now reports two.
        state.dependents.store(2, Ordering::SeqCst);

        let error = execute_verified(&registry, &pending, false).unwrap_err();
        assert_eq!(error, PrepareError::Drift);
        assert_eq!(
            state.executed.load(Ordering::SeqCst),
            0,
            "nothing may run on drift"
        );
    }

    #[test]
    fn a_component_that_vanished_is_refused() {
        let (registry, state) = setup();
        let pending =
            prepare(&registry, &component("btop"), OperationKind::Remove, false).expect("plan");

        state.installed.store(false, Ordering::SeqCst);

        let error = execute_verified(&registry, &pending, false).unwrap_err();
        assert_eq!(error, PrepareError::Vanished("btop".to_string()));
        assert_eq!(state.executed.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_unchanged_plan_executes_exactly_once() {
        let (registry, state) = setup();
        let pending =
            prepare(&registry, &component("btop"), OperationKind::Remove, false).expect("plan");
        let outcome = execute_verified(&registry, &pending, false).expect("execution");
        assert!(outcome.success);
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(state.executed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn execution_in_read_only_mode_is_refused_even_with_a_valid_plan() {
        let (registry, _state) = setup();
        let pending =
            prepare(&registry, &component("btop"), OperationKind::Remove, false).expect("plan");
        assert_eq!(
            execute_verified(&registry, &pending, true).unwrap_err(),
            PrepareError::ReadOnly
        );
    }

    #[test]
    fn an_unconfirmed_phrase_never_decides_to_execute() {
        let (registry, _state) = setup();
        let pending = prepare(&registry, &component("btop"), OperationKind::Remove, false).expect("plan");

        assert_eq!(decide(&pending, "btop"), ConfirmDecision::Execute);
        for wrong in ["", "y", "yes", "BT", "btop2", "btop now"] {
            assert_eq!(
                decide(&pending, wrong),
                ConfirmDecision::PhraseMismatch { expected: "btop".to_string() },
                "{wrong:?} must not confirm"
            );
        }
    }

    #[test]
    fn a_blocked_plan_decides_blocked_even_with_the_right_phrase() {
        let (registry, state) = setup();
        state.blocked.store(true, Ordering::SeqCst);
        let pending = prepare(&registry, &component("btop"), OperationKind::Remove, false).expect("plan");
        assert!(pending.is_blocked());
        assert!(matches!(decide(&pending, "btop"), ConfirmDecision::Blocked(_)));
    }

    #[test]
    fn formats_utc_timestamps_without_a_date_library() {
        assert_eq!(format_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_timestamp(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(format_timestamp(1_709_164_800), "2024-02-29T00:00:00Z");
    }
}
