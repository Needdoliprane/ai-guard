//! Fixed V2 session effect routes sharing the retained HTTPS client and slots.
//!
//! The caller supplies no URL, method, header or query. A raised cancellation
//! and the consumed guard are checked synchronously after the request is
//! built and immediately before its initiation; once initiated, every failure
//! is an uncertain effect.
//!
//! The attach route (stage S2b) goes through the same client: no second
//! client, no redirect and no change of CA, credential or authority. It is an
//! HTTP/1.1 upgrade of one fixed path to the `v5.channel.k8s.io` subprotocol
//! only, with no v4 or SPDY fallback. Attach attempts hold their own slots
//! and never take one of the request slots. The attach route is a plain
//! function: it initiates the built request with one synchronous poll right
//! after the consumed guard, and returns that same initiated request for the
//! owner task of the attempt to await.
//!
//! `allow`, not `expect`: `send_session_effect` is called by the effect
//! functions of `session_effect`, which are themselves dead outside tests
//! and carry an `expect(dead_code)`. Whether rustc then counts this callee as
//! dead depends on how its dead-code pass treats callees of expected items,
//! which was not verified with a compiler here; an `expect` that ends up
//! unfulfilled would fail `clippy -D warnings`, so this module stays robust
//! under both readings. The same holds for the attach route, whose only
//! caller is `session_attach`.
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
    session_attach::{
        ATTACH_HANDSHAKE, ATTACH_SUBPROTOCOL, AttachCapacity, AttachOpeningV2,
        SessionAttachRequestV2, attach_path, validate_attach_upgrade, websocket_key,
    },
    session_effect::{
        SessionEffectGuardV2, SessionEffectKindV2, SessionHttpEffectV2, SessionHttpResponseV2,
        effect_request_sha256,
    },
    session_instance::KubernetesSessionRuntimeError,
};
use reqwest::{
    Method,
    header::{
        CONNECTION, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_PROTOCOL, SEC_WEBSOCKET_VERSION, UPGRADE,
    },
};
use std::{
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll, Waker},
};
use tokio::time::{Instant, timeout_at};

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

    /// One slot of this runtime's attach capacity, or `AttachCapacity`.
    pub(in crate::runtime::kubernetes) fn reserve_attach(
        &self,
    ) -> Result<AttachCapacity, KubernetesSessionRuntimeError> {
        self.attach_permits
            .clone()
            .try_acquire_owned()
            .map(AttachCapacity::new)
            .map_err(|_| KubernetesSessionRuntimeError::AttachCapacity)
    }

    #[cfg(test)]
    pub(in crate::runtime::kubernetes) fn attach_slots_available(&self) -> usize {
        self.attach_permits.available_permits()
    }

    /// Takes every request slot that is free now. Only for the test of the
    /// local contention that makes a request answer `Busy` without being
    /// sent; the slots come back when the permits are dropped.
    #[cfg(test)]
    pub(in crate::runtime::kubernetes) fn hold_request_slots(
        &self,
    ) -> Vec<tokio::sync::SemaphorePermit<'_>> {
        std::iter::from_fn(|| self.permits.try_acquire().ok()).collect()
    }

    /// Exact attach upgrade of one Pod of this authority.
    ///
    /// Outer `Err`: refused before initiation, so no request was sent.
    /// `Ok`: the request was initiated, by one poll made here with no await
    /// since the consumed guard, and the returned opening is that same
    /// request. The caller hands it at once to the owner task of the
    /// attempt, which awaits it to its end; dropping it instead abandons a
    /// request that the server may have accepted. The opening resolves to
    /// the upgraded IO only for a `101` that passed the closed validation;
    /// every other answer, transport failure or elapsed handshake resolves
    /// to its closed error, and nothing is retried.
    ///
    /// Not `async`: no await can stand between the guard and the initiation.
    /// It must be called inside a tokio runtime, which the caller checks
    /// before any request: the client and the handshake bound arm timers.
    ///
    /// The request takes no request slot: the attempt already holds one of
    /// this transport's attach slots, which is checked here. The digest
    /// binds the kind, the authority, `GET`, the fixed path and query, and
    /// the canonical identity-one bytes in the place of a body.
    pub(in crate::runtime::kubernetes) fn send_session_attach<G: SessionEffectGuardV2>(
        &self,
        binding: &KubernetesAuthorityV1,
        attach: SessionAttachRequestV2<'_>,
        guard: G,
        cancelled: &AtomicBool,
    ) -> Result<AttachOpeningV2, KubernetesSessionRuntimeError> {
        let refused = KubernetesSessionRuntimeError::EffectRefusedBeforeSend;
        binding.validate().map_err(|_| refused)?;
        let namespace = binding.namespace();
        if !super::super::valid_dns_label(namespace)
            || !super::super::valid_dns_label(attach.pod_name)
            || self.endpoint != binding.endpoint.trim_end_matches('/')
            || !attach.attempt.holds_capacity_of(&self.attach_permits)
        {
            return Err(refused);
        }
        self.credentials
            .revalidate()
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        let key = websocket_key()?;
        let path = attach_path(namespace, attach.pod_name);
        let request = self
            .client
            .get(format!("{}{path}", self.endpoint))
            .header(CONNECTION, "Upgrade")
            .header(UPGRADE, "websocket")
            .header(SEC_WEBSOCKET_VERSION, "13")
            .header(SEC_WEBSOCKET_KEY, key.as_str())
            .header(SEC_WEBSOCKET_PROTOCOL, ATTACH_SUBPROTOCOL)
            .build()
            .map_err(|_| refused)?;
        let digest = effect_request_sha256(
            SessionEffectKindV2::Attach,
            &binding.fingerprint(),
            "GET",
            &path,
            attach.identity_one,
        );
        // The handshake never outlives the caller deadline.
        let deadline = attach.until.min(Instant::now() + ATTACH_HANDSHAKE);
        // Same order as `send_session_effect`: the transport refuses a raised
        // cancellation itself, then the consumed guard decides, and no await
        // or further check intervenes before initiating the built request.
        if cancelled.load(Ordering::Acquire) {
            return Err(refused);
        }
        guard.check_before_send(SessionEffectKindV2::Attach, &digest, binding, cancelled)?;
        attach.attempt.admit_initiation();
        let pending = self.client.execute(request);
        // The whole opening owns its request, its bound and its key: it
        // borrows nothing from this call and can be awaited by another task.
        let mut opening: AttachOpeningV2 = Box::pin(async move {
            let response = match timeout_at(deadline, pending).await {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => {
                    return Err(KubernetesSessionRuntimeError::Kubernetes(transport_error(
                        error,
                    )));
                }
                Err(_) => return Err(KubernetesSessionRuntimeError::AttachDeadline),
            };
            validate_attach_upgrade(response.status().as_u16(), response.headers(), &key)?;
            match timeout_at(deadline, response.upgrade()).await {
                Ok(Ok(upgraded)) => Ok(upgraded),
                Ok(Err(_)) => Err(KubernetesSessionRuntimeError::AttachHandshakeRefused),
                Err(_) => Err(KubernetesSessionRuntimeError::AttachDeadline),
            }
        });
        // The initiation itself: one poll, here. The opening is pinned in
        // its box before this poll and never leaves it. A no-op waker is
        // enough: the owner task polls the opening again as soon as it runs,
        // and that poll registers the task's own waker.
        let mut context = Context::from_waker(Waker::noop());
        let first = opening.as_mut().poll(&mut context);
        if let Poll::Ready(ended) = first {
            // Already ended, as after an immediate failure of the client:
            // its answer is kept, and the finished future is not polled
            // again.
            let ended: AttachOpeningV2 = Box::pin(std::future::ready(ended));
            return Ok(ended);
        }
        Ok(opening)
    }
}
