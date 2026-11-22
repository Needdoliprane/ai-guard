//! Exact reserved control operations authenticated from the retained registry.
//! No epoch or transaction survives an await; no descriptive DTO grants access.
use super::*;
use crate::api::{
    human_sources::{self, HumanOwnedUsage, OwnedHumanGateway},
    store_io,
};
use crate::{audit::HumanSourceAccess, control::ControlProtocolV1};
use std::sync::Arc;
use tokio::sync::OwnedSemaphorePermit;

mod inspection;
pub(in crate::api) use inspection::OwnedPolicyInspection;

/// Private operation classifier, produced by the reserved Rust transport only.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::api) enum PolicyWriteAction {
    Replace,
    Revoke,
}

/// Header/source admission is not a writer and has no body-derived scope.
pub(in crate::api) struct OwnedPolicyIdentity(OwnedProtocolIdentity);
pub(in crate::api) struct OwnedPolicyWriter {
    // Release the operation before the identity drops its usage.
    _operation: OwnedSemaphorePermit,
    identity: OwnedProtocolIdentity,
    action: PolicyWriteAction,
}
impl OwnedPolicyIdentity {
    pub(in crate::api) fn tenant(&self) -> &TenantState {
        &self.0.tenant
    }
    pub(in crate::api) fn accept(
        self,
        operation: OwnedSemaphorePermit,
        action: PolicyWriteAction,
    ) -> OwnedPolicyWriter {
        OwnedPolicyWriter {
            identity: self.0,
            action,
            _operation: operation,
        }
    }
}

pub(in crate::api) async fn authenticate_policy(
    state: &GatewayState,
    headers: &HeaderMap,
) -> ApiResult<OwnedPolicyIdentity> {
    let identity = authenticate(state, headers).await?;
    let shared = identity.state.io.clone();
    let tenant = identity.tenant.io.clone();
    store_io::run(
        shared,
        Some(tenant),
        store_io::Lane::Control,
        false,
        move || {
            let source = identity
                .owned
                .sources
                .get(identity.index)
                .ok_or_else(human_sources::unavailable)?;
            let epoch = identity
                .owned
                .owner
                .try_read_epoch()
                .map_err(policy_authority_failure)?;
            let access = HumanSourceAccess::read(&epoch, source).map_err(policy_store_failure)?;
            // Only actual source/format admission precedes body polling. This
            // read epoch ends here; it never becomes the post-body WriteEpoch.
            identity
                .tenant
                .store
                .modern_control_write_admission(&access)
                .map_err(policy_store_failure)?;
            epoch.check().map_err(policy_authority_failure)?;
            drop(epoch);
            Ok(OwnedPolicyIdentity(identity))
        },
    )
    .await
}

fn policy_authority_failure(
    error: crate::human_authority::HumanAuthorityFailure,
) -> (StatusCode, Json<Value>) {
    use crate::control::egress_policy::EgressPolicyAdminErrorV1 as E;
    super::super::egress_policy::failure(
        if error == crate::human_authority::HumanAuthorityFailure::GateBusy {
            E::Busy
        } else {
            E::Unavailable
        },
    )
}
fn policy_store_failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    super::super::egress_policy::failure(crate::audit::egress::EgressStoreFailure::admin_error(
        &error,
    ))
}

