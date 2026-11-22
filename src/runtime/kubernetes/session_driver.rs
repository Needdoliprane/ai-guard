//! Broker-side driver of one effectful Kubernetes session attempt (G11, K4).
//!
//! The driver composes the pieces of the effect path in one order and never
//! sends a request without a central claim: for every effect it builds the
//! exact request, claims it on the tenant ledger, turns the single `EmitOnce`
//! acknowledgement into the production permit, sends the request once and
//! records what it observed. A lost or `ObserveOnly` answer is never
//! re-sent: the driver stops and the caller observes (decision D6: no re-
//! attach, resume means reaching the terminal by UID).
//!
//! The ledger is a seam with two adapters: the broker's authenticated HTTP
//! client in production and the tenant Store in tests. Kubernetes answers
//! are what this module observes; worker messages stay declared.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "driver without its CLI entry until the session run command lands (K4)"
    )
)]
use super::{
    KubernetesRuntime,
    bootstrap::BootstrapCapacity,
    session::SessionBootstrapBindingV2,
    session_attach::{AttachAttemptV2, AttachIdentityV2},
    session_claim::{
        KubernetesSessionClaimRequestV2, KubernetesSessionStorageRequestV2,
        SessionClaimCreateOutcomeV2,
    },
    session_effect::{
        SessionCreateOutcomeV2, SessionDeleteOutcomeV2, SessionFinalizerOutcomeV2,
        SessionReleaseOutcomeV2,
    },
    session_instance::{
        KubernetesSessionRuntimeError, KubernetesSessionWorkspaceClaimV2, SessionContainerStateV2,
        SessionInstanceV2, SessionObservationStateV2, SessionObservedV2,
    },
    session_permit::{CentralEffectPermitV2, RetainedEffectClaimV2},
    session_pod::{KubernetesSessionPodRequestV2, session_workspace_claim_name},
    session_protocol::{SessionChannelV3, SessionModeV3},
    session_requests::{
        attach_request, delete_claim_request, delete_pod_request, release_finalizer_request,
        release_gate_request,
    },
};
use crate::control::kubernetes_session::{
    KubernetesSessionReservationCurrentV2, KubernetesSessionReservationIntentV2,
    KubernetesSessionReservationStateV2,
    effects::{
        KubernetesSessionDeadlineArmV2, KubernetesSessionEffectActionV2 as Action,
        KubernetesSessionEffectClaimAckV2, KubernetesSessionEffectDispositionV2,
        KubernetesSessionEffectOutcomeV2 as Outcome, KubernetesSessionEffectRequestV2,
        KubernetesSessionEffectResultV2, KubernetesSessionEffectStateV2,
        KubernetesSessionEffectSubjectV2 as Subject, KubernetesSessionPodEndV2,
        KubernetesSessionSettlementV2, KubernetesSessionTerminalV2,
    },
};
use chrono::{DateTime, SecondsFormat, Utc};
use std::{fmt, sync::atomic::AtomicBool, time::Duration};
use uuid::Uuid;

