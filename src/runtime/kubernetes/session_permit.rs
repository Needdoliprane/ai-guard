//! Production effect permit of the sealed `SessionEffectGuardV2` (S5).
//!
//! A permit exists only for one central claim acknowledgement whose
//! disposition is `EmitOnce`, after the broker journal retained it
//! (`RetainedEffectClaimV2`, constructible only by the journal module). It
//! is consumed by value by the single transport call that sends its request,
//! and it refuses, synchronously and before any byte is sent: another effect
//! kind, another request digest, another authority, an elapsed central
//! window, and a raised cancellation (CONTRACT S5 obligation 5).
//!
//! The window is the Store's `not_after`, compared with this host's UTC
//! clock: a skewed clock can only shorten or lengthen a window of seconds,
//! never turn an `ObserveOnly` replay into an emission.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "S5 permit without a production caller until the broker driver (K4) lands"
    )
)]
use super::{
    KubernetesAuthorityV1,
    session_effect::{SessionEffectGuardV2, SessionEffectKindV2, sealed},
    session_instance::KubernetesSessionRuntimeError,
};
use crate::control::kubernetes_session::effects::{
    KubernetesSessionEffectActionV2 as Action, KubernetesSessionEffectClaimAckV2,
    KubernetesSessionEffectDispositionV2, KubernetesSessionEffectSubjectV2 as Subject,
};
use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicBool, Ordering};

const REFUSED: KubernetesSessionRuntimeError =
    KubernetesSessionRuntimeError::EffectRefusedBeforeSend;

/// One central `EmitOnce` acknowledgement retained by the broker journal,
/// bound to the journal's authority. Not Clone, not serializable. Its
/// constructor is crate-internal to `runtime::kubernetes`: the type system
/// does not prove the retention, so its only production caller must be the
/// journal write of the acknowledgement (broker driver, K4).
#[derive(Debug)]
pub(in crate::runtime::kubernetes) struct RetainedEffectClaimV2 {
    ack: KubernetesSessionEffectClaimAckV2,
    authority_fingerprint: String,
}
impl RetainedEffectClaimV2 {
    /// To be called only after the exclusive and synced journal write of the
    /// acknowledgement for this authority.
    pub(in crate::runtime::kubernetes) fn retained(
        ack: KubernetesSessionEffectClaimAckV2,
        authority: &KubernetesAuthorityV1,
    ) -> Result<Self, KubernetesSessionRuntimeError> {
        ack.validate().map_err(|_| REFUSED)?;
        authority.validate().map_err(|_| REFUSED)?;
        if ack.disposition != KubernetesSessionEffectDispositionV2::EmitOnce || ack.replayed {
            return Err(REFUSED);
        }
        Ok(Self {
            ack,
            authority_fingerprint: authority.fingerprint(),
        })
    }
}

/// The single production implementor of the sealed effect guard.
#[derive(Debug)]
pub(in crate::runtime::kubernetes) struct CentralEffectPermitV2 {
    kind: SessionEffectKindV2,
    request_sha256: String,
    authority_fingerprint: String,
    not_after: DateTime<Utc>,
}
impl CentralEffectPermitV2 {
    /// Consumes the retained claim: one claim, one permit, one request.
    pub(in crate::runtime::kubernetes) fn from_retained(
        retained: RetainedEffectClaimV2,
    ) -> Result<Self, KubernetesSessionRuntimeError> {
        let request = &retained.ack.request;
        let kind = effect_kind(request.subject, request.action);
        let not_after = DateTime::parse_from_rfc3339(&retained.ack.not_after)
            .map_err(|_| REFUSED)?
            .with_timezone(&Utc);
        Ok(Self {
            kind,
            request_sha256: request.request_sha256.clone(),
            authority_fingerprint: retained.authority_fingerprint,
            not_after,
        })
    }
}
impl sealed::Sealed for CentralEffectPermitV2 {}
impl SessionEffectGuardV2 for CentralEffectPermitV2 {
    fn check_before_send(
        self,
        effect: SessionEffectKindV2,
        request_sha256: &str,
        authority: &KubernetesAuthorityV1,
        cancelled: &AtomicBool,
    ) -> Result<(), KubernetesSessionRuntimeError> {
        if cancelled.load(Ordering::Acquire)
            || effect != self.kind
            || request_sha256 != self.request_sha256
            || authority.validate().is_err()
            || authority.fingerprint() != self.authority_fingerprint
            || Utc::now() >= self.not_after
        {
            return Err(REFUSED);
        }
        Ok(())
    }
}

/// The transport effect each claimed subject/action sends.
pub(in crate::runtime::kubernetes) fn effect_kind(
    subject: Subject,
    action: Action,
) -> SessionEffectKindV2 {
    match (subject, action) {
        (Subject::WorkspaceClaim, Action::Create) => SessionEffectKindV2::CreateClaim,
        (Subject::WorkspaceClaim, _) => SessionEffectKindV2::DeleteClaimExact,
        (_, Action::Create) => SessionEffectKindV2::CreateHeld,
        (_, Action::ReleaseGate) => SessionEffectKindV2::ReleaseGate,
        (_, Action::Attach) => SessionEffectKindV2::Attach,
        (_, Action::ReleaseFinalizer) => SessionEffectKindV2::ReleaseFinalizer,
        (_, Action::Delete) => SessionEffectKindV2::DeleteExact,
    }
}

#[cfg(test)]
#[path = "session_permit_tests.rs"]
mod tests;
