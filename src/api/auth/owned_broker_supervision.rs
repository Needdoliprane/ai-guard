//! Private owned broker-supervision proof for the proposed
//! `broker_supervision_owned_v2` profile (proposal submitted to Root).
//!
//! `authenticate` is called only by the control transport gate, after its
//! exact owned broker target classification and before the presence permit
//! and body polling. It resolves the bearer digest in the complete startup
//! candidate index (no SQL on another tenant), then authenticates the bearer
//! in that single retained source under a real read epoch, in one checked
//! transaction inside the discovery-generation window. Only a current broker
//! binding of that tenant authenticates; a worker, human or technical
//! credential never does, and the technical review credential registry is
//! neither extended nor used.
//!
//! The retained work owns the identity, the server-chosen operation and the
//! presence permit. Each operation moves `Arc<Self>` into one blocking Store
//! job, so the permit and the accepted usage outlive an abandoned HTTP
//! observer. That job checks candidate, coverage and epoch in memory, then
//! runs one checked target transaction that compares the discovery
//! generation, re-authenticates the same bearer, requires the same broker
//! proof, rebuilds and revalidates the engaged identity, does its work and
//! compares the generation again before its commit. Epoch, coverage,
//! candidate and the redaction screen are checked again, in memory, before
//! publication. No epoch, transaction or lock survives an await. Neither the
//! proof nor the work is serializable, clonable or constructible from a DTO.
use super::*;
use crate::{
    api::{
        human_sources::{HumanOwnedUsage, OwnedHumanGateway},
        owned_brokers::{refusal, withheld},
        store_io,
    },
    audit::{HumanSourceAccess, OwnedBrokerWindow, OwnedRetainedSourceFacts, Store},
    broker_registry::{
        BrokerIdentityV1,
        owned_supervision::{
            OwnedBrokerCheckpointRequestV2, OwnedBrokerCheckpointResultV2,
            OwnedBrokerCheckpointSelfViewV2, OwnedBrokerHeartbeatRequestV2,
            OwnedBrokerHeartbeatResultV2, OwnedBrokerIdentityViewV2, OwnedBrokerOperationV2,
            OwnedBrokerSelfViewV2, OwnedChecked, OwnedReconciliationRequestV2,
            OwnedReconciliationResultV2, OwnedSupervisionErrorCodeV2 as Code,
        },
    },
    redaction::Redactor,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::OwnedSemaphorePermit;

/// Bearer bounds of the scoped workload authentication, checked before any
/// Store work so that a malformed bearer never reaches a source.
const MIN_BEARER_BYTES: usize = 32;
const MAX_BEARER_BYTES: usize = 512;

fn require(ok: bool, code: Code) -> OwnedChecked<()> {
    if ok { Ok(()) } else { Err(code) }
}

/// Initial owned broker proof. The transport supplies the server-chosen
/// operation and its presence permit after this bounded authentication and
/// before polling the request body.
pub(in crate::api) struct OwnedBrokerSupervisionIdentity {
    state: GatewayState,
    owned: Arc<OwnedHumanGateway>,
    tenant: TenantState,
    index: usize,
    digest: [u8; 32],
    /// Kept in memory only, to re-authenticate the same bearer in each job.
    token: String,
    /// Gateway dictionary plus this bearer: a positive DTO containing either
    /// is withheld, never published altered.
    redactor: Redactor,
    facts: OwnedRetainedSourceFacts,
    broker: BrokerIdentityV1,
    _usage: Arc<HumanOwnedUsage>,
}

/// Private retained work, not a wire object: the proof, the operation chosen
/// by the gate's route classification and the tenant's presence permit.
pub(in crate::api) struct OwnedBrokerSupervisionWork {
    identity: OwnedBrokerSupervisionIdentity,
    operation: OwnedBrokerOperationV2,
    _permit: OwnedSemaphorePermit,
}

/// Bearer, candidate source and retained source facts presented at the gate,
/// before the Store authenticated anything.
struct Presented {
    state: GatewayState,
    owned: Arc<OwnedHumanGateway>,
    tenant: TenantState,
    index: usize,
    digest: [u8; 32],
    token: String,
    facts: OwnedRetainedSourceFacts,
    usage: Arc<HumanOwnedUsage>,
}

/// Called only by the control transport gate for an exact owned broker
/// target. Every refusal is a closed owned error envelope.
pub(in crate::api) async fn authenticate(
    state: &GatewayState,
    headers: &HeaderMap,
) -> ApiResult<OwnedBrokerSupervisionIdentity> {
    let presented = present(state, headers).map_err(refusal)?;
    let shared = Arc::clone(&presented.state.io);
    let tenant = Arc::clone(&presented.tenant.io);
    let outcome = store_io::run(
        shared,
        Some(tenant),
        store_io::Lane::Read,
        true,
        move || Ok(presented.verify()),
    )
    .await;
    match outcome {
        Ok(verified) => verified.map_err(refusal),
        // Admission refused or the blocking job failed: nothing was accepted
        // and no operation exists yet, so the same request may be retried.
        Err(_) => Err(refusal(Code::SupervisionUnavailable)),
    }
}

/// Memory-only presentation: bearer syntax, complete candidate resolution,
/// the candidate's retained source and its tenant, and accepted usage.
fn present(state: &GatewayState, headers: &HeaderMap) -> OwnedChecked<Presented> {
    let owned = state
        .owned_human
        .clone()
        .ok_or(Code::SupervisionUnavailable)?;
    let token = bearer(headers)
        .filter(|token| (MIN_BEARER_BYTES..=MAX_BEARER_BYTES).contains(&token.len()))
        .ok_or(Code::AuthenticationRequired)?;
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let index = owned.broker_source(&digest)?;
    let tenant = state
        .tenants
        .get(index)
        .ok_or(Code::SourceRebuildRequired)?
        .clone();
    let source = owned
        .sources
        .get(index)
        .ok_or(Code::SourceRebuildRequired)?;
    let facts = OwnedRetainedSourceFacts::from_token(source)?;
    require(
        facts.tenant_id() == tenant.tenant_id,
        Code::SourceRebuildRequired,
    )?;
    let usage = owned.admit().map_err(|_| Code::SupervisionUnavailable)?;
    Ok(Presented {
        state: state.scoped(Arc::default()),
        owned,
        tenant,
        index,
        digest,
        token: token.to_owned(),
        facts,
        usage,
    })
}

impl Presented {
    /// Blocking authentication in the candidate's own retained source, in
    /// one checked transaction inside the discovery-generation window, with
    /// the candidate rechecked after.
    fn verify(self) -> OwnedChecked<OwnedBrokerSupervisionIdentity> {
        let store: &Store = self
            .tenant
            .store
            .sqlite()
            .map_err(|_| Code::SupervisionUnavailable)?;
        require(
            self.owned.broker_source(&self.digest)? == self.index,
            Code::SourceRebuildRequired,
        )?;
        let broker = self.owned.owned_broker_job(
            self.index,
            store,
            &self.token,
            false,
            |access, window| store.owned_window_authenticate(access, window, &self.facts),
        )?;
        require(
            self.owned.broker_source(&self.digest)? == self.index,
            Code::SourceRebuildRequired,
        )?;
        let redactor = self
            .state
            .redactor
            .union(&Redactor::new(vec![self.token.clone()]));
        Ok(OwnedBrokerSupervisionIdentity {
            state: self.state,
            owned: self.owned,
            tenant: self.tenant,
            index: self.index,
            digest: self.digest,
            token: self.token,
            redactor,
            facts: self.facts,
            broker,
            _usage: self.usage,
        })
    }
}

impl OwnedBrokerSupervisionIdentity {
    /// Tenant selected by the authenticated source, for its presence capacity
    /// only; it is never a permission and never replaces a revalidation.
    pub(in crate::api) fn tenant(&self) -> &TenantState {
        &self.tenant
    }

    pub(in crate::api) fn accept(
        self,
        operation: OwnedBrokerOperationV2,
        permit: OwnedSemaphorePermit,
    ) -> OwnedBrokerSupervisionWork {
        OwnedBrokerSupervisionWork {
            identity: self,
            operation,
            _permit: permit,
        }
    }

    /// One blocking job: candidate, then the single target transaction inside
    /// the generation window (re-authentication of the same bearer, same
    /// broker proof, target work), then the publication checks. A refusal
    /// after a mutating target is withheld.
    fn revalidated<T: Serialize>(
        &self,
        mutating: bool,
        target: impl FnOnce(
            &Store,
            &HumanSourceAccess<'_>,
            &OwnedBrokerWindow<'_>,
            &OwnedRetainedSourceFacts,
            &BrokerIdentityV1,
        ) -> OwnedChecked<T>,
    ) -> OwnedChecked<T> {
        let store: &Store = self
            .tenant
            .store
            .sqlite()
            .map_err(|_| Code::SupervisionUnavailable)?;
        require(
            self.owned.broker_source(&self.digest)? == self.index,
            Code::SourceRebuildRequired,
        )?;
        let value = self.owned.owned_broker_job(
            self.index,
            store,
            &self.token,
            mutating,
            |access, window| target(store, access, window, &self.facts, &self.broker),
        )?;
        self.publishable(&value)
            .map_err(|code| withheld(mutating, code))?;
        Ok(value)
    }

    /// Before publication: the same complete candidate, and neither this
    /// bearer nor a gateway secret anywhere in the positive DTO.
    fn publishable<T: Serialize>(&self, value: &T) -> OwnedChecked<()> {
        require(
            self.owned.broker_source(&self.digest)? == self.index,
            Code::SourceRebuildRequired,
        )?;
        let json = serde_json::to_value(value).map_err(|_| Code::ResponseUnpublishable)?;
        require(
            self.redactor.value_unclipped(&json) == json,
            Code::ResponseUnpublishable,
        )
    }
}

impl OwnedBrokerSupervisionWork {
    pub(in crate::api) async fn reconcile(
        self: Arc<Self>,
        request: OwnedReconciliationRequestV2,
    ) -> OwnedChecked<OwnedReconciliationResultV2> {
        self.run(
            request.operation(),
            move |store, access, window, facts, broker| {
                store.owned_broker_reconcile(access, window, facts, broker, &request)
            },
        )
        .await
    }

    /// Operation chosen by the transport gate's exact route classification.
    pub(in crate::api) fn operation(&self) -> OwnedBrokerOperationV2 {
        self.operation
    }

    /// Runs one operation in its blocking Store job. The job owns `Arc<Self>`
    /// and therefore the permit and the accepted usage until it completes.
    async fn run<T, F>(
        self: Arc<Self>,
        expected: OwnedBrokerOperationV2,
        target: F,
    ) -> OwnedChecked<T>
    where
        T: Serialize + Send + 'static,
        F: FnOnce(
                &Store,
                &HumanSourceAccess<'_>,
                &OwnedBrokerWindow<'_>,
                &OwnedRetainedSourceFacts,
                &BrokerIdentityV1,
            ) -> OwnedChecked<T>
            + Send
            + 'static,
    {
        require(self.operation == expected, Code::ProtocolMismatch)?;
        let mutating = expected.mutating();
        let owned = Arc::clone(&self.identity.owned);
        let shared = Arc::clone(&self.identity.state.io);
        let tenant = Arc::clone(&self.identity.tenant.io);
        let outcome = store_io::run(
            shared,
            Some(tenant),
            store_io::Lane::Read,
            true,
            move || {
                // The Arc retains the presence permit and the accepted usage in
                // this actual job even if the response future is abandoned.
                Ok(self.identity.revalidated(mutating, target))
            },
        )
        .await;
        let value = match outcome {
            Ok(value) => value?,
            // Capacity is refused before dispatch: nothing ran.
            Err((status, _)) if status == StatusCode::TOO_MANY_REQUESTS => {
                return Err(Code::SupervisionUnavailable);
            }
            // A failed job may have committed a mutation: withhold it.
            Err(_) => return Err(withheld(mutating, Code::SupervisionUnavailable)),
        };
        // Memory-only closing/admission check; no filesystem/SQLite on Tokio.
        owned
            .owned_health()
            .map_err(|code| withheld(mutating, code))?;
        Ok(value)
    }

    pub(in crate::api) async fn identity(
        self: Arc<Self>,
    ) -> OwnedChecked<OwnedBrokerIdentityViewV2> {
        self.run(
            OwnedBrokerOperationV2::Identity,
            |store, access, window, facts, broker| {
                store.owned_window_identity(access, window, facts, broker)
            },
        )
        .await
    }

    pub(in crate::api) async fn self_view(self: Arc<Self>) -> OwnedChecked<OwnedBrokerSelfViewV2> {
        self.run(
            OwnedBrokerOperationV2::SelfView,
            |store, access, window, facts, broker| {
                store.owned_broker_self_view(access, window, facts, broker)
            },
        )
        .await
    }

    pub(in crate::api) async fn heartbeat(
        self: Arc<Self>,
        request: OwnedBrokerHeartbeatRequestV2,
    ) -> OwnedChecked<OwnedBrokerHeartbeatResultV2> {
        self.run(
            OwnedBrokerOperationV2::Heartbeat,
            move |store, access, window, facts, broker| {
                store.owned_broker_heartbeat(access, window, facts, broker, &request)
            },
        )
        .await
    }

    pub(in crate::api) async fn checkpoint(
        self: Arc<Self>,
        request: OwnedBrokerCheckpointRequestV2,
    ) -> OwnedChecked<OwnedBrokerCheckpointResultV2> {
        self.run(
            OwnedBrokerOperationV2::Checkpoint,
            move |store, access, window, facts, broker| {
                store.owned_broker_checkpoint(access, window, facts, broker, &request)
            },
        )
        .await
    }

    pub(in crate::api) async fn checkpoint_self(
        self: Arc<Self>,
    ) -> OwnedChecked<OwnedBrokerCheckpointSelfViewV2> {
        self.run(
            OwnedBrokerOperationV2::CheckpointSelf,
            |store, access, window, facts, broker| {
                store.owned_broker_checkpoint_self(access, window, facts, broker)
            },
        )
        .await
    }
}