/// Why the driver stopped. None of these releases quota: only a settlement
/// does, after observed ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionDriverErrorV2 {
    /// The ledger refused the record; for a claim, nothing was sent.
    Ledger(String),
    /// No validated answer: the record may be durable. For a claim nothing
    /// was sent and the step stays open until an authorized recovery; a
    /// result is re-sent identically (same operation id) before this is
    /// returned.
    LedgerReceptionUnknown,
    /// The step was already claimed (a replay): observe, never send.
    ObserveOnly { step: u32 },
    /// The request was sent and its effect is unknown; ownership stays
    /// unknown and the quota stays held until an authorized recovery.
    Uncertain { step: u32 },
    /// The request was answered without effect (failed test or precondition).
    NotApplied { step: u32, status: u16 },
    /// A local refusal before any request or a closed observation error.
    Runtime(KubernetesSessionRuntimeError),
    /// The claim did not become `Bound` within the configured wait.
    ClaimNotBound,
    /// The observed claim is not the object of the 201 that created it.
    ClaimReplaced,
    /// The bound claim differs from the authorized storage request.
    ClaimStorageClassChanged,
    /// This reservation does not confirm a resumable first claim.
    WorkspaceResumeUnavailable,
}
impl fmt::Display for SessionDriverErrorV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ledger(reason) => {
                write!(f, "Kubernetes session ledger refused the step: {reason}")
            }
            Self::LedgerReceptionUnknown => f.write_str(
                "the Kubernetes session ledger answer was lost; the record may be durable",
            ),
            Self::ObserveOnly { step } => write!(
                f,
                "Kubernetes session step {step} was already claimed; observe it, never resend"
            ),
            Self::Uncertain { step } => write!(
                f,
                "Kubernetes session step {step} has an unknown effect; quota stays held until recovery"
            ),
            Self::NotApplied { step, status } => write!(
                f,
                "Kubernetes session step {step} was not applied (HTTP {status})"
            ),
            Self::Runtime(error) => write!(f, "{error}"),
            Self::ClaimNotBound => {
                f.write_str("the workspace claim was not bound within the configured wait")
            }
            Self::ClaimReplaced => {
                f.write_str("the workspace claim is not the object created by this attempt")
            }
            Self::ClaimStorageClassChanged => {
                f.write_str("the bound workspace claim does not match the approved storage class")
            }
            Self::WorkspaceResumeUnavailable => {
                f.write_str("the current reservation does not confirm this workspace claim")
            }
        }
    }
}
impl std::error::Error for SessionDriverErrorV2 {}
impl From<KubernetesSessionRuntimeError> for SessionDriverErrorV2 {
    fn from(error: KubernetesSessionRuntimeError) -> Self {
        Self::Runtime(error)
    }
}
type Driven<T> = Result<T, SessionDriverErrorV2>;

/// The tenant effect ledger as the driver sees it.
pub(crate) trait SessionLedgerV2 {
    async fn claim(
        &self,
        request: &KubernetesSessionEffectRequestV2,
    ) -> Driven<KubernetesSessionEffectClaimAckV2>;
    async fn result(
        &self,
        result: &KubernetesSessionEffectResultV2,
    ) -> Driven<KubernetesSessionEffectStateV2>;
    async fn terminal(
        &self,
        terminal: &KubernetesSessionTerminalV2,
    ) -> Driven<KubernetesSessionEffectStateV2>;
    async fn settle(
        &self,
        settlement: &KubernetesSessionSettlementV2,
    ) -> Driven<KubernetesSessionEffectStateV2>;
    async fn state(&self, reservation_id: &str) -> Driven<KubernetesSessionEffectStateV2>;
    async fn reservation(
        &self,
        intent: &KubernetesSessionReservationIntentV2,
    ) -> Driven<KubernetesSessionReservationCurrentV2>;
}

fn ledger_error(
    error: crate::broker::held_create_client::sessions::SessionControlErrorV2,
) -> SessionDriverErrorV2 {
    use crate::broker::held_create_client::sessions::SessionControlErrorV2 as E;
    match error {
        E::ReceptionUnknown => SessionDriverErrorV2::LedgerReceptionUnknown,
        other => SessionDriverErrorV2::Ledger(other.to_string()),
    }
}