enum PolicyMutation {
    Replace(crate::control::egress_policy::EgressReplacePolicyRequestV1),
    Revoke(crate::control::egress_policy::EgressRevokePolicyRequestV1),
}
impl PolicyMutation {
    fn scope(&self) -> &crate::control::egress_policy::EgressPolicyExpectedScopeV1 {
        match self {
            Self::Replace(value) => value.expected_scope(),
            Self::Revoke(value) => value.expected_scope(),
        }
    }
}
impl OwnedPolicyWriter {
    pub(in crate::api) fn action(&self) -> PolicyWriteAction {
        self.action
    }
    pub(in crate::api) async fn replace(
        self: Arc<Self>,
        request: crate::control::egress_policy::EgressReplacePolicyRequestV1,
    ) -> ApiResult<crate::audit::egress::EgressPolicyReceiptV1> {
        if self.action != PolicyWriteAction::Replace {
            return Err(human_sources::unavailable());
        }
        self.write(PolicyMutation::Replace(request)).await
    }
    pub(in crate::api) async fn revoke(
        self: Arc<Self>,
        request: crate::control::egress_policy::EgressRevokePolicyRequestV1,
    ) -> ApiResult<crate::audit::egress::EgressPolicyReceiptV1> {
        if self.action != PolicyWriteAction::Revoke {
            return Err(human_sources::unavailable());
        }
        self.write(PolicyMutation::Revoke(request)).await
    }
    async fn write(
        self: Arc<Self>,
        mutation: PolicyMutation,
    ) -> ApiResult<crate::audit::egress::EgressPolicyReceiptV1> {
        let owned = self.identity.owned.clone();
        let shared = self.identity.state.io.clone();
        let tenant_io = self.identity.tenant.io.clone();
        let result = store_io::run_retaining(
            shared,
            Some(tenant_io),
            store_io::Lane::Control,
            false,
            self.identity._usage.clone(),
            move || {
                let identity = &self.identity;
                let owned = &identity.owned;
                if !owned.technical_candidate(&identity.digest)? {
                    return Err(human_sources::unavailable());
                }
                identity
                    .credential
                    .revalidate()
                    .map_err(policy_store_failure)?;
                let source = owned
                    .sources
                    .get(identity.index)
                    .ok_or_else(human_sources::unavailable)?;
                let expected = *owned
                    .discovery_generations
                    .get(identity.index)
                    .ok_or_else(human_sources::unavailable)?;
                let scope = mutation.scope().clone();
                let receipt = {
                    // Accepted work owns the usage/registry/source/operation and
                    // IO permits. An abandoned HTTP future cannot release them.
                    let epoch = owned
                        .owner
                        .try_write_epoch()
                        .map_err(policy_authority_failure)?;
                    let result = match &mutation {
                        PolicyMutation::Replace(value) => {
                            let request = value.store_request().map_err(policy_store_failure)?;
                            identity.tenant.store.egress_replace_policy_scoped(
                                &epoch,
                                source,
                                &identity.credential,
                                &scope,
                                expected,
                                &request,
                            )
                        }
                        PolicyMutation::Revoke(value) => {
                            let request = value.store_request().map_err(policy_store_failure)?;
                            identity.tenant.store.egress_revoke_policy_scoped(
                                &epoch,
                                source,
                                &identity.credential,
                                &scope,
                                expected,
                                &request,
                            )
                        }
                    };
                    if result.as_ref().err().is_some_and(|error| {
                        error.downcast_ref::<crate::audit::egress::EgressStoreFailure>()
                            == Some(&crate::audit::egress::EgressStoreFailure::DiscoveryStale)
                    }) {
                        owned.require_rebuild();
                    }
                    let receipt = result.map_err(policy_store_failure)?;
                    epoch
                        .validate_source(source)
                        .map_err(policy_authority_failure)?;
                    receipt
                }; // True E1 commit, SQLite restoration and WriteEpoch end first.
                #[cfg(test)]
                test_hook::pause(&identity.state)?;
                // A fresh, short read revalidates current binding after the true
                // commit. No test wait consumes or prolongs the write epoch.
                identity
                    .credential
                    .revalidate()
                    .map_err(policy_store_failure)?;
                let epoch = owned
                    .owner
                    .try_read_epoch()
                    .map_err(policy_authority_failure)?;
                let access =
                    HumanSourceAccess::read(&epoch, source).map_err(policy_store_failure)?;
                let current = identity
                    .tenant
                    .store
                    .modern_control_protocol(&access, &identity.credential, expected)
                    .map_err(policy_store_failure)?;
                let Some(current) = current else {
                    owned.require_rebuild();
                    return Err(human_sources::unavailable());
                };
                if current.schema_version != 15
                    || current.control_namespace_uuid.as_deref()
                        != Some(scope.control_namespace_uuid())
                    || identity.tenant.tenant_id != scope.tenant_id()
                    || !owned.technical_candidate(&identity.digest)?
                {
                    return Err(human_sources::unavailable());
                }
                epoch
                    .validate_source(source)
                    .map_err(policy_authority_failure)?;
                epoch.check().map_err(policy_authority_failure)?;
                Ok(receipt)
            },
        )
        .await?;
        if !owned.healthy() {
            return Err(human_sources::unavailable());
        }
        Ok(result)
    }
}

/// Initial identity only. The transport supplies its real operation permit
/// after this bounded authentication and before polling the request body.
pub(in crate::api) struct OwnedProtocolIdentity {
    state: GatewayState,
    owned: Arc<OwnedHumanGateway>,
    tenant: TenantState,
    index: usize,
    digest: [u8; 32],
    credential: AuthenticatedReviewRunnerCredential,
    _usage: Arc<HumanOwnedUsage>,
}

/// Private retained work, not a wire object. Its permit/usage outlive an HTTP
/// observer whenever the actual Store job is still queued or running.
pub(in crate::api) struct OwnedProtocol {
    _operation: OwnedSemaphorePermit,
    identity: OwnedProtocolIdentity,
}

impl OwnedProtocolIdentity {
    pub(in crate::api) fn tenant(&self) -> &TenantState {
        &self.tenant
    }

