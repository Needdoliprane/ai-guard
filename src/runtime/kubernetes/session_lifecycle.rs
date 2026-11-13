//! V2 before-effect lifecycle mechanics (DESIGN layer C).
//!
//! Retains the exact request of one attempt, abandons a reservation that
//! never issued an effect, recovers it through the existing successor API and
//! reports an observe-only reconciliation. No production caller exists yet:
//! the gateway Store mints confirmations while the broker keeps the journal
//! and the Kubernetes credentials, and the broker routes that would join them
//! are the Root request R2. Operators use the existing Runner
//! `control-recovery` route meanwhile.
//!
//! Nothing here creates, releases, attaches or deletes a Pod. The only
//! central mutation is the existing before-effect close, with a stable
//! operation id retained locally first; quota is released only by its
//! committed compare-and-set, and an exact replay adds no event. The
//! incarnation passed by the caller selects which existing Store
//! authorization is tried; it is never a proof, and the Store re-checks the
//! authenticated broker, tenant, namespace and history.
//!
//! The successor recovery policy is trusted input, not checked against any
//! configuration: the Store verifies only that the policy it is given
//! approves the authenticated broker, and records the policy hash. A future
//! production caller (R2) must take it from the gateway configuration of the
//! tenant (`session_recovery_policy`), never from a broker request, a journal
//! file or a worker; otherwise it becomes a self-issued successor capability.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "V2 lifecycle mechanics without a production caller until broker routes exist (DESIGN section 0, layer C)"
    )
)]
use super::{
    KubernetesError, KubernetesRuntime,
    session::{
        RetainedSessionViewV2, SessionError, SessionJournal, SessionJournalProtection,
        SessionPhaseV2,
    },
    session_instance::{
        KubernetesSessionNameInspectionV2, KubernetesSessionObjectPresenceV2,
        KubernetesSessionWorkspaceClaimV2,
    },
    session_pod::KubernetesSessionPodRequestV2,
};
use crate::{
    audit::{Store, kubernetes_session::KubernetesSessionRecoveryAuthorizationV2},
    broker_registry::BrokerIdentityV1,
    control::kubernetes_session::{
        KubernetesSessionRecoveryPolicyV2, KubernetesSessionReservationCloseReasonV2,
        KubernetesSessionReservationCloseV2, KubernetesSessionReservationFailureV2,
        KubernetesSessionReservationMutationResponseV2, KubernetesSessionReservationStateV2,
    },
    control_fs::Directory,
};
use serde::Serialize;
use std::fmt;
use uuid::Uuid;

/// Closed lifecycle failures. They carry no server body, path or credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionLifecycleError {
    Journal(SessionError),
    /// A typed reservation failure returned by the Store.
    Control(KubernetesSessionReservationFailureV2),
    /// A Store failure without a typed reservation result (broker identity
    /// revalidation, audit capacity or storage). Nothing is assumed committed.
    ControlUnclassified,
    Kubernetes(KubernetesError),
    /// The journal was prepared through another Kubernetes authority than the
    /// runtime's; nothing was requested.
    InvalidContext,
}
impl fmt::Display for SessionLifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Journal(error) => fmt::Display::fmt(error, f),
            Self::Control(error) => fmt::Display::fmt(error, f),
            Self::ControlUnclassified => f.write_str(
                "Kubernetes session control refused or failed without a typed reservation result; nothing is assumed committed",
            ),
            Self::Kubernetes(error) => fmt::Display::fmt(error, f),
            Self::InvalidContext => f.write_str(
                "Kubernetes session journal belongs to another Kubernetes authority; nothing was requested",
            ),
        }
    }
}
impl std::error::Error for SessionLifecycleError {}
type Lifecycle<T> = Result<T, SessionLifecycleError>;

fn journal_error(error: SessionError) -> SessionLifecycleError {
    SessionLifecycleError::Journal(error)
}
fn failure(error: &anyhow::Error) -> Option<KubernetesSessionReservationFailureV2> {
    error
        .downcast_ref::<KubernetesSessionReservationFailureV2>()
        .copied()
}
fn control(error: anyhow::Error) -> SessionLifecycleError {
    failure(&error).map_or(
        SessionLifecycleError::ControlUnclassified,
        SessionLifecycleError::Control,
    )
}

