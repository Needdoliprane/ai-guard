//! Provenance of one bounded response to the exact held CREATE POST.
//!
//! This is HTTP provenance only: no effect permit, ownership or Running proof.
//! Only the containing HTTP transport can construct it, after the real POST
//! returned 201. Raw bodies, GETs and durable records cannot construct it.
use crate::runtime::kubernetes::{
    KubernetesError,
    session_claim::{KubernetesSessionClaimRequestV2, KubernetesSessionClaimTargetV2},
    session_instance::KubernetesSessionRuntimeError,
    session_pod::{KubernetesSessionPodRequestV2, KubernetesSessionPodTargetV2},
};
use std::fmt;

/// Affine response bound to the entire target rendered for this POST.
/// Intentionally neither Clone nor serializable/deserializable.
pub(in crate::runtime::kubernetes) struct CreatedSessionResponseV2 {
    target: KubernetesSessionPodTargetV2,
    bytes: Vec<u8>,
}
impl CreatedSessionResponseV2 {
    // Only transport::session and its descendants can call this constructor.
    // Its single production callsite is the real CreateHeld/201 response arm.
    pub(super) fn from_created_post(target: KubernetesSessionPodTargetV2, bytes: Vec<u8>) -> Self {
        Self { target, bytes }
    }

    pub(in crate::runtime::kubernetes) fn into_bytes_for(
        self,
        request: &KubernetesSessionPodRequestV2,
    ) -> Result<Vec<u8>, KubernetesSessionRuntimeError> {
        if &self.target != request.target() {
            return Err(KubernetesSessionRuntimeError::Kubernetes(
                KubernetesError::InvalidResponse,
            ));
        }
        Ok(self.bytes)
    }
}
impl fmt::Debug for CreatedSessionResponseV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Do not expose response contents or target metadata in a diagnostic.
        f.debug_struct("CreatedSessionResponseV2")
            .finish_non_exhaustive()
    }
}

/// Affine response bound to the entire claim target rendered for this POST.
/// Same provenance rule as `CreatedSessionResponseV2`: only the real
/// CreateClaim/201 arm of the transport constructs it.
pub(in crate::runtime::kubernetes) struct CreatedClaimResponseV2 {
    target: KubernetesSessionClaimTargetV2,
    bytes: Vec<u8>,
}
impl CreatedClaimResponseV2 {
    pub(super) fn from_created_post(
        target: KubernetesSessionClaimTargetV2,
        bytes: Vec<u8>,
    ) -> Self {
        Self { target, bytes }
    }

    pub(in crate::runtime::kubernetes) fn into_bytes_for(
        self,
        request: &KubernetesSessionClaimRequestV2,
    ) -> Result<Vec<u8>, KubernetesSessionRuntimeError> {
        if &self.target != request.target() {
            return Err(KubernetesSessionRuntimeError::Kubernetes(
                KubernetesError::InvalidResponse,
            ));
        }
        Ok(self.bytes)
    }
}
impl fmt::Debug for CreatedClaimResponseV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedClaimResponseV2")
            .finish_non_exhaustive()
    }
}
