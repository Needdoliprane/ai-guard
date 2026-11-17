//! Bounded custody of original native causes. Counts are observations only;
//! the actual returned error/proof or live IO owner remains retained here.
use crate::runtime::{
    NativeCollected, NativeCollectionOutcome, NativeNoChild, NativeRuntimeOutcome,
    NativeUncollectedCall,
};
use std::{
    any::Any,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
pub(super) struct CauseTarget {
    _arm_id: Option<String>,
    _snapshot_hash: Option<String>,
    // Catch-up changes targets. Keep the original validated request beside
    // its retained cause; an identifier or diagnostic is not an active owner.
    _retained_request: Option<Arc<dyn Any + Send + Sync>>,
}
impl CauseTarget {
    pub(super) fn context() -> Self {
        Self {
            _arm_id: None,
            _snapshot_hash: None,
            _retained_request: None,
        }
    }
    pub(super) fn fixed(arm_id: &str, snapshot_hash: &str) -> Self {
        Self {
            _arm_id: Some(arm_id.into()),
            _snapshot_hash: Some(snapshot_hash.into()),
            _retained_request: None,
        }
    }
    pub(super) fn retained<T: Any + Send + Sync>(
        arm_id: &str,
        snapshot_hash: &str,
        request: Arc<T>,
    ) -> Self {
        Self {
            _arm_id: Some(arm_id.into()),
            _snapshot_hash: Some(snapshot_hash.into()),
            _retained_request: Some(request),
        }
    }
}

pub(super) enum ReturnedProof {
    NoChild { _native: NativeNoChild },
    Collected { _native: NativeCollected },
}
pub(super) struct Settled<T> {
    pub(super) value: T,
    pub(super) proof: ReturnedProof,
}
enum CauseProof {
    Returned { _proof: ReturnedProof },
    // An actual native context failure, not a control collection proof. It
    // never grants reusable command capacity.
    ContextFailure,
}
struct RetainedCause {
    _cause: anyhow::Error,
    _target: CauseTarget,
    _proof: CauseProof,
}
struct PendingCause {
    call: Arc<Mutex<NativeUncollectedCall>>,
    _target: CauseTarget,
}
struct ObservedStop {
    _target: CauseTarget,
    _observation: crate::runtime::ContainerObservation,
}
#[derive(Default)]
struct Causes {
    first: Option<RetainedCause>,
    last_settled: Option<RetainedCause>,
    pending: Option<PendingCause>,
    last_observed_stop: Option<ObservedStop>,
    native_exit_cause: Option<Box<dyn Any + Send>>,
    omitted_settled: u64,
    omissions_exact: bool,
    custody_unknown: bool,
    native_call_in_flight: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct FailureFacts {
    pub(super) retained_causes: u32,
    pub(super) omitted_settled: u64,
    pub(super) omissions_exact: bool,
    pub(super) uncollected_io: bool,
}
impl Default for FailureFacts {
    fn default() -> Self {
        Self {
            retained_causes: 0,
            omitted_settled: 0,
            omissions_exact: true,
            uncollected_io: false,
        }
    }
}
impl FailureFacts {
    pub(super) fn merge(&mut self, other: Self) {
        self.omissions_exact &= other.omissions_exact;
        self.retained_causes = self
            .retained_causes
            .checked_add(other.retained_causes)
            .unwrap_or_else(|| {
                self.omissions_exact = false;
                u32::MAX
            });
        self.omitted_settled = self
            .omitted_settled
            .checked_add(other.omitted_settled)
            .unwrap_or_else(|| {
                self.omissions_exact = false;
                u64::MAX
            });
        self.uncollected_io |= other.uncollected_io;
    }
}

pub(super) struct NativeStopFailures(Mutex<Causes>);
impl Default for NativeStopFailures {
    fn default() -> Self {
        Self(Mutex::new(Causes {
            omissions_exact: true,
            ..Causes::default()
        }))
    }
}
impl NativeStopFailures {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub(super) fn facts(&self) -> FailureFacts {
        let causes = self.0.lock().unwrap_or_else(|p| p.into_inner());
        FailureFacts {
            retained_causes: u32::from(causes.first.is_some())
                + u32::from(causes.last_settled.is_some())
                + u32::from(causes.pending.is_some())
                + u32::from(causes.native_exit_cause.is_some()),
            omitted_settled: causes.omitted_settled,
            omissions_exact: causes.omissions_exact && !causes.custody_unknown,
            uncollected_io: causes.native_call_in_flight
                || causes.pending.is_some()
                || causes.custody_unknown,
        }
    }
    #[cfg(test)]
    pub(super) fn inspect_first_for_test<R>(
        &self,
        inspect: impl FnOnce(Option<&anyhow::Error>) -> R,
    ) -> R {
        let causes = self.0.lock().unwrap_or_else(|p| p.into_inner());
        inspect(causes.first.as_ref().map(|cause| &cause._cause))
    }

    fn retain(&self, target: CauseTarget, cause: anyhow::Error, proof: CauseProof) {
        // Recover the actual retained state on poison rather than dropping a
        // cause/owner because a health mutex failed. This cannot grant a start.
        let mut causes = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let retained = RetainedCause {
            _cause: cause,
            _target: target,
            _proof: proof,
        };
        if causes.first.is_none() {
            causes.first = Some(retained);
        } else {
            if causes.last_settled.is_some() {
                match causes.omitted_settled.checked_add(1) {
                    Some(next) => causes.omitted_settled = next,
                    None => causes.omissions_exact = false,
                }
            }
            // Only positively settled errors or an actual pre-invocation
            // constructor failure reach this field. Replacing a collected
            // diagnostic never replaces an uncollected owner.
            causes.last_settled = Some(retained);
        }
    }
    pub(super) fn context_failure<E>(&self, target: CauseTarget, error: E)
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.retain(
            target,
            anyhow::Error::new(error),
            CauseProof::ContextFailure,
        );
    }
    pub(super) fn native_thread_failed(&self, payload: Box<dyn Any + Send>) {
        let mut causes = self.0.lock().unwrap_or_else(|p| p.into_inner());
        causes.custody_unknown = true;
        // One actual thread can return one failure. Never format or drop its
        // original payload: panic_any may itself own affine resources.
        if causes.native_exit_cause.is_some() {
            drop(causes);
            loop {
                std::thread::park();
            }
        }
        causes.native_exit_cause = Some(payload);
    }
    pub(super) fn returned(&self, target: CauseTarget, error: anyhow::Error, proof: ReturnedProof) {
        self.retain(target, error, CauseProof::Returned { _proof: proof });
    }
    pub(super) fn observed_stop(
        &self,
        mut target: CauseTarget,
        observation: crate::runtime::ContainerObservation,
    ) {
        // This observation follows positive collection. Its immutable ID/hash
        // suffices as provenance; the current/queued request and original raw
        // error owners have their own bounded, independent custody.
        target._retained_request = None;
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .last_observed_stop = Some(ObservedStop {
            _target: target,
            _observation: observation,
        });
    }

    /// The native worker, not its IPC waiter, owns this complete operation.
    /// A pending/unknown return keeps its exact IO capsule in this cell. No
    /// target/capacity can be reused until a positive native receipt arrives.
    pub(super) fn execute<T>(
        &self,
        runtime: &tokio::runtime::Runtime,
        target: CauseTarget,
        operation: impl std::future::Future<Output = NativeRuntimeOutcome<T>>,
    ) -> Option<Settled<T>> {
        {
            let mut causes = self.0.lock().unwrap_or_else(|p| p.into_inner());
            if causes.native_call_in_flight || causes.pending.is_some() || causes.custody_unknown {
                // No second invocation is polled over an occupied cell. Keep
                // this continuation and every prior affine owner in place.
                causes.custody_unknown = true;
                drop(causes);
                loop {
                    std::thread::park();
                }
            }
            causes.native_call_in_flight = true;
        }
        let outcome = runtime.block_on(operation);
        self.settle(runtime, target, outcome)
    }

    fn settle<T>(
        &self,
        runtime: &tokio::runtime::Runtime,
        target: CauseTarget,
        outcome: NativeRuntimeOutcome<T>,
    ) -> Option<Settled<T>> {
        match outcome {
            NativeRuntimeOutcome::NoChild { result, proof } => {
                let settled =
                    self.returned_value(target, result, ReturnedProof::NoChild { _native: proof });
                self.0
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .native_call_in_flight = false;
                settled
            }
            NativeRuntimeOutcome::Collected { result, proof } => {
                let settled = self.returned_value(
                    target,
                    result,
                    ReturnedProof::Collected { _native: proof },
                );
                self.0
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .native_call_in_flight = false;
                settled
            }
            NativeRuntimeOutcome::Uncollected(call) => {
                let call = Arc::new(Mutex::new(call));
                {
                    let mut causes = self.0.lock().unwrap_or_else(|p| p.into_inner());
                    // There is exactly one issuer per native context. A live
                    // capsule is never displaced to install another attempt.
                    if causes.pending.is_some() {
                        // Preserve the incoming owner too without admitting a
                        // new native operation: retain it on this same stack.
                        causes.custody_unknown = true;
                        drop(causes);
                        loop {
                            std::thread::park();
                        }
                    }
                    causes.pending = Some(PendingCause {
                        call: Arc::clone(&call),
                        _target: target.clone(),
                    });
                }
                loop {
                    let collected = {
                        let mut original = call.lock().unwrap_or_else(|p| p.into_inner());
                        runtime.block_on(original.collect())
                    };
                    let settled = match collected {
                        NativeCollectionOutcome::NoChild { result, proof } => {
                            Some((result, ReturnedProof::NoChild { _native: proof }))
                        }
                        NativeCollectionOutcome::Collected { result, proof } => {
                            Some((result, ReturnedProof::Collected { _native: proof }))
                        }
                        NativeCollectionOutcome::PendingOrUnknown => None,
                    };
                    if let Some((result, proof)) = settled {
                        // The collection API moves the original failure and
                        // its now-collected IO here exactly once, never Ok.
                        match result {
                            Err(error) => self.returned(target.clone(), error, proof),
                            Ok(()) => {
                                // A producer violating this contract must not
                                // cause the old capsule/capacity to disappear.
                                self.0
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .custody_unknown = true;
                                loop {
                                    std::thread::park();
                                }
                            }
                        }
                        let mut causes = self.0.lock().unwrap_or_else(|p| p.into_inner());
                        if causes
                            .pending
                            .as_ref()
                            .is_some_and(|pending| Arc::ptr_eq(&pending.call, &call))
                        {
                            causes.pending.take();
                            causes.native_call_in_flight = false;
                        } else {
                            // A changed custody identity cannot certify that
                            // this native cell is reusable.
                            causes.custody_unknown = true;
                            drop(causes);
                            loop {
                                std::thread::park();
                            }
                        }
                        // This operation's higher-level result remains
                        // unknown. The same target can retry after collection;
                        // a catch-up context can now fairly take another one.
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }
    fn returned_value<T>(
        &self,
        target: CauseTarget,
        result: anyhow::Result<T>,
        proof: ReturnedProof,
    ) -> Option<Settled<T>> {
        match result {
            Ok(value) => Some(Settled { value, proof }),
            Err(error) => {
                self.returned(target, error, proof);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{ManagedRuntime, ManagedRuntimeError, RuntimeAuthority};

    #[test]
    fn caught_native_panic_keeps_original_opaque_payload_and_marks_unknown() {
        let marker = Arc::new(());
        let original = Arc::clone(&marker);
        let payload = std::panic::catch_unwind(move || -> () { std::panic::panic_any(original) })
            .unwrap_err();
        let failures = NativeStopFailures::new();
        failures.native_thread_failed(payload);
        let facts = failures.facts();
        assert_eq!(facts.retained_causes, 1);
        assert!(facts.uncollected_io);
        assert!(!facts.omissions_exact);
        let causes = failures.0.lock().unwrap();
        let retained = causes
            .native_exit_cause
            .as_ref()
            .unwrap()
            .downcast_ref::<Arc<()>>()
            .unwrap();
        assert!(Arc::ptr_eq(retained, &marker));
        assert!(causes.first.is_none() && causes.pending.is_none());
    }

    #[test]
    fn native_no_child_errors_keep_first_object_and_a_bounded_settled_tail() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let failures = NativeStopFailures::new();
        let mut original_first = 0usize;
        for index in 0..4 {
            let binding: RuntimeAuthority = serde_json::from_value(serde_json::json!({
                "authority_id": uuid::Uuid::new_v4(), "kind":"docker",
                "endpoint":"unix:///synthetic-not-opened.sock", "engine_id":"synthetic",
                "podman":null, "invocation":null
            }))
            .unwrap();
            let outcome = runtime.block_on(ManagedRuntime::reopen_native(binding));
            // This proof comes from the real missing-invocation branch, not
            // a test-created token, status, counter or error-format matcher.
            let NativeRuntimeOutcome::NoChild {
                result: Err(ref cause),
                ..
            } = outcome
            else {
                panic!("missing invocation must not issue a control Child");
            };
            let raw = cause.downcast_ref::<ManagedRuntimeError>().unwrap();
            if index == 0 {
                original_first = raw as *const ManagedRuntimeError as usize;
            }
            assert!(
                failures
                    .execute(
                        &runtime,
                        CauseTarget::fixed(&index.to_string(), "synthetic"),
                        async move { outcome }
                    )
                    .is_none()
            );
            assert!(!failures.facts().uncollected_io);
        }
        let causes = failures.0.lock().unwrap();
        let original = causes.first.as_ref().unwrap();
        let raw = original
            ._cause
            .downcast_ref::<ManagedRuntimeError>()
            .unwrap();
        assert_eq!(raw as *const ManagedRuntimeError as usize, original_first);
        assert_eq!(original._target._arm_id.as_deref(), Some("0"));
        assert_eq!(
            causes
                .last_settled
                .as_ref()
                .unwrap()
                ._target
                ._arm_id
                .as_deref(),
            Some("3")
        );
        assert_eq!(causes.omitted_settled, 2);
        assert!(causes.omissions_exact);
        assert!(causes.pending.is_none());
        assert!(matches!(
            &original._proof,
            CauseProof::Returned {
                _proof: ReturnedProof::NoChild { .. }
            }
        ));
    }
}

impl std::fmt::Debug for NativeStopFailures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let causes = self.0.lock().unwrap_or_else(|p| p.into_inner());
        f.debug_struct("NativeStopFailures")
            .field("first_retained", &causes.first.is_some())
            .field("last_settled_retained", &causes.last_settled.is_some())
            .field("pending_owner_retained", &causes.pending.is_some())
            .field(
                "last_observed_stop_retained",
                &causes.last_observed_stop.is_some(),
            )
            .field("omitted_settled", &causes.omitted_settled)
            .field("omissions_exact", &causes.omissions_exact)
            .finish_non_exhaustive()
    }
}