/// Typed result of one abandonment attempt. Only `Closed` committed a central
/// close in this call; `Replayed` returned the historical receipt without a
/// new event. The other outcomes made no Store mutation.
#[derive(Debug)]
pub(crate) enum SessionAbandonOutcomeV2 {
    Closed(KubernetesSessionReservationMutationResponseV2),
    Replayed(KubernetesSessionReservationMutationResponseV2),
    /// Another incarnation's journal without a tenant recovery policy: no
    /// Store call was made and nothing was retained.
    SuccessorRequiresRecoveryPolicy,
    /// The view retains no close: no Store call was made. A successor cannot
    /// retain one today (no successor confirmation, Root R11); the operator
    /// route is the Runner `control-recovery` close.
    NoRetainedAbandonment,
    /// Same incarnation: the Store reports the reservation closed by another
    /// operation, classified by observation. That operation released the
    /// quota; no new event was appended.
    ClosedCentrallyByAnotherOperation {
        last_close_operation_id: String,
    },
    /// Successor: the Store has no record of the reservation or of the
    /// retained operation. A successor cannot observe a closed reservation,
    /// so this stays ambiguous (absent reservation, or closed by another
    /// operation) and is reported as such.
    SuccessorLookupNotFound,
}

/// Retains the claim observed for this attempt and the exact request
/// rendered from the retained claim. An identical retained claim is reused;
/// a newer observation of the same PVC conflicts, and the first retained
/// claim stays authoritative. Synchronous file work for the caller's blocking
/// segment; no network, no Store call and no effect.
pub(crate) fn retain_session_request(
    journal: &mut SessionJournal,
    claim: &KubernetesSessionWorkspaceClaimV2,
) -> Lifecycle<KubernetesSessionPodRequestV2> {
    journal
        .retain_workspace_claim(claim)
        .map_err(journal_error)?;
    let retained = journal
        .workspace_claim()
        .map_err(journal_error)?
        .ok_or(SessionLifecycleError::Journal(SessionError::Corrupt))?;
    let intent = journal.intent().map_err(journal_error)?;
    let binding = journal.bootstrap_binding().map_err(journal_error)?;
    let request = KubernetesSessionPodRequestV2::render(&intent, &binding, &retained)
        .map_err(SessionLifecycleError::Kubernetes)?;
    journal
        .retain_pod_request(&request)
        .map_err(journal_error)?;
    Ok(request)
}

/// Abandons the reservation of a live journal before any effect.
///
/// Reuses the retained close, or retains a fresh one first (stable operation
/// id, reason `preparation_abandoned`, expected revision 1), then closes it
/// through the existing Store API. The journal is revalidated just before the
/// Store call, so an effect checkpoint name or a replaced record refuses
/// (`Corrupt`) without any Store call. Retrying after a lost response returns
/// `Replayed`. A journal of another incarnation needs the tenant recovery
/// policy; without it nothing is retained and the Store is not called.
/// `recovery_policy` is trusted input (module doc): the tenant's configured
/// policy, never one supplied by a broker request, a journal or a worker.
pub(crate) fn abandon_before_effect(
    store: &Store,
    identity: &BrokerIdentityV1,
    own_incarnation_id: Uuid,
    journal: &mut SessionJournal,
    recovery_policy: Option<&KubernetesSessionRecoveryPolicyV2>,
) -> Lifecycle<SessionAbandonOutcomeV2> {
    let intent = journal.intent().map_err(journal_error)?;
    let successor = intent
        .ids()
        .map_err(|_| SessionLifecycleError::Journal(SessionError::Corrupt))?
        .incarnation_id
        != own_incarnation_id;
    if successor && recovery_policy.is_none() {
        return Ok(SessionAbandonOutcomeV2::SuccessorRequiresRecoveryPolicy);
    }
    let close = match journal.retained_abandonment().map_err(journal_error)? {
        Some(close) => close,
        None => {
            let binding = journal.bootstrap_binding().map_err(journal_error)?;
            let close = KubernetesSessionReservationCloseV2 {
                version: 2,
                operation_id: Uuid::new_v4().to_string(),
                reservation_id: intent.reservation_id().into(),
                reservation_input_hash: binding.reservation_input_hash.clone(),
                expected_revision: 1,
                reason: KubernetesSessionReservationCloseReasonV2::PreparationAbandoned,
            };
            journal.retain_abandonment(&close).map_err(journal_error)?;
            close
        }
    };
    // The last local check before the Store: a full revalidation of the
    // journal against its files (an effect checkpoint name, a replaced or a
    // removed record refuses as Corrupt), whose phase must now be the
    // retained abandonment.
    if journal.phase().map_err(journal_error)? != SessionPhaseV2::Abandoned {
        return Err(SessionLifecycleError::Journal(SessionError::Corrupt));
    }
    settle(store, identity, &close, successor, recovery_policy)
}