/// Production adapter: the authenticated broker HTTP client.
impl SessionLedgerV2 for crate::broker::held_create_client::sessions::SessionControlClientV2 {
    async fn claim(
        &self,
        request: &KubernetesSessionEffectRequestV2,
    ) -> Driven<KubernetesSessionEffectClaimAckV2> {
        self.claim_effect(request).await.map_err(ledger_error)
    }
    async fn result(
        &self,
        result: &KubernetesSessionEffectResultV2,
    ) -> Driven<KubernetesSessionEffectStateV2> {
        self.record_effect_result(result)
            .await
            .map_err(ledger_error)
    }
    async fn terminal(
        &self,
        terminal: &KubernetesSessionTerminalV2,
    ) -> Driven<KubernetesSessionEffectStateV2> {
        self.record_terminal(terminal).await.map_err(ledger_error)
    }
    async fn settle(
        &self,
        settlement: &KubernetesSessionSettlementV2,
    ) -> Driven<KubernetesSessionEffectStateV2> {
        crate::broker::held_create_client::sessions::SessionControlClientV2::settle(
            self, settlement,
        )
        .await
        .map_err(ledger_error)
    }
    async fn state(&self, reservation_id: &str) -> Driven<KubernetesSessionEffectStateV2> {
        self.effect_state(reservation_id)
            .await
            .map_err(ledger_error)
    }
    async fn reservation(
        &self,
        intent: &KubernetesSessionReservationIntentV2,
    ) -> Driven<KubernetesSessionReservationCurrentV2> {
        self.observe(intent).await.map_err(ledger_error)
    }
}

/// Polling bounds of the driver. Harness waits, not product deadlines: the
/// session deadline itself is the enforcer's armed deadline (R3).
#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionDriverWaitsV2 {
    pub(crate) bind: Duration,
    pub(crate) poll: Duration,
}
impl Default for SessionDriverWaitsV2 {
    fn default() -> Self {
        Self {
            bind: Duration::from_secs(120),
            poll: Duration::from_millis(500),
        }
    }
}

/// Identical re-sends of a result whose answer was lost.
const REPORT_RESENDS: u32 = 3;

pub(crate) struct SessionDriverV2<'a, L> {
    runtime: &'a KubernetesRuntime,
    ledger: &'a L,
    intent: KubernetesSessionReservationIntentV2,
    reservation_input_hash: String,
    last_step: u32,
    waits: SessionDriverWaitsV2,
    cancelled: AtomicBool,
}

/// A claimed, not yet reported step and its single permit.
struct Claimed {
    ack: KubernetesSessionEffectClaimAckV2,
    permit: CentralEffectPermitV2,
}

