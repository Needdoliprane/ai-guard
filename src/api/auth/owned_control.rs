//! Exact reserved diagnostic, authenticated from the retained technical registry.
//! No epoch or transaction survives an await; no descriptive DTO grants access.
use super::*;
use crate::api::{
    human_sources::{self, HumanOwnedUsage, OwnedHumanGateway},
    store_io,
};
use crate::{audit::HumanSourceAccess, control::ControlProtocolV1};
use std::sync::Arc;
use tokio::sync::OwnedSemaphorePermit;

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
    identity: OwnedProtocolIdentity,
    _operation: OwnedSemaphorePermit,
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
        let result = store_io::run(
            state.io.clone(),
            Some(tenant.io.clone()),
            store_io::Lane::Control,
            false,
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
    }
    #[derive(Default)]
    pub(in crate::api) struct Hook {
        next: Mutex<Option<Pause>>,
    }
    pub(crate) struct Release(Option<mpsc::Sender<()>>);
    impl Release {
        pub(crate) fn release(self) {
            drop(self);
        }
    }
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }
    pub(crate) fn install(state: &GatewayState) -> (mpsc::Receiver<()>, Release) {
        let (reached, observer) = mpsc::channel();
        let (release, waiter) = mpsc::channel();
        let mut next = state.owned_control_hook.next.lock().unwrap();
        assert!(next.is_none(), "owned diagnostic hook already installed");
        *next = Some(Pause {
            reached,
            release: waiter,
        });
        (observer, Release(Some(release)))
    }
    pub(super) fn pause(state: &GatewayState) -> ApiResult<()> {
        let next = state
            .owned_control_hook
            .next
            .lock()
            .map_err(|_| human_sources::unavailable())?
            .take();
        if let Some(next) = next {
            next.reached
                .send(())
                .map_err(|_| human_sources::unavailable())?;
            next.release
                .recv_timeout(std::time::Duration::from_secs(15))
                .map_err(|_| human_sources::unavailable())?;
        }
        Ok(())
    }
}