/// Resumes the abandonment retained in an observe-only view, after a lost
/// response or a restart. Same branches as `abandon_before_effect`; the view
/// holds no lock and proves nothing about the central state.
/// `recovery_policy` is trusted input, as for `abandon_before_effect`.
pub(crate) fn resume_abandonment(
    store: &Store,
    identity: &BrokerIdentityV1,
    own_incarnation_id: Uuid,
    view: &RetainedSessionViewV2,
    recovery_policy: Option<&KubernetesSessionRecoveryPolicyV2>,
) -> Lifecycle<SessionAbandonOutcomeV2> {
    let Some(close) = view.abandonment() else {
        return Ok(SessionAbandonOutcomeV2::NoRetainedAbandonment);
    };
    let successor = view
        .intent()
        .ids()
        .map_err(|_| SessionLifecycleError::Journal(SessionError::Corrupt))?
        .incarnation_id
        != own_incarnation_id;
    settle(store, identity, close, successor, recovery_policy)
}

fn settle(
    store: &Store,
    identity: &BrokerIdentityV1,
    close: &KubernetesSessionReservationCloseV2,
    successor: bool,
    recovery_policy: Option<&KubernetesSessionRecoveryPolicyV2>,
) -> Lifecycle<SessionAbandonOutcomeV2> {
    if !successor {
        return same_incarnation(store, identity, close);
    }
    let Some(policy) = recovery_policy else {
        return Ok(SessionAbandonOutcomeV2::SuccessorRequiresRecoveryPolicy);
    };
    successor_recovery(store, identity, close, policy)
}

fn receipt(response: KubernetesSessionReservationMutationResponseV2) -> SessionAbandonOutcomeV2 {
    if response.replayed {
        SessionAbandonOutcomeV2::Replayed(response)
    } else {
        SessionAbandonOutcomeV2::Closed(response)
    }
}

/// Reconciliation authorization with the exact retained close, then the
/// existing close. The Store requires the incarnation that reserved.
fn same_incarnation(
    store: &Store,
    identity: &BrokerIdentityV1,
    close: &KubernetesSessionReservationCloseV2,
) -> Lifecycle<SessionAbandonOutcomeV2> {
    let committed = store
        .authorize_kubernetes_session_reconciliation(identity, &close.reservation_id, Some(close))
        .and_then(|authorization| {
            store.close_kubernetes_session_before_effect(identity, &authorization, close)
        });
    match committed {
        Ok(response) => Ok(receipt(response)),
        Err(error) if failure(&error) == Some(KubernetesSessionReservationFailureV2::Conflict) => {
            closed_by_another_operation(store, identity, close)
        }
        Err(error) => Err(control(error)),
    }
}

/// A conflict is classified by one observation, never by a replay loop.
fn closed_by_another_operation(
    store: &Store,
    identity: &BrokerIdentityV1,
    close: &KubernetesSessionReservationCloseV2,
) -> Lifecycle<SessionAbandonOutcomeV2> {
    let observe = store
        .authorize_kubernetes_session_reconciliation(identity, &close.reservation_id, None)
        .map_err(control)?;
    let current = store
        .observe_kubernetes_session(identity, &observe, &close.reservation_id)
        .map_err(control)?;
    let state = &current.snapshot.state;
    match &state.last_close {
        Some(last)
            if state.state == KubernetesSessionReservationStateV2::ClosedBeforeEffect
                && last != close =>
        {
            Ok(SessionAbandonOutcomeV2::ClosedCentrallyByAnotherOperation {
                last_close_operation_id: last.operation_id.clone(),
            })
        }
        _ => Err(SessionLifecycleError::Control(
            KubernetesSessionReservationFailureV2::Conflict,
        )),
    }
}

/// The existing successor path: recovery authorization under the tenant
/// policy, then the before-effect recovery close or the exact receipt lookup.
fn successor_recovery(
    store: &Store,
    identity: &BrokerIdentityV1,
    close: &KubernetesSessionReservationCloseV2,
    policy: &KubernetesSessionRecoveryPolicyV2,
) -> Lifecycle<SessionAbandonOutcomeV2> {
    let not_found = |error: &anyhow::Error| {
        failure(error) == Some(KubernetesSessionReservationFailureV2::NotFound)
    };
    let authorization = match store.authorize_kubernetes_session_recovery(
        identity,
        &close.reservation_id,
        Some(close),
        policy,
    ) {
        Ok(authorization) => authorization,
        Err(error) if not_found(&error) => {
            return Ok(SessionAbandonOutcomeV2::SuccessorLookupNotFound);
        }
        Err(error) => return Err(control(error)),
    };
    let response = match authorization {
        KubernetesSessionRecoveryAuthorizationV2::BeforeEffect(authorization) => {
            store.recover_kubernetes_session_before_effect(identity, &authorization, close, policy)
        }
        KubernetesSessionRecoveryAuthorizationV2::ReceiptLookup(authorization) => store
            .lookup_kubernetes_session_recovery_receipt(identity, &authorization, close, policy),
    };
    match response {
        Ok(response) => Ok(receipt(response)),
        Err(error) if not_found(&error) => Ok(SessionAbandonOutcomeV2::SuccessorLookupNotFound),
        Err(error) => Err(control(error)),
    }
}

