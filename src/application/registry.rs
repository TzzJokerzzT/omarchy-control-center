//! Adapter registry: the only place that knows which integrations exist.
//!
//! Isolation rules enforced here:
//! * an adapter that is unavailable is recorded as such and contributes nothing;
//! * an adapter that errors or panics never stops the other adapters;
//! * two adapters may not describe the same identity;
//! * exactly one adapter may claim an operation, otherwise the request is refused.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{
    AdapterError, AdapterStatus, Component, ComponentId, ComponentAdapter, OperationKind,
};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RegistryError {
    #[error("two adapters registered the same id: {id}")]
    DuplicateAdapterId { id: String },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProviderError {
    #[error("no adapter supports {} for {}", kind.label(), component)]
    NoProvider { component: String, kind: OperationKind },
    #[error("{} for {} is ambiguous: {adapters}", kind.label(), component)]
    Ambiguous { component: String, kind: OperationKind, adapters: String },
}

/// A failure attributed to one adapter and one stage. Never fatal for the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterFailure {
    pub adapter: String,
    pub stage: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateIdentity {
    pub id: ComponentId,
    pub kept: String,
    pub rejected: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InventorySnapshot {
    pub components: Vec<Component>,
    pub statuses: Vec<AdapterStatus>,
    pub failures: Vec<AdapterFailure>,
    pub duplicates: Vec<DuplicateIdentity>,
}

impl InventorySnapshot {
    pub fn component(&self, id: &ComponentId) -> Option<&Component> {
        self.components.iter().find(|component| &component.id == id)
    }

    pub fn failure_count(&self) -> usize {
        self.failures.len() + self.duplicates.len()
    }
}

pub struct AdapterRegistry {
    adapters: Vec<Box<dyn ComponentAdapter>>,
}

impl AdapterRegistry {
    pub fn new(adapters: Vec<Box<dyn ComponentAdapter>>) -> Result<Self, RegistryError> {
        let mut seen: BTreeMap<&'static str, ()> = BTreeMap::new();
        for adapter in &adapters {
            if seen.insert(adapter.id(), ()).is_some() {
                return Err(RegistryError::DuplicateAdapterId { id: adapter.id().to_string() });
            }
        }
        Ok(Self { adapters })
    }

    pub fn adapters(&self) -> &[Box<dyn ComponentAdapter>] {
        &self.adapters
    }

    pub fn get(&self, id: &str) -> Option<&dyn ComponentAdapter> {
        self.adapters.iter().find(|adapter| adapter.id() == id).map(|adapter| adapter.as_ref())
    }

    pub fn statuses(&self) -> Vec<AdapterStatus> {
        self.adapters.iter().map(|adapter| guarded_status(adapter.as_ref())).collect()
    }

    /// Loads every adapter, isolating failures, panics and duplicate identities.
    pub fn load_all(&self) -> InventorySnapshot {
        let mut snapshot = InventorySnapshot::default();
        let mut owners: BTreeMap<ComponentId, String> = BTreeMap::new();

        for adapter in &self.adapters {
            let Some(status) = guarded_status_opt(adapter.as_ref()) else {
                snapshot.failures.push(AdapterFailure {
                    adapter: adapter.id().to_string(),
                    stage: "status",
                    message: "adapter panicked while reporting its status".to_string(),
                });
                continue;
            };
            let available = status.available();
            snapshot.statuses.push(status);
            if !available {
                continue;
            }

            match catch_unwind(AssertUnwindSafe(|| adapter.list())) {
                Ok(Ok(components)) => {
                    for component in components {
                        match owners.get(&component.id) {
                            Some(kept) => snapshot.duplicates.push(DuplicateIdentity {
                                id: component.id.clone(),
                                kept: kept.clone(),
                                rejected: adapter.id().to_string(),
                            }),
                            None => {
                                owners.insert(component.id.clone(), adapter.id().to_string());
                                snapshot.components.push(component);
                            }
                        }
                    }
                }
                Ok(Err(error)) => snapshot.failures.push(AdapterFailure {
                    adapter: adapter.id().to_string(),
                    stage: "list",
                    message: error.to_string(),
                }),
                Err(_) => snapshot.failures.push(AdapterFailure {
                    adapter: adapter.id().to_string(),
                    stage: "list",
                    message: "adapter panicked while listing components".to_string(),
                }),
            }
        }

        snapshot.components.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
        snapshot
    }

    /// Re-reads one component from its source. `None` means it disappeared, which
    /// must cancel a pending operation instead of proceeding.
    pub fn reload(&self, id: &ComponentId) -> Result<Option<Component>, AdapterError> {
        let adapter = self.get(&id.adapter).ok_or_else(|| {
            AdapterError::unsupported(id.adapter.clone(), "inspect (unknown adapter)")
        })?;
        match catch_unwind(AssertUnwindSafe(|| adapter.details(&id.native_id))) {
            Ok(result) => result,
            Err(_) => Err(AdapterError::Other(format!(
                "{} panicked while refreshing {}",
                id.adapter, id.native_id
            ))),
        }
    }

    /// Finds the single adapter allowed to run `kind` for `component`.
    pub fn operation_provider(
        &self,
        component: &Component,
        kind: OperationKind,
    ) -> Result<&dyn ComponentAdapter, ProviderError> {
        let claims: Vec<&dyn ComponentAdapter> = self
            .adapters
            .iter()
            .filter(|adapter| adapter.supports(component, kind))
            .map(|adapter| adapter.as_ref())
            .collect();

        match claims.len() {
            0 => Err(ProviderError::NoProvider {
                component: component.id.to_string(),
                kind,
            }),
            1 => Ok(claims[0]),
            _ => Err(ProviderError::Ambiguous {
                component: component.id.to_string(),
                kind,
                adapters: claims.iter().map(|adapter| adapter.id()).collect::<Vec<_>>().join(", "),
            }),
        }
    }
}

fn guarded_status(adapter: &dyn ComponentAdapter) -> AdapterStatus {
    guarded_status_opt(adapter).unwrap_or_else(|| AdapterStatus {
        id: adapter.id().to_string(),
        display_name: adapter.display_name().to_string(),
        description: adapter.description().to_string(),
        availability: crate::domain::AdapterAvailability::Unavailable {
            reason: "adapter panicked while reporting its status".to_string(),
        },
        tool: None,
        tool_version: None,
        capabilities: Vec::new(),
        commands: Vec::new(),
        notes: Vec::new(),
    })
}

fn guarded_status_opt(adapter: &dyn ComponentAdapter) -> Option<AdapterStatus> {
    catch_unwind(AssertUnwindSafe(|| adapter.status())).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AdapterAvailability, ComponentCapability, OperationOutcome, OperationPlan, OperationRequest,
        PacmanOrigin, ComponentCategory, ComponentSource, ComponentStatus, DetectionConfidence,
        DetectionMethod,
    };

    struct Fake {
        id: &'static str,
        availability: AdapterAvailability,
        behaviour: Behaviour,
        claims_remove: bool,
        owns_foreign: bool,
    }

    enum Behaviour {
        Empty,
        /// Emits one component owned by this adapter.
        One,
        /// Emits a component claiming to belong to another adapter, which is how a
        /// second adapter can collide on an identity it does not own.
        ClaimsForeignIdentity,
        Fails,
        Panics,
    }

    impl Fake {
        fn new(id: &'static str) -> Self {
            Self {
                id,
                availability: AdapterAvailability::Available,
                behaviour: Behaviour::Empty,
                claims_remove: true,
                owns_foreign: false,
            }
        }

        fn behaviour(mut self, behaviour: Behaviour) -> Self {
            self.behaviour = behaviour;
            self
        }

        fn unavailable(mut self, reason: &str) -> Self {
            self.availability =
                AdapterAvailability::Unavailable { reason: reason.to_string() };
            self
        }
    }

    fn sample(adapter: &'static str, name: &str) -> Component {
        Component {
            id: ComponentId::new(adapter, name),
            name: name.to_string(),
            version: Some("1.0-1".to_string()),
            description: None,
            category: ComponentCategory::Package,
            source: ComponentSource::Pacman { origin: PacmanOrigin::Foreign },
            status: ComponentStatus::Installed,
            confidence: DetectionConfidence::Authoritative,
            capabilities: vec![ComponentCapability::Inspect],
            detection: DetectionMethod::command("pacman", vec!["-Q".to_string()]),
            installed_size: None,
            location: None,
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: Vec::new(),
        }
    }

    impl ComponentAdapter for Fake {
        fn id(&self) -> &'static str {
            self.id
        }

        fn display_name(&self) -> &'static str {
            self.id
        }

        fn description(&self) -> &'static str {
            "fake adapter"
        }

        fn capabilities(&self) -> Vec<ComponentCapability> {
            vec![ComponentCapability::Inspect]
        }

        fn status(&self) -> AdapterStatus {
            AdapterStatus {
                id: self.id.to_string(),
                display_name: self.id.to_string(),
                description: "fake adapter".to_string(),
                availability: self.availability.clone(),
                tool: None,
                tool_version: None,
                capabilities: vec![ComponentCapability::Inspect],
                commands: vec!["fake --list".to_string()],
                notes: Vec::new(),
            }
        }

        fn list(&self) -> Result<Vec<Component>, AdapterError> {
            match self.behaviour {
                Behaviour::Empty => Ok(Vec::new()),
                Behaviour::One => Ok(vec![sample(self.id, "shared-name")]),
                Behaviour::ClaimsForeignIdentity => Ok(vec![sample("pacman", "shared-name")]),
                Behaviour::Fails => Err(AdapterError::Other("boom".to_string())),
                Behaviour::Panics => panic!("adapter exploded"),
            }
        }

        fn details(&self, native_id: &str) -> Result<Option<Component>, AdapterError> {
            Ok(Some(sample(self.id, native_id)))
        }

        fn supports(&self, component: &Component, kind: OperationKind) -> bool {
            if !self.claims_remove || kind != OperationKind::Remove {
                return false;
            }
            let foreign = matches!(
                component.source,
                ComponentSource::Pacman { origin: PacmanOrigin::Foreign }
            );
            if self.owns_foreign { foreign } else { !foreign }
        }

        fn plan_operation(
            &self,
            _request: &OperationRequest,
            _component: &Component,
        ) -> Result<OperationPlan, AdapterError> {
            Err(AdapterError::unsupported(self.id, "plan"))
        }

        fn execute(&self, _plan: &OperationPlan) -> Result<OperationOutcome, AdapterError> {
            Err(AdapterError::unsupported(self.id, "execute"))
        }
    }

    fn registry(fakes: Vec<Fake>) -> AdapterRegistry {
        AdapterRegistry::new(fakes.into_iter().map(|fake| Box::new(fake) as Box<dyn ComponentAdapter>).collect())
            .expect("registry")
    }

    #[test]
    fn rejects_two_adapters_with_the_same_id() {
        let error = AdapterRegistry::new(vec![
            Box::new(Fake::new("pacman")) as Box<dyn ComponentAdapter>,
            Box::new(Fake::new("pacman")) as Box<dyn ComponentAdapter>,
        ])
        .err()
        .expect("duplicate id must be refused");
        assert_eq!(error, RegistryError::DuplicateAdapterId { id: "pacman".to_string() });
    }

    #[test]
    fn a_missing_tool_does_not_stop_the_other_adapters() {
        let snapshot = registry(vec![
            Fake::new("pacman").behaviour(Behaviour::One),
            Fake::new("flatpak").unavailable("flatpak was not found"),
        ])
        .load_all();

        assert_eq!(snapshot.components.len(), 1);
        assert!(snapshot.failures.is_empty(), "unavailable is not a failure");
        let flatpak = snapshot.statuses.iter().find(|status| status.id == "flatpak").expect("status");
        assert!(!flatpak.available());
        assert_eq!(flatpak.unavailable_reason(), Some("flatpak was not found"));
    }

    #[test]
    fn an_erroring_adapter_does_not_prevent_the_others_from_loading() {
        let snapshot = registry(vec![
            Fake::new("broken").behaviour(Behaviour::Fails),
            Fake::new("pacman").behaviour(Behaviour::One),
        ])
        .load_all();

        assert_eq!(snapshot.components.len(), 1);
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(snapshot.failures[0].adapter, "broken");
        assert_eq!(snapshot.failures[0].stage, "list");
    }

    #[test]
    fn a_panicking_adapter_is_contained() {
        let snapshot = registry(vec![
            Fake::new("explosive").behaviour(Behaviour::Panics),
            Fake::new("pacman").behaviour(Behaviour::One),
        ])
        .load_all();

        assert_eq!(snapshot.components.len(), 1);
        assert_eq!(snapshot.failures.len(), 1);
        assert!(snapshot.failures[0].message.contains("panicked"));
    }

    #[test]
    fn the_same_identity_claimed_twice_is_reported_not_duplicated() {
        let snapshot = registry(vec![
            Fake::new("pacman").behaviour(Behaviour::One),
            Fake::new("aur").behaviour(Behaviour::ClaimsForeignIdentity),
        ])
        .load_all();

        assert_eq!(snapshot.components.len(), 1);
        assert_eq!(snapshot.duplicates.len(), 1);
        assert_eq!(snapshot.duplicates[0].id.to_string(), "pacman:shared-name");
        assert_eq!(snapshot.duplicates[0].kept, "pacman");
        assert_eq!(snapshot.duplicates[0].rejected, "aur");
    }

    #[test]
    fn the_same_name_in_two_ecosystems_is_two_components() {
        let snapshot = registry(vec![
            Fake::new("pacman").behaviour(Behaviour::One),
            Fake::new("flatpak").behaviour(Behaviour::One),
        ])
        .load_all();

        assert_eq!(snapshot.components.len(), 2);
        assert!(snapshot.duplicates.is_empty());
    }

    #[test]
    fn an_operation_has_exactly_one_provider() {
        let with_aur = registry(vec![
            Fake::new("pacman"),
            Fake {
                id: "aur",
                availability: AdapterAvailability::Available,
                behaviour: Behaviour::Empty,
                claims_remove: true,
                owns_foreign: true,
            },
        ]);
        let foreign = sample("pacman", "some-aur-pkg");
        let provider = with_aur.operation_provider(&foreign, OperationKind::Remove).expect("provider");
        assert_eq!(provider.id(), "aur");

        let ambiguous = registry(vec![Fake::new("pacman"), Fake::new("flatpak")]);
        let official = Component {
            source: ComponentSource::Pacman { origin: PacmanOrigin::Official { repo: Some("extra".into()) } },
            ..sample("pacman", "btop")
        };
        let error = ambiguous.operation_provider(&official, OperationKind::Remove).err().expect("ambiguity");
        assert!(matches!(error, ProviderError::Ambiguous { .. }));
    }

    #[test]
    fn no_provider_means_the_action_is_not_offered() {
        let no_remove = registry(vec![Fake { claims_remove: false, ..Fake::new("pacman") }]);
        let error = no_remove
            .operation_provider(&sample("pacman", "btop"), OperationKind::Remove)
            .err()
            .expect("no provider");
        assert!(matches!(error, ProviderError::NoProvider { .. }));
    }
}