    pub(in crate::api) fn accept(self, operation: OwnedSemaphorePermit) -> OwnedProtocol {
        OwnedProtocol {
            identity: self,
            _operation: operation,
        }
    }
}

/// Called only by the reserved transport gate, never by the public route.
/// The complete startup candidate index decides collisions without SQL on a
/// neighboring tenant. Revalidation of its generation occurs in the target
/// diagnostic transaction, before reading diagnostic facts.
pub(in crate::api) async fn authenticate(
    state: &GatewayState,
    headers: &HeaderMap,
) -> ApiResult<OwnedProtocolIdentity> {
    let owned = state
        .owned_human
        .as_ref()
        .ok_or_else(human_sources::unavailable)?
        .clone();
    let (tenant, _) = state.control_binding(headers, true)?;
    let token = bearer(headers)
        .ok_or_else(|| super::super::error(StatusCode::UNAUTHORIZED, "Jeton Bearer requis."))?;
    let binding = state
        .credential_index
        .lookup(token)
        .ok_or_else(human_sources::unavailable)?;
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    if !owned.technical_candidate(&digest)? {
        return Err(human_sources::unavailable());
    }
    let usage = owned.admit()?;
    let token = token.to_owned();
    let state = state.scoped(Arc::default());
    store_io::run(
        state.io.clone(),
        Some(tenant.io.clone()),
        store_io::Lane::Control,
        false,
        move || {
            if !owned.technical_candidate(&digest)? {
                return Err(human_sources::unavailable());
            }
            let epoch = owned
                .owner
                .try_read_epoch()
                .map_err(|_| human_sources::unavailable())?;
            let source = owned
                .sources
                .get(binding.tenant_index)
                .ok_or_else(human_sources::unavailable)?;
            epoch
                .validate_source(source)
                .map_err(|_| human_sources::unavailable())?;
            let credential = owned
                .registry
                .authenticate(&token)
                .map_err(|_| human_sources::unavailable())?;
            if credential.tenant_id() != tenant.tenant_id
                || credential.control_namespace_uuid() != source.descriptor().durable_namespace_id
            {
                return Err(human_sources::unavailable());
            }
            epoch.check().map_err(|_| human_sources::unavailable())?;
            drop(epoch);
            Ok(OwnedProtocolIdentity {
                state,
                owned,
                tenant,
                index: binding.tenant_index,
                digest,
                credential,
                _usage: usage,
            })
        },
    )
    .await
}

impl OwnedProtocol {
    pub(in crate::api) async fn read(self: Arc<Self>) -> ApiResult<Json<ControlProtocolV1>> {
        let owned = self.identity.owned.clone();
        let state = self.identity.state.clone();
        let tenant = self.identity.tenant.clone();
        let result = store_io::run_retaining(
            state.io.clone(),
            Some(tenant.io.clone()),
            store_io::Lane::Control,
            false,
            self.identity._usage.clone(),
            move || {
                // The Arc retains the operation permit and accepted usage in
                // this actual job even if the response future is abandoned.
                let identity = &self.identity;
                if !identity.owned.technical_candidate(&identity.digest)? {
                    return Err(human_sources::unavailable());
                }
                identity
                    .credential
                    .revalidate()
                    .map_err(|_| human_sources::unavailable())?;
                let epoch = identity
                    .owned
                    .owner
                    .try_read_epoch()
                    .map_err(|_| human_sources::unavailable())?;
                let source = identity
                    .owned
                    .sources
                    .get(identity.index)
                    .ok_or_else(human_sources::unavailable)?;
                let access = HumanSourceAccess::read(&epoch, source)
                    .map_err(|_| human_sources::unavailable())?;
                let expected = *identity
                    .owned
                    .discovery_generations
                    .get(identity.index)
                    .ok_or_else(human_sources::unavailable)?;
                let snapshot = tenant
                    .store
                    .modern_control_protocol(&access, &identity.credential, expected)
                    .map_err(super::super::human::store_failure)?;
                let Some(snapshot) = snapshot else {
                    identity.owned.require_rebuild();
                    return Err(human_sources::unavailable());
                };
                // with_modern_read has committed, restored the timeout and
                // released the connection mutex before this test-only seam.
                #[cfg(test)]
                test_hook::pause(&state)?;
                identity
                    .credential
                    .revalidate()
                    .map_err(|_| human_sources::unavailable())?;
                epoch
                    .validate_source(source)
                    .map_err(|_| human_sources::unavailable())?;
                epoch.check().map_err(|_| human_sources::unavailable())?;
                if !identity.owned.technical_candidate(&identity.digest)? {
                    return Err(human_sources::unavailable());
                }
                let protocol = ControlProtocolV1 {
                    version: snapshot.version,
                    schema_version: snapshot.schema_version,
                    control_namespace_uuid: snapshot.control_namespace_uuid,
                    tenant_id: identity.credential.tenant_id().to_owned(),
                    role: "runner".into(),
                    protocols: snapshot.protocols,
                    active: snapshot.active,
                };
                protocol
                    .validate()
                    .map_err(|_| human_sources::unavailable())?;
                Ok(Json(protocol))
            },
        )
        .await?;
        // Memory-only closing/admission check; no filesystem/SQLite on Tokio.
        if !owned.healthy() {
            return Err(human_sources::unavailable());
        }
        Ok(result)
    }
}