/// Descriptive interpretation of one observe-only report. None is an
/// ownership, adoption, terminal or quota statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionInterpretationV2 {
    /// No exact request is retained and no close either, so the session
    /// names were not inspected: whether an object exists there is unknown.
    NoRequestRetained,
    /// The exact request is retained and its names were inspected: nothing at
    /// the Pod name and the retained claim UID, not being deleted, at the
    /// claim name. This directory holds no effect checkpoint. Another
    /// directory or process could exist for the same reservation; the central
    /// fence stays authoritative.
    NoEffectIssuedLocally,
    /// An object exists under the derived Pod name; its ownership is unknown.
    ObjectPresentAtSessionName,
    /// The PVC at the attempt's claim name is absent, has another UID than
    /// the retained claim, or is being deleted.
    WorkspaceClaimChanged,
    /// The close is retained locally; resume the abandonment. The central
    /// state is not observed here.
    AbandonmentRetained,
}

/// Observe-only reconcile report of one journal directory. It performs no
/// Store call, mutation, adoption or quota change.
#[derive(Debug, Serialize)]
pub(crate) struct KubernetesSessionReconcileReportV2 {
    version: u32,
    phase: SessionPhaseV2,
    reservation_id: String,
    attempt_id: String,
    /// Present only when the exact request is retained and its names were
    /// inspected (absent from the serialized report otherwise): names come
    /// from the rendered target, never from a caller.
    #[serde(skip_serializing_if = "Option::is_none")]
    names: Option<KubernetesSessionNameInspectionV2>,
    interpretation: SessionInterpretationV2,
}
impl KubernetesSessionReconcileReportV2 {
    pub(crate) fn phase(&self) -> SessionPhaseV2 {
        self.phase
    }
    pub(crate) fn reservation_id(&self) -> &str {
        &self.reservation_id
    }
    pub(crate) fn attempt_id(&self) -> &str {
        &self.attempt_id
    }
    pub(crate) fn names(&self) -> Option<&KubernetesSessionNameInspectionV2> {
        self.names.as_ref()
    }
    pub(crate) fn interpretation(&self) -> SessionInterpretationV2 {
        self.interpretation
    }
}

fn interpret(
    names: Option<&KubernetesSessionNameInspectionV2>,
    abandoned: bool,
) -> SessionInterpretationV2 {
    if let Some(names) = names {
        if matches!(
            names.pod(),
            KubernetesSessionObjectPresenceV2::Present { .. }
        ) {
            return SessionInterpretationV2::ObjectPresentAtSessionName;
        }
        let claim_deleting = matches!(
            names.workspace_claim(),
            KubernetesSessionObjectPresenceV2::Present {
                deletion_requested: true,
                ..
            }
        );
        if claim_deleting || names.workspace_claim_uid_matches_retained() != Some(true) {
            return SessionInterpretationV2::WorkspaceClaimChanged;
        }
    }
    if abandoned {
        SessionInterpretationV2::AbandonmentRetained
    } else if names.is_none() {
        SessionInterpretationV2::NoRequestRetained
    } else {
        SessionInterpretationV2::NoEffectIssuedLocally
    }
}

/// Reads the journal observe-only (the flock is taken first, so a live owner
/// makes the report `Busy`, and it is released before any network I/O), then
/// refuses another authority before any request, then inspects the session
/// names of a retained request with GETs only. A journal without a retained
/// request issues no request.
pub(crate) async fn inspect_session_report(
    runtime: &KubernetesRuntime,
    root: Directory,
    protection: SessionJournalProtection,
) -> Lifecycle<KubernetesSessionReconcileReportV2> {
    let view =
        tokio::task::spawn_blocking(move || SessionJournal::inspect_retained(root, protection))
            .await
            .map_err(|_| SessionLifecycleError::Journal(SessionError::Unavailable))?
            .map_err(journal_error)?;
    if view.intent().authority().fingerprint() != runtime.binding().fingerprint() {
        return Err(SessionLifecycleError::InvalidContext);
    }
    let attempt_id = view
        .intent()
        .ids()
        .map_err(|_| SessionLifecycleError::Journal(SessionError::Corrupt))?
        .attempt_id;
    let names = match view.request() {
        Some(request) => Some(
            runtime
                .inspect_session_names(request.target())
                .await
                .map_err(SessionLifecycleError::Kubernetes)?,
        ),
        None => None,
    };
    let interpretation = interpret(names.as_ref(), view.abandonment().is_some());
    Ok(KubernetesSessionReconcileReportV2 {
        version: 1,
        phase: view.phase(),
        reservation_id: view.intent().reservation_id().into(),
        attempt_id: attempt_id.to_string(),
        names,
        interpretation,
    })
}

#[cfg(test)]
#[path = "session_lifecycle_tests.rs"]
mod tests;