impl<'a, L: SessionLedgerV2> SessionDriverV2<'a, L> {
    /// Stops every later send of this driver. Cleanup of already issued
    /// effects goes through a new driver opened on the same reservation.
    pub(crate) fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// The position is read from the ledger's current effect state of this
    /// reservation; the driver never guesses it.
    pub(crate) async fn open(
        runtime: &'a KubernetesRuntime,
        ledger: &'a L,
        intent: KubernetesSessionReservationIntentV2,
        tenant_id: &str,
        waits: SessionDriverWaitsV2,
    ) -> Driven<Self> {
        let refused = || {
            SessionDriverErrorV2::Runtime(KubernetesSessionRuntimeError::EffectRefusedBeforeSend)
        };
        intent.validate().map_err(|_| refused())?;
        if intent.authority() != runtime.binding() {
            return Err(refused());
        }
        let state = ledger.state(intent.reservation_id()).await?;
        if state.reservation_id != intent.reservation_id() {
            return Err(refused());
        }
        let reservation_input_hash = intent.input_hash(tenant_id).map_err(|_| refused())?;
        Ok(Self {
            runtime,
            ledger,
            intent,
            reservation_input_hash,
            last_step: state.last_step,
            waits,
            cancelled: AtomicBool::new(false),
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one closed request per effect kind; fields are validated by the DTO"
    )]
    async fn claim(
        &mut self,
        subject: Subject,
        action: Action,
        request_sha256: &str,
        object_uid: Option<&str>,
        object_resource_version: Option<&str>,
        storage: Option<&KubernetesSessionStorageRequestV2>,
        deadline: Option<&KubernetesSessionDeadlineArmV2>,
    ) -> Driven<Claimed> {
        // A cancelled driver claims nothing more; a cancellation raised after
        // a claim refuses its permit and the step is recorded `NotSent`.
        if self.cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(SessionDriverErrorV2::Runtime(
                KubernetesSessionRuntimeError::EffectRefusedBeforeSend,
            ));
        }
        let request = KubernetesSessionEffectRequestV2 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            reservation_id: self.intent.reservation_id().into(),
            reservation_input_hash: self.reservation_input_hash.clone(),
            step: self.last_step + 1,
            subject,
            action,
            request_sha256: request_sha256.into(),
            object_uid: object_uid.map(str::to_owned),
            object_resource_version: object_resource_version.map(str::to_owned),
            storage: storage.cloned(),
            deadline: deadline.cloned(),
        };
        let ack = self.ledger.claim(&request).await?;
        if ack.request != request {
            return Err(SessionDriverErrorV2::Ledger(
                "acknowledgement for another request".into(),
            ));
        }
        self.last_step = request.step;
        if ack.disposition != KubernetesSessionEffectDispositionV2::EmitOnce || ack.replayed {
            return Err(SessionDriverErrorV2::ObserveOnly { step: request.step });
        }
        let permit = CentralEffectPermitV2::from_retained(RetainedEffectClaimV2::retained(
            ack.clone(),
            self.intent.authority(),
        )?)?;
        Ok(Claimed { ack, permit })
    }

    /// Records the observed outcome. A lost answer is re-sent identically:
    /// the ledger replays an exact operation and refuses a different one.
    async fn report(
        &self,
        ack: &KubernetesSessionEffectClaimAckV2,
        outcome: Outcome,
    ) -> Driven<KubernetesSessionEffectStateV2> {
        let result = KubernetesSessionEffectResultV2 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            reservation_id: ack.request.reservation_id.clone(),
            step: ack.request.step,
            claim_operation_id: ack.request.operation_id.clone(),
            request_sha256: ack.request.request_sha256.clone(),
            outcome,
            observed_at: now(),
        };
        let mut attempts = 0;
        loop {
            match self.ledger.result(&result).await {
                Err(SessionDriverErrorV2::LedgerReceptionUnknown) if attempts < REPORT_RESENDS => {
                    attempts += 1;
                    tokio::time::sleep(self.waits.poll).await;
                }
                answer => return answer,
            }
        }
    }

    /// R5: claim, create and wait for the exact bound workspace claim.
    pub(crate) async fn provision_workspace(
        &mut self,
        storage: &KubernetesSessionStorageRequestV2,
    ) -> Driven<KubernetesSessionWorkspaceClaimV2> {
        let request = KubernetesSessionClaimRequestV2::render(&self.intent, storage)
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        let Claimed { ack, permit } = self
            .claim(
                Subject::WorkspaceClaim,
                Action::Create,
                request.target().request_sha256(),
                None,
                None,
                Some(storage),
                None,
            )
            .await?;
        let sent = self
            .runtime
            .create_session_workspace_claim(&request, permit, &self.cancelled)
            .await;
        let created = match self.sent(&ack, sent).await? {
            SessionClaimCreateOutcomeV2::Created(created) => created,
            SessionClaimCreateOutcomeV2::OwnershipUnknown { http_status, .. } => {
                self.report(&ack, Outcome::Uncertain { http_status })
                    .await?;
                return Err(SessionDriverErrorV2::Uncertain {
                    step: ack.request.step,
                });
            }
        };
        self.report(
            &ack,
            Outcome::Applied {
                object_uid: created.uid().into(),
                resource_version: Some(created.resource_version().into()),
            },
        )
        .await?;
        let bound = self
            .wait_for_bound_workspace(created.claim_name(), created.uid())
            .await?;
        if bound.storage_class_name() != storage.storage_class_name.as_str() {
            return Err(SessionDriverErrorV2::ClaimStorageClassChanged);
        }
        Ok(bound)
    }

    /// Observe a confirmed first PVC CREATE after reopening the driver.
    /// The central position binds the original UID, but contains no proof of
    /// the requested storage bytes. A successful observation grants no permit.
    pub(crate) async fn observe_provisioned_workspace(&self) -> Driven<()> {
        let unavailable = SessionDriverErrorV2::WorkspaceResumeUnavailable;
        let current = self.ledger.reservation(&self.intent).await?;
        if current.snapshot.input_hash != self.reservation_input_hash
            || current.snapshot.state.state != KubernetesSessionReservationStateV2::Reserved
        {
            return Err(unavailable);
        }
        let reservation_id = self.intent.reservation_id();
        let state = self.ledger.state(reservation_id).await?;
        if state.version != 1
            || state.reservation_id != reservation_id
            || state.last_step != 1
            || state.unresolved_uncertain_steps != 0
            || state.session_pod_uid.is_some()
            || state.export_pod_uid.is_some()
            || state.session_terminal
            || state.export_terminal
            || state.settled
        {
            return Err(unavailable);
        }
        let uid = state
            .workspace_claim_uid
            .as_deref()
            .ok_or(SessionDriverErrorV2::WorkspaceResumeUnavailable)?;
        let ids = self
            .intent
            .ids()
            .map_err(|_| SessionDriverErrorV2::WorkspaceResumeUnavailable)?;
        self.wait_for_bound_workspace(&session_workspace_claim_name(ids.attempt_id), uid)
            .await?;
        if self.ledger.state(reservation_id).await? != state {
            return Err(unavailable);
        }
        Ok(())
    }

    async fn wait_for_bound_workspace(
        &self,
        name: &str,
        uid: &str,
    ) -> Driven<KubernetesSessionWorkspaceClaimV2> {
        let deadline = tokio::time::Instant::now() + self.waits.bind;
        loop {
            match self
                .runtime
                .inspect_session_workspace_claim_exact(name, uid)
                .await
            {
                Ok(claim) => return Ok(claim),
                Err(KubernetesSessionRuntimeError::WorkspaceClaimReplaced) => {
                    return Err(SessionDriverErrorV2::ClaimReplaced);
                }
                Err(KubernetesSessionRuntimeError::WorkspaceClaimPending) => {}
                Err(error) => return Err(error.into()),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(SessionDriverErrorV2::ClaimNotBound);
            }
            tokio::time::sleep(self.waits.poll).await;
        }
    }

    /// Held CREATE of one rendered r2 Pod (session or export).
    pub(crate) async fn create_pod(
        &mut self,
        subject: Subject,
        request: &KubernetesSessionPodRequestV2,
    ) -> Driven<SessionInstanceV2> {
        let Claimed { ack, permit } = self
            .claim(
                subject,
                Action::Create,
                request.target().request_sha256(),
                None,
                None,
                None,
                None,
            )
            .await?;
        let sent = self
            .runtime
            .create_session_held(request, permit, &self.cancelled)
            .await;
        match self.sent(&ack, sent).await? {
            SessionCreateOutcomeV2::CreatedHeld(instance) => {
                self.report(
                    &ack,
                    Outcome::Applied {
                        object_uid: instance.pod_uid().into(),
                        resource_version: Some(instance.created_resource_version().into()),
                    },
                )
                .await?;
                Ok(*instance)
            }
            SessionCreateOutcomeV2::OwnershipUnknown { http_status, .. } => {
                self.report(&ack, Outcome::Uncertain { http_status })
                    .await?;
                Err(SessionDriverErrorV2::Uncertain {
                    step: ack.request.step,
                })
            }
        }
    }

    /// Gate release of a Held observation with the armed deadline (R3).
    pub(crate) async fn release_pod(
        &mut self,
        subject: Subject,
        instance: &SessionInstanceV2,
        deadline: &KubernetesSessionDeadlineArmV2,
    ) -> Driven<()> {
        let observed = self.runtime.observe_session_instance(instance).await?;
        let body = release_gate_request(
            self.runtime.binding(),
            instance.target().pod_name(),
            instance.pod_uid(),
            observed.resource_version(),
        )?;
        let Claimed { ack, permit } = self
            .claim(
                subject,
                Action::ReleaseGate,
                &body.request_sha256,
                Some(instance.pod_uid()),
                Some(observed.resource_version()),
                None,
                Some(deadline),
            )
            .await?;
        let sent = self
            .runtime
            .release_session_gate(instance, &observed, permit, &self.cancelled);
        let outcome = self.sent(&ack, sent.await).await?;
        self.finish(&ack, instance.pod_uid(), release_outcome(outcome))
            .await
    }

    /// The single attach of a running session Pod (`stdinOnce`; no second
    /// attach, decision D6). The claimed digest binds identity one, read
    /// here from the same observation fields the bracket reads again before
    /// sending: a changed container refuses the permit, nothing is sent.
    #[expect(
        clippy::too_many_arguments,
        reason = "the streaming attach inputs; each is owned by a different layer"
    )]
    pub(crate) async fn attach_session(
        &mut self,
        instance: &SessionInstanceV2,
        binding: SessionBootstrapBindingV2,
        attempt: &AttachAttemptV2,
        bootstrap: BootstrapCapacity,
        until: tokio::time::Instant,
        session_until: tokio::time::Instant,
        mode: SessionModeV3,
    ) -> Driven<SessionChannelV3> {
        let observed = self.runtime.observe_session_instance(instance).await?;
        let identity = AttachIdentityV2::from_observation(&observed).ok_or(
            SessionDriverErrorV2::Runtime(KubernetesSessionRuntimeError::EffectRefusedBeforeSend),
        )?;
        let body = attach_request(
            self.runtime.binding(),
            instance.target().pod_name(),
            &identity.canonical_bytes()?,
        )?;
        let Claimed { ack, permit } = self
            .claim(
                Subject::SessionPod,
                Action::Attach,
                &body.request_sha256,
                Some(instance.pod_uid()),
                None,
                None,
                None,
            )
            .await?;
        let attached = self
            .runtime
            .attach_exact_instance_streaming(
                instance,
                binding,
                permit,
                attempt,
                bootstrap,
                until,
                session_until,
                &self.cancelled,
                mode,
            )
            .await;
        match attached {
            Ok(channel) => {
                self.report(
                    &ack,
                    Outcome::Applied {
                        object_uid: instance.pod_uid().into(),
                        resource_version: None,
                    },
                )
                .await?;
                Ok(channel)
            }
            Err(error) => {
                // The record tells whether the permit let the request out.
                let _ = attempt.join().await;
                let initiated = attempt
                    .report()
                    .is_none_or(|report| report.request_initiated());
                if initiated {
                    self.report(&ack, Outcome::Uncertain { http_status: None })
                        .await?;
                    Err(SessionDriverErrorV2::Uncertain {
                        step: ack.request.step,
                    })
                } else {
                    self.report(&ack, Outcome::NotSent).await?;
                    Err(error.into())
                }
            }
        }
    }

    /// DELETE of one Pod by its UID (stop, or cleanup after the terminal).
    pub(crate) async fn delete_pod(
        &mut self,
        subject: Subject,
        instance: &SessionInstanceV2,
    ) -> Driven<()> {
        let observed = self.runtime.observe_session_instance(instance).await?;
        let held = matches!(observed.state(), SessionObservationStateV2::Held);
        let held_version = held.then(|| observed.resource_version());
        let body = delete_pod_request(
            self.runtime.binding(),
            instance.target().pod_name(),
            instance.pod_uid(),
            held_version,
        )?;
        let Claimed { ack, permit } = self
            .claim(
                subject,
                Action::Delete,
                &body.request_sha256,
                Some(instance.pod_uid()),
                held_version,
                None,
                None,
            )
            .await?;
        let sent = self
            .runtime
            .delete_session_exact(instance, &observed, permit, &self.cancelled);
        let outcome = self.sent(&ack, sent.await).await?;
        self.finish(&ack, instance.pod_uid(), delete_outcome(outcome))
            .await
    }

    /// The runtime-observed end of one Pod, kept readable by the terminal
    /// finalizer, recorded centrally.
    pub(crate) async fn record_terminal(
        &mut self,
        subject: Subject,
        instance: &SessionInstanceV2,
        declared_exit_code: Option<i32>,
    ) -> Driven<(SessionObservedV2, KubernetesSessionEffectStateV2)> {
        let observed = self.runtime.observe_session_instance(instance).await?;
        let end = pod_end(&observed).ok_or(SessionDriverErrorV2::Runtime(
            KubernetesSessionRuntimeError::EffectRefusedBeforeSend,
        ))?;
        let state = self
            .ledger
            .terminal(&KubernetesSessionTerminalV2 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                reservation_id: self.intent.reservation_id().into(),
                subject,
                pod_uid: instance.pod_uid().into(),
                end,
                observed_at: now(),
                declared_exit_code,
            })
            .await?;
        Ok((observed, state))
    }

    /// D5: removal of the terminal finalizer after the recorded terminal.
    pub(crate) async fn release_finalizer(
        &mut self,
        subject: Subject,
        instance: &SessionInstanceV2,
        observed: &SessionObservedV2,
    ) -> Driven<()> {
        let body = release_finalizer_request(
            self.runtime.binding(),
            instance.target().pod_name(),
            instance.pod_uid(),
        )?;
        let Claimed { ack, permit } = self
            .claim(
                subject,
                Action::ReleaseFinalizer,
                &body.request_sha256,
                Some(instance.pod_uid()),
                None,
                None,
                None,
            )
            .await?;
        let sent =
            self.runtime
                .release_session_finalizer(instance, observed, permit, &self.cancelled);
        let outcome = self.sent(&ack, sent.await).await?;
        self.finish(&ack, instance.pod_uid(), finalizer_outcome(outcome))
            .await
    }

    /// DELETE of the workspace claim by its UID once nothing writes to it.
    pub(crate) async fn delete_workspace(
        &mut self,
        claim: &KubernetesSessionWorkspaceClaimV2,
    ) -> Driven<()> {
        let body = delete_claim_request(self.runtime.binding(), claim.name(), claim.uid())?;
        let Claimed { ack, permit } = self
            .claim(
                Subject::WorkspaceClaim,
                Action::Delete,
                &body.request_sha256,
                Some(claim.uid()),
                None,
                None,
                None,
            )
            .await?;
        let sent =
            self.runtime
                .delete_session_workspace_claim_exact(claim, permit, &self.cancelled);
        let outcome = self.sent(&ack, sent.await).await?;
        self.finish(&ack, claim.uid(), delete_outcome(outcome))
            .await
    }

    /// Releases the attempt's quota after every observed end.
    pub(crate) async fn settle(&mut self) -> Driven<KubernetesSessionEffectStateV2> {
        self.ledger
            .settle(&KubernetesSessionSettlementV2 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                reservation_id: self.intent.reservation_id().into(),
                reservation_input_hash: self.reservation_input_hash.clone(),
                expected_last_step: self.last_step,
            })
            .await
    }

    /// After a claim, a runtime refusal means nothing was sent: it is
    /// recorded `NotSent` so that the step does not stay open.
    async fn sent<T>(
        &self,
        ack: &KubernetesSessionEffectClaimAckV2,
        sent: Result<T, KubernetesSessionRuntimeError>,
    ) -> Driven<T> {
        match sent {
            Ok(value) => Ok(value),
            Err(error) => {
                self.report(ack, Outcome::NotSent).await?;
                Err(error.into())
            }
        }
    }

    /// Records the observed outcome and maps it to the driver's result.
    async fn finish(
        &self,
        ack: &KubernetesSessionEffectClaimAckV2,
        uid: &str,
        outcome: StepOutcome,
    ) -> Driven<()> {
        let step = ack.request.step;
        match outcome {
            StepOutcome::Applied { resource_version } => {
                self.report(
                    ack,
                    Outcome::Applied {
                        object_uid: uid.into(),
                        resource_version,
                    },
                )
                .await?;
                Ok(())
            }
            StepOutcome::NotApplied { status } => {
                self.report(
                    ack,
                    Outcome::NotApplied {
                        http_status: status,
                    },
                )
                .await?;
                Err(SessionDriverErrorV2::NotApplied { step, status })
            }
            StepOutcome::Uncertain { http_status } => {
                self.report(ack, Outcome::Uncertain { http_status }).await?;
                Err(SessionDriverErrorV2::Uncertain { step })
            }
        }
    }
}