/// Production contains neither a hook slot nor its rendez-vous. Tests share
/// the slot only through clones of their own GatewayState, never a global.
#[cfg(test)]
pub(in crate::api) mod test_hook {
    use super::*;
    use std::sync::{Mutex, mpsc};

    struct Pause {
        reached: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
        acknowledged: mpsc::Sender<()>,
    }
    impl Pause {
        fn wait_for_release(self, timeout: std::time::Duration) -> ApiResult<()> {
            self.reached
                .send(())
                .map_err(|_| human_sources::unavailable())?;
            self.release
                .recv_timeout(timeout)
                .map_err(|_| human_sources::unavailable())?;
            // This witness is emitted by the actual job only after a
            // successful receive. Cleanup-only callers may drop its receiver.
            let _ = self.acknowledged.send(());
            Ok(())
        }
    }
    #[derive(Default)]
    pub(in crate::api) struct Hook {
        next: Mutex<Option<Pause>>,
    }
    pub(crate) struct Release {
        sender: Option<mpsc::Sender<()>>,
        acknowledged: Option<mpsc::Receiver<()>>,
    }
    impl Release {
        pub(crate) fn release(self) {
            drop(self);
        }
        pub(crate) fn release_with_ack(
            mut self,
        ) -> Result<mpsc::Receiver<()>, mpsc::SendError<()>> {
            self.sender.take().unwrap().send(())?;
            Ok(self.acknowledged.take().unwrap())
        }
    }
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(sender) = self.sender.take() {
                let _ = sender.send(());
            }
        }
    }
    fn rendezvous() -> (Pause, mpsc::Receiver<()>, Release) {
        let (reached, observer) = mpsc::channel();
        let (release, waiter) = mpsc::channel();
        let (acknowledged, ack) = mpsc::channel();
        (
            Pause {
                reached,
                release: waiter,
                acknowledged,
            },
            observer,
            Release {
                sender: Some(release),
                acknowledged: Some(ack),
            },
        )
    }
    pub(crate) fn install(state: &GatewayState) -> (mpsc::Receiver<()>, Release) {
        let (pause, observer, release) = rendezvous();
        let mut next = state.owned_control_hook.next.lock().unwrap();
        assert!(next.is_none(), "owned diagnostic hook already installed");
        *next = Some(pause);
        (observer, release)
    }
    pub(super) fn pause(state: &GatewayState) -> ApiResult<()> {
        let next = state
            .owned_control_hook
            .next
            .lock()
            .map_err(|_| human_sources::unavailable())?
            .take();
        if let Some(next) = next {
            next.wait_for_release(std::time::Duration::from_secs(15))?;
        }
        Ok(())
    }

    #[test]
    fn release_ack_requires_successful_waiter_receive() {
        use mpsc::TryRecvError;

        let (waiter, reached, release) = rendezvous();
        let ack = release.release_with_ack().unwrap();
        assert!(matches!(ack.try_recv(), Err(TryRecvError::Empty)));
        waiter.wait_for_release(std::time::Duration::ZERO).unwrap();
        assert!(reached.try_recv().is_ok());
        assert!(ack.try_recv().is_ok());

        let (waiter, _reached, release) = rendezvous();
        let ack = release.release_with_ack().unwrap();
        // A successful send alone proves nothing about the waiter's receive.
        assert!(matches!(ack.try_recv(), Err(TryRecvError::Empty)));
        drop(waiter);
        assert!(matches!(ack.try_recv(), Err(TryRecvError::Disconnected)));

        let (waiter, reached, release) = rendezvous();
        // Zero is an injected helper-only timeout, without an elapsed-time
        // oracle or changing the actual hook's fixed 15-second bound.
        assert!(waiter.wait_for_release(std::time::Duration::ZERO).is_err());
        assert!(reached.try_recv().is_ok());
        assert!(matches!(
            release.acknowledged.as_ref().unwrap().try_recv(),
            Err(TryRecvError::Disconnected)
        ));
        assert!(release.release_with_ack().is_err());
    }
}
