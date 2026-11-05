//! Internal one-shot held CREATE. Production activation remains unavailable;
//! returned observations grant no handle or mutation authority by themselves.
use super::{journal::AdmissionJournal, permission::FreshKubernetesCreatePermission};
use crate::control::kubernetes_admission::{
    KubernetesCreateOutcomeV1, KubernetesCreateUnknownReasonV1,
};
use crate::runtime::kubernetes::transport::HeldCreateResponse;
use crate::runtime::kubernetes::{KubernetesError, KubernetesResult, KubernetesRuntime};
use std::sync::atomic::AtomicBool;

impl KubernetesRuntime {
    pub(crate) async fn create_held(
        &self,
        permission: FreshKubernetesCreatePermission,
        journal: &AdmissionJournal,
        cancelled: &AtomicBool,
    ) -> KubernetesResult<KubernetesCreateOutcomeV1> {
        if &permission.intent().authority != self.binding()
            || permission.target().authority_fingerprint != self.binding().fingerprint()
        {
            return Err(KubernetesError::HeldCreateAuthorizationDenied);
        }
        // The two namespace anchors detect divergence but are not an atomic
        // NamespaceUID precondition on the following Pod CREATE.
        self.verify_authority().await?;
        let intent = permission.intent().clone();
        let bytes = match self
            .transport
            .create_held_pod(&self.binding, permission, journal, cancelled)
            .await?
        {
            HeldCreateResponse::Complete(bytes) => bytes,
            HeldCreateResponse::Uncertain(error) => return Ok(unknown(error)),
        };
        let observed_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let outcome = match super::parse_created_held(&intent, &bytes, &observed_at) {
            Ok(outcome) => outcome,
            Err(error) => return Ok(unknown(error)),
        };
        // Recheck after a safe 201; a changed or unreadable authority cannot
        // become a successful retained observation after the possible effect.
        if let Err(error) = self.verify_authority().await {
            return Ok(unknown(error));
        }
        Ok(outcome)
    }
}

fn unknown(error: KubernetesError) -> KubernetesCreateOutcomeV1 {
    let (reason, http_status) = match error {
        KubernetesError::ApiRejected(status) => {
            (KubernetesCreateUnknownReasonV1::ApiRejected, Some(status))
        }
        KubernetesError::Timeout
        | KubernetesError::TransportUnavailable
        | KubernetesError::Busy => (KubernetesCreateUnknownReasonV1::TransportUnknown, None),
        KubernetesError::AuthorityChanged
        | KubernetesError::InvalidAuthority
        | KubernetesError::InvalidCredentials => {
            (KubernetesCreateUnknownReasonV1::AuthorityChanged, None)
        }
        KubernetesError::PreflightProfileChanged | KubernetesError::PodChanged => {
            (KubernetesCreateUnknownReasonV1::ProfileChanged, None)
        }
        _ => (KubernetesCreateUnknownReasonV1::ResponseInvalid, None),
    };
    KubernetesCreateOutcomeV1::OwnershipUnknown {
        reason,
        http_status,
    }
}
