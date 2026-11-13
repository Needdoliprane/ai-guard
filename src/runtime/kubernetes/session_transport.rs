//! Fixed V2 session effect routes sharing the retained HTTPS client and slots.
//!
//! The caller supplies no URL, method, header or query. A raised cancellation
//! and the consumed guard are checked synchronously after the request is
//! built and immediately before its initiation; once initiated, every failure
//! is an uncertain effect.
//!
//! `allow`, not `expect`: `send_session_effect` is called by the effect
//! functions of `session_effect`, which are themselves dead outside tests
//! and carry an `expect(dead_code)`. Whether rustc then counts this callee as
//! dead depends on how its dead-code pass treats callees of expected items,
//! which was not verified with a compiler here; an `expect` that ends up
//! unfulfilled would fail `clippy -D warnings`, so this module stays robust
//! under both readings.
#![cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "V2 effect transport without a production caller or guard (DESIGN section 0, layer D)"
    )
)]
use super::{
    MAX_CREATE_REQUEST_BYTES, MAX_RESPONSE_BYTES, Transport, read_response_bytes_limited,
    transport_error,
};
use crate::runtime::kubernetes::{
    KubernetesAuthorityV1, KubernetesError,
    session_effect::{
        SessionEffectGuardV2, SessionHttpEffectV2, SessionHttpResponseV2, effect_request_sha256,
    },
    session_instance::KubernetesSessionRuntimeError,
};
use reqwest::Method;
use std::sync::atomic::{AtomicBool, Ordering};

/// Release and delete bodies are a few hundred bytes.
const MAX_SMALL_EFFECT_BYTES: usize = 4096;

impl Transport {
    /// Outer `Err`: refused before initiation, so no request was sent.
    /// `Ok`: the request was initiated and its effect may have happened; the
    /// caller classifies the answer and never retries.
    pub(in crate::runtime::kubernetes) async fn send_session_effect<G: SessionEffectGuardV2>(
        &self,
        binding: &KubernetesAuthorityV1,
        effect: SessionHttpEffectV2<'_>,
        guard: G,
        cancelled: &AtomicBool,
    ) -> Result<SessionHttpResponseV2, KubernetesSessionRuntimeError> {
        let refused = KubernetesSessionRuntimeError::EffectRefusedBeforeSend;
        binding.validate().map_err(|_| refused)?;
        let namespace = binding.namespace();
        if !super::super::valid_dns_label(namespace)
            || self.endpoint != binding.endpoint.trim_end_matches('/')
        {
            return Err(refused);
        }
        let fingerprint = binding.fingerprint();
        let kind = effect.kind();
        let (method, path, content_type, body, digest) = match effect {
            SessionHttpEffectV2::CreateHeld { request } => {
                let target = request.target();
                if target.authority_fingerprint() != fingerprint
                    || request.bytes().len() > MAX_CREATE_REQUEST_BYTES
                {
                    return Err(refused);
                }
                // The central claim binds the rendered request digest.
                (
                    Method::POST,
                    format!("/api/v1/namespaces/{namespace}/pods?fieldValidation=Strict"),
                    "application/json",
                    request.bytes().to_vec(),
                    target.request_sha256().to_owned(),
                )
            }
            SessionHttpEffectV2::ReleaseGate { name, body } => {
                if !super::super::valid_dns_label(name) || body.len() > MAX_SMALL_EFFECT_BYTES {
                    return Err(refused);
                }
                let path =
                    format!("/api/v1/namespaces/{namespace}/pods/{name}?fieldValidation=Strict");
                let digest = effect_request_sha256(kind, &fingerprint, "PATCH", &path, &body);
                (
                    Method::PATCH,
                    path,
                    "application/json-patch+json",
                    body,
                    digest,
                )
            }
            SessionHttpEffectV2::DeleteExact { name, body } => {
                if !super::super::valid_dns_label(name) || body.len() > MAX_SMALL_EFFECT_BYTES {
                    return Err(refused);
                }
                let path = format!("/api/v1/namespaces/{namespace}/pods/{name}");
                let digest = effect_request_sha256(kind, &fingerprint, "DELETE", &path, &body);
                (Method::DELETE, path, "application/json", body, digest)
            }
        };
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| KubernetesSessionRuntimeError::Kubernetes(KubernetesError::Busy))?;
        self.credentials
            .revalidate()
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        let request = self
            .client
            .request(method, format!("{}{path}", self.endpoint))
            .header("accept", "application/json")
            .header("content-type", content_type)
            .body(body)
            .build()
            .map_err(|_| refused)?;
        // Construction precedes the consumed guard. The transport refuses a
        // cancellation raised during the awaited anchor reads itself, whatever
        // the guard checks; no await or further check intervenes before
        // initiating the already built request.
        if cancelled.load(Ordering::Acquire) {
            return Err(refused);
        }
        guard.check_before_send(kind, &digest, binding, cancelled)?;
        let response = match self.client.execute(request).await {
            Ok(response) => response,
            Err(error) => {
                return Ok(SessionHttpResponseV2::Uncertain {
                    status: None,
                    error: transport_error(error),
                });
            }
        };
        let status = response.status().as_u16();
        Ok(
            match read_response_bytes_limited(response, MAX_RESPONSE_BYTES).await {
                Ok(bytes) => SessionHttpResponseV2::Complete { status, bytes },
                Err(error) => SessionHttpResponseV2::Uncertain {
                    status: Some(status),
                    error,
                },
            },
        )
    }
}
