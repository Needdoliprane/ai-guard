//! Read-only batch observations tied to the real managed invocation/instance.
//! No public/serde constructor, grant, execution permit or terminal accounting.
use super::{
    ContainerObservation, ManagedRuntime, RuntimeState, process_state, valid_immutable_id,
};
use anyhow::{Result, ensure};
use std::sync::Arc;

struct ScopeInner {
    runtime: ManagedRuntime,
    target: String,
    created: Box<ContainerObservation>,
}
#[derive(Clone)]
pub(in crate::runtime) struct BatchObservationScope(Arc<ScopeInner>);

/// Born only after this scope's actual native inspection and full validation.
/// NonClone and nonSerde: an arbitrary RuntimeState/JSON cannot become this type.
pub(in crate::runtime) struct NativeBatchTerminal {
    scope: Arc<ScopeInner>,
    observed: Box<ContainerObservation>,
}

pub(in crate::runtime) struct NativeBatchObservation {
    scope: Arc<ScopeInner>,
    observed: ContainerObservation,
    terminal: Option<NativeBatchTerminal>,
}

impl ManagedRuntime {
    pub(in crate::runtime) fn batch_observation_scope(
        &self,
        target: &str,
        created: &ContainerObservation,
    ) -> Result<BatchObservationScope> {
        ensure!(
            valid_immutable_id(target) && created.id.as_deref() == Some(target),
            "batch observation scope does not match its native created instance"
        );
        created.validate_identity_allocation(created)?;
        Ok(BatchObservationScope(Arc::new(ScopeInner {
            runtime: self.clone(),
            target: target.to_owned(),
            created: Box::new(created.clone()),
        })))
    }
}
impl BatchObservationScope {
    pub(in crate::runtime) fn require_binding(
        &self,
        owner: &ManagedRuntime,
        target: &str,
        created: &ContainerObservation,
    ) -> Result<()> {
        ensure!(
            self.0.target == target
                && Arc::ptr_eq(&self.0.runtime.runtime.anchor, &owner.runtime.anchor)
                && self.0.runtime.binding.fingerprint() == owner.binding.fingerprint(),
            "batch observation scope belongs to another invocation or instance"
        );
        created.validate_identity_allocation(&self.0.created)?;
        Ok(())
    }
    pub(in crate::runtime) async fn observe(&self) -> Result<NativeBatchObservation> {
        // Same native authority/FD/inspect pipeline as inspect_snapshot_state.
        let observed = self
            .0
            .runtime
            .observe_id(&self.0.target, self.0.created.fingerprint_version())
            .await?;
        observed.validate_identity_allocation(&self.0.created)?;
        let terminal = if observed
            .validate_terminal(self.0.runtime.binding.kind, &self.0.created)
            .is_ok()
        {
            Some(NativeBatchTerminal {
                scope: self.0.clone(),
                observed: Box::new(observed.clone()),
            })
        } else {
            None
        };
        Ok(NativeBatchObservation {
            scope: self.0.clone(),
            observed,
            terminal,
        })
    }
}
impl NativeBatchObservation {
    pub(in crate::runtime) fn belongs_to(&self, expected: &BatchObservationScope) -> bool {
        Arc::ptr_eq(&self.scope, &expected.0)
    }
    pub(in crate::runtime) fn into_parts(
        self,
    ) -> (ContainerObservation, Option<NativeBatchTerminal>) {
        (self.observed, self.terminal)
    }
}
impl NativeBatchTerminal {
    pub(in crate::runtime) fn belongs_to(&self, expected: &BatchObservationScope) -> bool {
        Arc::ptr_eq(&self.scope, &expected.0)
    }
    pub(in crate::runtime) fn observed(&self) -> &ContainerObservation {
        &self.observed
    }
    pub(in crate::runtime) fn same_terminal(&self, observed: &ContainerObservation) -> bool {
        self.observed.id == observed.id
            && self.observed.configuration_fingerprint == observed.configuration_fingerprint
            && self.observed.process.started_at == observed.process.started_at
            && self.observed.process.finished_at == observed.process.finished_at
            && self.observed.process.exit_code == observed.process.exit_code
            && self.observed.process.oom_killed == observed.process.oom_killed
            && self.observed.restart_count == observed.restart_count
            && !observed.process.running.unwrap_or(true)
    }
}

pub(in crate::runtime) fn runtime_state(observed: &ContainerObservation) -> Result<RuntimeState> {
    process_state(observed)
}