enum StepOutcome {
    Applied { resource_version: Option<String> },
    NotApplied { status: u16 },
    Uncertain { http_status: Option<u16> },
}
/// A validated release is applied; its resourceVersion is the new one.
fn release_outcome(outcome: SessionReleaseOutcomeV2) -> StepOutcome {
    match outcome {
        SessionReleaseOutcomeV2::Released { resource_version } => StepOutcome::Applied {
            resource_version: Some(resource_version),
        },
        SessionReleaseOutcomeV2::RefusedNotApplied { status } => StepOutcome::NotApplied { status },
        SessionReleaseOutcomeV2::Uncertain { http_status, .. } => {
            StepOutcome::Uncertain { http_status }
        }
    }
}
/// An accepted DELETE is applied; its terminal still needs an observation.
/// The runtime does not read the resourceVersion of that answer.
fn delete_outcome(outcome: SessionDeleteOutcomeV2) -> StepOutcome {
    match outcome {
        SessionDeleteOutcomeV2::DeletionRequested => StepOutcome::Applied {
            resource_version: None,
        },
        SessionDeleteOutcomeV2::NotFound => StepOutcome::NotApplied { status: 404 },
        SessionDeleteOutcomeV2::PreconditionFailed => StepOutcome::NotApplied { status: 409 },
        SessionDeleteOutcomeV2::Uncertain { http_status, .. } => {
            StepOutcome::Uncertain { http_status }
        }
    }
}
fn finalizer_outcome(outcome: SessionFinalizerOutcomeV2) -> StepOutcome {
    match outcome {
        SessionFinalizerOutcomeV2::Released => StepOutcome::Applied {
            resource_version: None,
        },
        SessionFinalizerOutcomeV2::RefusedNotApplied { status } => {
            StepOutcome::NotApplied { status }
        }
        SessionFinalizerOutcomeV2::NotFound => StepOutcome::NotApplied { status: 404 },
        SessionFinalizerOutcomeV2::Uncertain { http_status, .. } => {
            StepOutcome::Uncertain { http_status }
        }
    }
}

/// The observed end of one exact instance, if it has one: a terminated
/// container, or a deletion while the gate was still present.
fn pod_end(observed: &SessionObservedV2) -> Option<KubernetesSessionPodEndV2> {
    let container = match observed.state() {
        SessionObservationStateV2::Scheduled { container, .. }
        | SessionObservationStateV2::Deleting { container } => container,
        SessionObservationStateV2::Held | SessionObservationStateV2::ReleasedUnscheduled => {
            return None;
        }
    };
    match container {
        SessionContainerStateV2::Terminated {
            container_id,
            exit_code,
            oom_killed,
            started_at,
            finished_at,
        } => Some(KubernetesSessionPodEndV2::Terminated {
            container_id: container_id.clone(),
            exit_code: *exit_code,
            oom_killed: *oom_killed,
            started_at: started_at.map(millis),
            finished_at: finished_at.map(millis),
        }),
        SessionContainerStateV2::Absent
            if observed.gated()
                && matches!(observed.state(), SessionObservationStateV2::Deleting { .. }) =>
        {
            Some(KubernetesSessionPodEndV2::NeverStarted)
        }
        _ => None,
    }
}
fn millis(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn now() -> String {
    millis(Utc::now())
}

#[cfg(test)]
#[path = "session_driver_tests.rs"]
mod tests;
