use crate::{
    audit::{AuditCapacityFailureV1, DecisionFailure, EventQuery, Store},
    control::{
        AdmissionRefusalV1, AdmissionV1, ControlFailure, DecisionPhaseV1, DecisionReceiptV1,
        QuotaPolicyV1, WorkerMutationV1,
    },
    jev::{JevBackend, JevClient, SelectionSource},
    model::*,
    policy::{self, Policy},
    redaction::Redactor,
    tenancy::validate_tenant_id,
};
use axum::{
    Json, Router,
    body::Body,
    extract::{
        DefaultBodyLimit, Path, Query, Request, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    convert::Infallible,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[cfg(test)]
mod audit_capacity_tests;
mod auth;
#[cfg(test)]
#[path = "build_identity/http_tests.rs"]
mod build_identity_http_tests;
pub use auth::{
    EnvelopeExpectationsV1, TechnicalCredentialIdentityV1, TechnicalCredentialInput,
    TechnicalCredentialRoleV1, prepare_envelope_configuration,
    validate_technical_credentials_configuration,
};
mod brokers;
mod control_transport;
mod egress_policy;
#[cfg(test)]
mod held_policy_tests;
mod human;
mod human_sources;
pub use human_sources::GatewayHumanRetirement;
#[cfg(test)]
pub(crate) use human_sources::tests::fixture as owned_human_fixture_for_test;
mod human_state;
mod human_streams;
mod kubernetes;
mod kubernetes_sessions;
#[cfg(test)]
mod kubernetes_tests;
mod oidc;
/// Proposed owned broker-supervision routes (`broker_supervision_owned_v2`).
mod owned_brokers;
#[cfg(test)]
mod read_execution_tests;
mod relay;
mod relay_admission;
mod store_io;
mod supervision;
mod workload;
pub(crate) use auth::AuthenticatedReviewRunnerCredential;
pub use auth::GatewayControlRecoveryRetirementV2;
#[cfg(test)]
pub(crate) use auth::authenticate_review_runner_fixture;
#[cfg(test)]
pub(crate) use auth::with_authenticated_control_recovery_for_test;
#[cfg(test)]
pub(crate) use auth::with_control_recovery_auth_pair_for_test;
pub(crate) use auth::{AuthenticatedControlRecoveryV2, ControlRecoveryUsageV2};
use auth::{CredentialIndex, CredentialRole, ReadIdentity, RunnerIdentity, bearer};
pub use oidc::{OidcClaimRuleV1, OidcClaimsMappingV1, OidcGateway, OidcGatewayConfigV1};

type ApiResult<T> = Result<T, (StatusCode, Json<Value>)>;
fn error(status: StatusCode, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"error":message})))
}
fn audit_capacity_failure(failure: &anyhow::Error) -> Option<(StatusCode, Json<Value>)> {
    let (code, message) = match failure.downcast_ref::<AuditCapacityFailureV1>()? {
        AuditCapacityFailureV1::HumanReadBudgetExhausted => (
            "audit_read_budget_exhausted",
            "The cumulative human read audit budget is exhausted. Preserve the audit and contact the operator.",
        ),
        AuditCapacityFailureV1::HumanReadDisabled => (
            "audit_read_budget_exhausted",
            "The configured capacity cannot support protected human read audit budgets. Contact the operator.",
        ),
        AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted => (
            "audit_write_budget_exhausted",
            "The remaining audit capacity is protected by control or identity reserves. Preserve the audit and contact the operator.",
        ),
        AuditCapacityFailureV1::WorkloadAuditBudgetExhausted => (
            "audit_workload_budget_exhausted",
            "The cumulative workload audit budget is exhausted. Preserve the audit and contact the operator.",
        ),
        AuditCapacityFailureV1::WorkloadAuditDisabled => (
            "audit_workload_budget_exhausted",
            "The configured capacity cannot support protected workload audit budgets. Contact the operator.",
        ),
    };
    Some((
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({"error":{"code":code,"message":message,"retry_action":"contact_operator"}})),
    ))
}
fn unavailable(failure: anyhow::Error) -> (StatusCode, Json<Value>) {
    if let Some(response) = audit_capacity_failure(&failure) {
        return response;
    }
    if crate::audit::retention::failure::refusal(&failure).is_some_and(|refusal| {
        refusal.code == crate::audit::retention::failure::code::EVENT_PREDATES_ARCHIVE
    }) {
        return (
            StatusCode::CONFLICT,
            Json(
                json!({"error":{"code":"event_predates_archive","message":"This event is declared at or before the archived audit horizon and could repeat an archived event. Send a new event.","retry_action":"send_new_event"}}),
            ),
        );
    }
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Audit indisponible, incohérent ou plein : opération refusée.",
    )
}
fn decision_error(failure: anyhow::Error) -> (StatusCode, Json<Value>) {
    match failure.downcast_ref::<DecisionFailure>() {
        Some(DecisionFailure::Conflict | DecisionFailure::AmbiguousHistory) => error(
            StatusCode::CONFLICT,
            "Identité de demande déjà utilisée ou historique ambigu.",
        ),
        Some(DecisionFailure::ApprovalRequired) => error(
            StatusCode::FORBIDDEN,
            "Revue initiale durable et hash exact requis.",
        ),
        Some(DecisionFailure::InvalidRequest) => error(
            StatusCode::BAD_REQUEST,
            "Identité ou phase de demande invalide.",
        ),
        None => unavailable(failure),
    }
}
fn decision_response(stored: StoredEvent) -> Json<Value> {
    let receipt = stored.event.data["request_id"]
        .as_str()
        .zip(stored.event.data["request_hash"].as_str())
        .zip(stored.event.data["human_approved"].as_bool())
        .map(|((request_id, request_hash), approved)| DecisionReceiptV1 {
            version: 1,
            request_id: request_id.into(),
            request_hash: request_hash.into(),
            phase: if approved {
                DecisionPhaseV1::Approved
            } else {
                DecisionPhaseV1::Initial
            },
            audit_seq: stored.seq,
            audit_hash: stored.hash.clone(),
        });
    Json(
        json!({"decision":stored.event.decision,"request_hash":stored.event.data["request_hash"],"jev":stored.event.data["jev"],"audit_seq":stored.seq,"audit_hash":stored.hash,"receipt":receipt}),
    )
}
pub fn token_matches(received: &str, expected: &str) -> bool {
    let a = Sha256::digest(received.as_bytes());
    let b = Sha256::digest(expected.as_bytes());
    a.iter()
        .zip(b.iter())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}
#[derive(Clone)]
pub struct TenantGateway {
    pub tenant_id: String,
    pub store: Arc<Store>,
    pub runner_token: String,
    pub viewer_token: String,
}

#[derive(Clone)]
struct TenantState {
    tenant_id: String,
    store: Arc<Store>,
    streams: Arc<tokio::sync::Semaphore>,
    human_streams: Arc<human_streams::Admission>,
    rate: Arc<Mutex<(Instant, u32)>>,
    ingestion_rate: Arc<Mutex<(Instant, u32)>>,
    worker_quota: QuotaPolicyV1,
    held_create_policy:
        Option<Arc<crate::control::kubernetes_admission::KubernetesHeldCreatePolicyV1>>,
    session_reservation_policy:
        Option<Arc<crate::control::kubernetes_session::KubernetesSessionReservationPolicyV2>>,
    session_recovery_policy:
        Option<Arc<crate::control::kubernetes_session::KubernetesSessionRecoveryPolicyV2>>,
    session_control_recovery_policy:
        Option<Arc<crate::control::kubernetes_session::KubernetesSessionControlRecoveryPolicyV2>>,
    io: Arc<store_io::Gate>,
    shared_io: Arc<store_io::Gate>,
    control_proof: Option<Arc<ControlProof>>,
    control_capacity: Arc<control_transport::ControlCapacity>,
}
struct ControlProof {
    state: GatewayState,
    headers: HeaderMap,
    write: bool,
    lane: store_io::Lane,
}
impl TenantState {
    fn from_gateway(tenant: TenantGateway, shared_io: Arc<store_io::Gate>) -> Self {
        Self {
            tenant_id: tenant.tenant_id,
            store: tenant.store,
            streams: Arc::new(tokio::sync::Semaphore::new(
                human_streams::TOTAL_STREAM_LIMIT,
            )),
            human_streams: Arc::new(human_streams::Admission::new()),
            rate: Arc::new(Mutex::new((Instant::now(), 0))),
            ingestion_rate: Arc::new(Mutex::new((Instant::now(), 0))),
            worker_quota: QuotaPolicyV1::default(),
            held_create_policy: None,
            session_reservation_policy: None,
            session_recovery_policy: None,
            session_control_recovery_policy: None,
            io: Arc::new(store_io::Gate::new(false)),
            shared_io,
            control_proof: None,
            control_capacity: Arc::new(control_transport::ControlCapacity::new()),
        }
    }
}

#[derive(Clone)]
pub struct GatewayState {
    tenants: Arc<Vec<TenantState>>,
    credential_index: Arc<CredentialIndex>,
    valid_credentials: bool,
    policy: Arc<Policy>,
    jev: Option<Arc<JevBackend>>,
    redactor: Arc<Redactor>,
    human: Arc<human::HumanAccessState>,
    oidc: Option<Arc<OidcGateway>>,
    io: Arc<store_io::Gate>,
    io_scope: Arc<store_io::Scope>,
    control_recovery_epoch: Arc<auth::ControlRecoveryAuthEpochV2>,
    owned_human: Option<Arc<human_sources::OwnedHumanGateway>>,
    #[cfg(test)]
    owned_control_hook: Arc<auth::owned_control::test_hook::Hook>,
}
impl GatewayState {
    fn scoped(&self, scope: Arc<store_io::Scope>) -> Self {
        let mut state = self.clone();
        state.io_scope = scope;
        state
    }
    /// OIDC/discovery work with no authenticated tenant yet. Callers must move
    /// owned input into this closure and revalidate registry/session expiry
    /// inside it. Never hold a registry guard across this await.
    pub(super) async fn blocking_identity<T, F>(&self, operation: F) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(GatewayState) -> ApiResult<T> + Send + 'static,
    {
        let usage = self
            .owned_human
            .as_ref()
            .map(|owned| owned.admit())
            .transpose()?;
        let state = self.scoped(Arc::default());
        store_io::run(
            Arc::clone(&self.io),
            None,
            store_io::Lane::Discovery,
            false,
            move || {
                let _usage = usage;
                operation(state)
            },
        )
        .await
    }
    pub fn with_oidc(mut self, oidc: OidcGateway) -> anyhow::Result<Self> {
        anyhow::ensure!(
            self.owned_human.is_none(),
            "owned human OIDC handlers are not adopted"
        );
        for tenant in self.tenants.iter() {
            oidc.audit.ensure_distinct_from_business(&tenant.store)?;
        }
        let version = oidc.audit.format_version()?;
        anyhow::ensure!(
            matches!(version, 3 | 4),
            "authentication audit requires an explicit supported-format migration"
        );
        if self.tenants.len() > 1 || version == 4 {
            let tenants = self
                .tenants
                .iter()
                .map(|tenant| tenant.tenant_id.clone())
                .collect();
            oidc.audit.validate_capacity_tenants(&tenants)?;
        }
        self.oidc = Some(Arc::new(oidc));
        Ok(self)
    }
    /// Quotas come only from trusted server configuration, never request data.
    pub fn with_worker_quotas(
        mut self,
        quotas: Vec<(String, QuotaPolicyV1)>,
    ) -> anyhow::Result<Self> {
        let mut configured = HashSet::new();
        let tenants = Arc::make_mut(&mut self.tenants);
        for (identity, quota) in quotas {
            quota.validate()?;
            anyhow::ensure!(
                configured.insert(identity.clone()),
                "duplicate quota identity"
            );
            let tenant = tenants
                .iter_mut()
                .find(|tenant| tenant.tenant_id == identity)
                .ok_or_else(|| anyhow::anyhow!("quota identity is not registered"))?;
            tenant.worker_quota = quota;
        }
        Ok(self)
    }
    /// Resolve operator policy data for registered identities before serving.
    pub fn with_kubernetes_held_create_policies(
        mut self,
        policies: Vec<(
            String,
            crate::control::kubernetes_admission::KubernetesHeldCreatePolicyV1,
        )>,
    ) -> anyhow::Result<Self> {
        let mut configured = HashSet::new();
        let tenants = Arc::make_mut(&mut self.tenants);
        for (identity, policy) in policies {
            let policy = policy.canonicalized()?;
            anyhow::ensure!(
                configured.insert(identity.clone()),
                "duplicate held CREATE policy identity"
            );
            let tenant = tenants
                .iter_mut()
                .find(|tenant| tenant.tenant_id == identity)
                .ok_or_else(|| anyhow::anyhow!("held CREATE policy identity is not registered"))?;
            tenant.held_create_policy = Some(Arc::new(policy));
        }
        Ok(self)
    }

    /// Trusted server configuration only; absence leaves reservation disabled.
    pub fn with_kubernetes_session_reservation_policies(
        mut self,
        policies: Vec<(
            String,
            crate::control::kubernetes_session::KubernetesSessionReservationPolicyV2,
        )>,
    ) -> anyhow::Result<Self> {
        let mut configured = HashSet::new();
        let tenants = Arc::make_mut(&mut self.tenants);
        for (identity, policy) in policies {
            let policy = policy.canonicalized()?;
            anyhow::ensure!(
                configured.insert(identity.clone()),
                "duplicate session reservation policy identity"
            );
            let tenant = tenants
                .iter_mut()
                .find(|tenant| tenant.tenant_id == identity)
                .ok_or_else(|| {
                    anyhow::anyhow!("session reservation policy identity is not registered")
                })?;
            tenant.session_reservation_policy = Some(Arc::new(policy));
        }
        Ok(self)
    }

    /// Broker recovery and control recovery are separate opt-in policies.
    pub fn with_kubernetes_session_recovery_policies(
        mut self,
        policies: Vec<(
            String,
            crate::control::kubernetes_session::KubernetesSessionRecoveryPolicyV2,
        )>,
    ) -> anyhow::Result<Self> {
        let mut configured = HashSet::new();
        let tenants = Arc::make_mut(&mut self.tenants);
        for (identity, policy) in policies {
            let policy = policy.canonicalized()?;
            anyhow::ensure!(
                configured.insert(identity.clone()),
                "duplicate session recovery policy identity"
            );
            let tenant = tenants
                .iter_mut()
                .find(|tenant| tenant.tenant_id == identity)
                .ok_or_else(|| {
                    anyhow::anyhow!("session recovery policy identity is not registered")
                })?;
            tenant.session_recovery_policy = Some(Arc::new(policy));
        }
        Ok(self)
    }

    /// Only this policy enables the shared Runner's no-effect recovery route.
    pub fn with_kubernetes_session_control_recovery_policies(
        mut self,
        policies: Vec<(
            String,
            crate::control::kubernetes_session::KubernetesSessionControlRecoveryPolicyV2,
        )>,
    ) -> anyhow::Result<Self> {
        let mut configured = HashSet::new();
        let tenants = Arc::make_mut(&mut self.tenants);
        for (identity, policy) in policies {
            let policy = policy.canonicalized()?;
            anyhow::ensure!(
                configured.insert(identity.clone()),
                "duplicate session control recovery policy identity"
            );
            let tenant = tenants
                .iter_mut()
                .find(|tenant| tenant.tenant_id == identity)
                .ok_or_else(|| {
                    anyhow::anyhow!("session control recovery policy identity is not registered")
                })?;
            tenant.session_control_recovery_policy = Some(Arc::new(policy));
        }
        Ok(self)
    }

    /// Invalid identity configuration denies every authenticated route.
    pub fn new(
        store: Arc<Store>,
        runner_token: String,
        viewer_token: String,
        policy: Policy,
        jev: Option<JevClient>,
        redactor: Redactor,
    ) -> Self {
        Self::new_with_backend(
            store,
            runner_token,
            viewer_token,
            policy,
            jev.map(|client| JevBackend::typesafe(client, SelectionSource::Programmatic)),
            redactor,
        )
    }
    pub fn new_with_backend(
        store: Arc<Store>,
        runner_token: String,
        viewer_token: String,
        policy: Policy,
        jev: Option<JevBackend>,
        redactor: Redactor,
    ) -> Self {
        let io = Arc::new(store_io::Gate::new(true));
        let redactor = redactor.union(&Redactor::new(vec![
            runner_token.clone(),
            viewer_token.clone(),
        ]));
        let redactor = jev
            .as_ref()
            .map_or_else(|| redactor.clone(), |backend| backend.redactor(&redactor));
        let redactor = store.redactor_union(&redactor);
        let jev_separation = jev.as_ref().is_none_or(|client| {
            client
                .validate_control_credentials([runner_token.as_str(), viewer_token.as_str()])
                .is_ok()
                && client.validate_redaction(&redactor).is_ok()
        });
        let store = match store.with_additional_redaction(&redactor) {
            Ok(store) => store,
            Err(_) => {
                return Self {
                    tenants: Arc::new(Vec::new()),
                    credential_index: Arc::new(CredentialIndex::default()),
                    valid_credentials: false,
                    policy: Arc::new(policy),
                    jev: jev.map(Arc::new),
                    redactor: Arc::new(redactor),
                    human: Arc::new(human::HumanAccessState::disabled()),
                    oidc: None,
                    io,
                    io_scope: Arc::default(),
                    control_recovery_epoch: Arc::new(auth::ControlRecoveryAuthEpochV2::new()),
                    owned_human: None,
                    #[cfg(test)]
                    owned_control_hook: Arc::default(),
                };
            }
        };
        let tenants = vec![TenantGateway {
            tenant_id: store.tenant_id().into(),
            store,
            runner_token,
            viewer_token,
        }];
        let (credential_index, valid_credentials) = match CredentialIndex::for_tenants(&tenants) {
            Ok(index) => (index, jev_separation),
            Err(_) => (CredentialIndex::default(), false),
        };
        let (human, valid_credentials) = match human::HumanAccessState::for_tenants(
            tenants.iter().map(|tenant| tenant.tenant_id.as_str()),
        ) {
            Ok(human) => (human, valid_credentials),
            Err(_) => (human::HumanAccessState::disabled(), false),
        };
        Self {
            tenants: Arc::new(
                tenants
                    .into_iter()
                    .map(|tenant| TenantState::from_gateway(tenant, Arc::clone(&io)))
                    .collect(),
            ),
            credential_index: Arc::new(credential_index),
            valid_credentials,
            policy: Arc::new(policy),
            jev: jev.map(Arc::new),
            redactor: Arc::new(redactor),
            human: Arc::new(human),
            oidc: None,
            io,
            io_scope: Arc::default(),
            control_recovery_epoch: Arc::new(auth::ControlRecoveryAuthEpochV2::new()),
            owned_human: None,
            #[cfg(test)]
            owned_control_hook: Arc::default(),
        }
    }
    pub fn for_tenants(
        tenants: Vec<TenantGateway>,
        policy: Policy,
        jev: Option<JevClient>,
        redactor: Redactor,
    ) -> anyhow::Result<Self> {
        Self::for_tenants_with_backend(
            tenants,
            policy,
            jev.map(|client| JevBackend::typesafe(client, SelectionSource::Programmatic)),
            redactor,
        )
    }
    pub fn for_tenants_with_backend(
        mut tenants: Vec<TenantGateway>,
        policy: Policy,
        jev: Option<JevBackend>,
        redactor: Redactor,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(!tenants.is_empty(), "at least one tenant is required");
        let redactor = redactor.union(&Redactor::new(
            tenants
                .iter()
                .flat_map(|tenant| [tenant.runner_token.clone(), tenant.viewer_token.clone()])
                .collect(),
        ));
        let redactor = jev
            .as_ref()
            .map_or_else(|| redactor.clone(), |backend| backend.redactor(&redactor));
        let redactor = tenants.iter().fold(redactor, |redactor, tenant| {
            tenant.store.redactor_union(&redactor)
        });
        if let Some(client) = &jev {
            client.validate_control_credentials(
                tenants.iter().flat_map(|tenant| {
                    [tenant.runner_token.as_str(), tenant.viewer_token.as_str()]
                }),
            )?;
            client.validate_redaction(&redactor)?;
        }
        let mut identities = HashSet::new();
        for tenant in &mut tenants {
            validate_tenant_id(&tenant.tenant_id)?;
            anyhow::ensure!(
                identities.insert(tenant.tenant_id.clone()),
                "duplicate tenant identity"
            );
            anyhow::ensure!(
                tenant.store.tenant_id() == tenant.tenant_id,
                "database tenant binding mismatch"
            );
            tenant.store = tenant
                .store
                .with_additional_redaction(&redactor)
                .map_err(|_| anyhow::anyhow!("gateway audit redaction attachment failed"))?;
            anyhow::ensure!(tenant.store.integrity()?.valid, "audit integrity failure");
        }
        let credential_index = CredentialIndex::for_tenants(&tenants)?;
        let human = human::HumanAccessState::for_tenants(
            tenants.iter().map(|tenant| tenant.tenant_id.as_str()),
        )?;
        let io = Arc::new(store_io::Gate::new(true));
        Ok(Self {
            tenants: Arc::new(
                tenants
                    .into_iter()
                    .map(|tenant| TenantState::from_gateway(tenant, Arc::clone(&io)))
                    .collect(),
            ),
            credential_index: Arc::new(credential_index),
            valid_credentials: true,
            policy: Arc::new(policy),
            jev: jev.map(Arc::new),
            redactor: Arc::new(redactor),
            human: Arc::new(human),
            oidc: None,
            io,
            io_scope: Arc::default(),
            control_recovery_epoch: Arc::new(auth::ControlRecoveryAuthEpochV2::new()),
            owned_human: None,
            #[cfg(test)]
            owned_control_hook: Arc::default(),
        })
    }
    fn control_binding(
        &self,
        headers: &HeaderMap,
        write: bool,
    ) -> ApiResult<(TenantState, &'static str)> {
        if !self.valid_credentials {
            return Err(error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Configuration d'identité invalide.",
            ));
        }
        let token = bearer(headers)
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Jeton Bearer requis."))?;
        let binding = self
            .credential_index
            .lookup(token)
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Jeton invalide."))?;
        let tenant = self.tenants.get(binding.tenant_index).ok_or_else(|| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Configuration d'identité invalide.",
            )
        })?;
        if write && binding.role != CredentialRole::Runner {
            return Err(error(
                StatusCode::FORBIDDEN,
                "Cette identité est en lecture seule.",
            ));
        }
        Ok((tenant.clone(), binding.role.as_str()))
    }
    fn authorize_sync(
        &self,
        headers: &HeaderMap,
        write: bool,
    ) -> ApiResult<(TenantState, &'static str)> {
        let (mut tenant, role) = self.control_binding(headers, write)?;
        let token = bearer(headers)
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Jeton Bearer requis."))?;
        let binding = self
            .credential_index
            .lookup(token)
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Jeton invalide."))?;
        let lane = if role == "runner" {
            store_io::Lane::Control
        } else {
            store_io::Lane::Read
        };
        human::reject_control_collision(self, token, Some((binding.tenant_index, lane)))?;
        workload::reject_control_collision(self, token, Some((binding.tenant_index, lane)))?;
        let mut proof_headers = HeaderMap::new();
        if let Some(value) = headers.get(header::AUTHORIZATION) {
            proof_headers.insert(header::AUTHORIZATION, value.clone());
        }
        tenant.control_proof = Some(Arc::new(ControlProof {
            // No task admission lease may escape into a returned HTTP proof.
            state: self.scoped(Arc::default()),
            headers: proof_headers,
            write,
            lane: if role == "runner" {
                store_io::Lane::Control
            } else {
                store_io::Lane::Read
            },
        }));
        Ok((tenant, role))
    }
    async fn authorize(
        &self,
        headers: &HeaderMap,
        write: bool,
    ) -> ApiResult<(TenantState, &'static str)> {
        // Reject malformed/unrecognized credentials before admitting Store work.
        let (tenant, role) = self.control_binding(headers, write)?;
        let lane = if role == "runner" {
            store_io::Lane::Control
        } else {
            store_io::Lane::Read
        };
        let mut scope = store_io::Scope::default();
        scope.hold(&tenant, lane, false)?;
        let state = self.scoped(Arc::new(scope));
        let headers = headers.clone();
        store_io::run(Arc::clone(&self.io), None, lane, false, move || {
            state.authorize_sync(&headers, write)
        })
        .await
    }
}
impl TenantState {
    async fn blocking<T, F>(&self, expensive: bool, operation: F) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(TenantState) -> ApiResult<T> + Send + 'static,
    {
        let tenant = self.clone();
        let lane = self
            .control_proof
            .as_ref()
            .map_or(store_io::Lane::Read, |proof| proof.lane);
        let mut scope = store_io::Scope::default();
        scope.hold(&tenant, lane, expensive)?;
        let scope = Arc::new(scope);
        store_io::run(
            Arc::clone(&self.shared_io),
            None,
            lane,
            expensive,
            move || {
                // Queueing for a blocking worker cannot extend an authorization.
                if let Some(proof) = &tenant.control_proof {
                    proof
                        .state
                        .scoped(Arc::clone(&scope))
                        .authorize_sync(&proof.headers, proof.write)?;
                }
                // Hold the exact Store lease through all legacy/control I/O.
                let _scope = scope;
                operation(tenant)
            },
        )
        .await
    }
    fn write_quota(&self) -> ApiResult<()> {
        Self::take_write_slot(&self.rate)
    }
    fn ingestion_quota(&self) -> ApiResult<()> {
        Self::take_write_slot(&self.ingestion_rate)
    }
    fn take_write_slot(rate: &Mutex<(Instant, u32)>) -> ApiResult<()> {
        let mut state = rate
            .lock()
            .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Limiteur indisponible."))?;
        if state.0.elapsed() >= Duration::from_secs(1) {
            *state = (Instant::now(), 0);
        }
        if state.1 >= 200 {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(
                    json!({"error":{"code":"write_rate_limited","message":"Maximum 200 écritures par seconde."}}),
                ),
            ));
        }
        state.1 += 1;
        Ok(())
    }
}

pub async fn security_headers(request: Request, next: Next) -> Response {
    security_headers_inner(request, next, false, false).await
}
async fn gateway_security_headers(
    State(state): State<GatewayState>,
    request: Request,
    next: Next,
) -> Response {
    let configured = state.oidc.as_ref();
    let callback = configured.is_some()
        && request.method() == "GET"
        && request.uri().path() == "/auth/oidc/callback";
    let trusted_origin = configured.is_some_and(|oidc| {
        oidc::browser_post(request.method(), request.uri().path())
            && oidc::same_origin(request.headers(), &oidc.config.web_origin)
            || request.method() == "GET"
                && request.uri().path() == "/auth/oidc/status"
                && oidc::public_status_origin(request.headers(), &oidc.config.web_origin)
    });
    let response = security_headers_inner(request, next, trusted_origin, callback).await;
    write_rate_retry_header(response).await
}

/// Keep the existing API error tuple compatible across handlers. Only the
/// gateway's bounded, typed rate error carries a retry hint; quota and durable
/// audit-capacity refusals retain their distinct bodies and no Retry-After.
async fn write_rate_retry_header(response: Response) -> Response {
    if response.status() != StatusCode::TOO_MANY_REQUESTS
        || response.headers().get(header::CONTENT_TYPE)
            != Some(&HeaderValue::from_static("application/json"))
    {
        return response;
    }
    let (parts, body) = response.into_parts();
    let mut stream = body.into_data_stream();
    let mut chunks = Vec::new();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let inspect = match &chunk {
            Ok(chunk) => bytes.len().saturating_add(chunk.len()) <= 4096,
            Err(_) => false,
        } && chunks.len() < 16;
        if inspect && let Ok(chunk) = &chunk {
            bytes.extend_from_slice(chunk);
        }
        chunks.push(chunk);
        if !inspect {
            // A body that cannot be inspected safely is still returned intact,
            // including its original chunks/error and every remaining chunk.
            return Response::from_parts(
                parts,
                Body::from_stream(futures_util::stream::iter(chunks).chain(stream)),
            );
        }
    }
    let rate_limited = serde_json::from_slice::<Value>(&bytes).is_ok_and(|body| {
        body.pointer("/error/code").and_then(Value::as_str) == Some("write_rate_limited")
    });
    let mut response =
        Response::from_parts(parts, Body::from_stream(futures_util::stream::iter(chunks)));
    if rate_limited {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    }
    response
}
async fn security_headers_inner(
    request: Request,
    next: Next,
    trusted_origin: bool,
    oidc_navigation: bool,
) -> Response {
    if request.uri().to_string().len() > 4096 {
        return error(StatusCode::URI_TOO_LONG, "URI trop longue.").into_response();
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let safe = origin
            .to_str()
            .ok()
            .and_then(|s| reqwest::Url::parse(s).ok())
            .is_some_and(|u| {
                ["http", "https"].contains(&u.scheme())
                    && u.host_str().is_some_and(|h| {
                        h == "localhost"
                            || h.parse::<std::net::IpAddr>().is_ok_and(|a| a.is_loopback())
                    })
            });
        if !safe && !trusted_origin && !oidc_navigation {
            return error(StatusCode::FORBIDDEN, "Origine non autorisée.").into_response();
        }
    }
    if !oidc_navigation
        && request
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|h| h == "cross-site")
    {
        return error(StatusCode::FORBIDDEN, "Requête cross-site refusée.").into_response();
    }
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("content-security-policy",HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"));
    response
}

/// Public routes plus an explicit control transport on the same origin. Both
/// this nest and the dedicated listener share tenant-owned admission capacity.
pub fn gateway_router(state: GatewayState) -> Router {
    gateway_base_router(state.clone()).nest("/control", gateway_control_router(state))
}

/// Private gateway-root control API, for the fixed internal relay destination.
pub fn gateway_control_router(state: GatewayState) -> Router {
    control_transport::router(state)
}

fn gateway_base_router(state: GatewayState) -> Router {
    let routes = Router::new()
        .route("/events", get(list_events).post(ingest))
        .route("/decide", post(decide))
        .route("/identity", get(identity))
        .route("/overview", get(overview))
        .route("/agents", get(agents))
        .route("/sandboxes", get(sandboxes))
        .route("/stream", get(stream))
        .route("/export", get(export))
        .route("/integrity", get(integrity))
        .route("/workers", get(workers))
        .route("/workers/reservations", post(reserve_worker))
        .route("/workers/operations/{id}", get(worker_operation))
        .route("/workers/{id}", get(worker))
        .route("/workers/{id}/transitions", post(transition_worker))
        .route("/quota", get(worker_quota))
        .route("/usage", get(worker_usage));
    Router::new()
        .route("/healthz", get(gateway_health))
        .route("/api/v1/status", get(gateway_build_status))
        .nest("/auth/oidc", oidc::routes())
        .nest(
            "/api/v1",
            routes
                .clone()
                .route("/control/events", post(ingest_control_event))
                .merge(supervision::routes())
                .merge(egress_policy::router())
                .merge(workload::routes())
                .merge(relay::routes())
                .merge(relay_admission::routes())
                .merge(brokers::routes())
                .merge(owned_brokers::routes())
                .merge(kubernetes::routes())
                .merge(kubernetes_sessions::routes())
                .merge(human::routes()),
        )
        .nest("/api", routes)
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            human_sources::route_gate,
        ))
        .layer(DefaultBodyLimit::max(262_144))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            gateway_security_headers,
        ))
        .with_state(state)
}

async fn gateway_build_status() -> Json<crate::build_identity::BuildStatusV1> {
    Json(crate::build_identity::BuildStatusV1::current(
        crate::build_identity::Component::Gateway,
    ))
}

async fn gateway_health(State(state): State<GatewayState>) -> (StatusCode, Json<Value>) {
    let available = state.valid_credentials
        && !state.tenants.is_empty()
        && state.tenants.iter().all(|tenant| tenant.store.healthy());
    let available = available
        && state
            .owned_human
            .as_ref()
            .is_none_or(|owned| owned.healthy());
    if available {
        (
            StatusCode::OK,
            Json(json!({"status":"ok","service":"gateway","version":env!("CARGO_PKG_VERSION")})),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(
                json!({"status":"unavailable","service":"gateway","reason":"configuration_or_audit_unavailable","version":env!("CARGO_PKG_VERSION")}),
            ),
        )
    }
}

async fn ingest(
    RunnerIdentity(tenant): RunnerIdentity,
    Json(event): Json<Event>,
) -> ApiResult<Json<StoredEvent>> {
    tenant
        .blocking(false, move |tenant| {
            tenant.ingestion_quota()?;
            event
                .validate()
                .map_err(|_| error(StatusCode::BAD_REQUEST, "Événement invalide."))?;
            if crate::audit::D5_CONTROL_EVENT_KINDS.contains(&event.kind.as_str())
                || crate::relay::admission::reserved_kind(&event.kind)
                || ["security.decision", "control", "worker", "usage.allocation"]
                    .iter()
                    .any(|namespace| crate::model::event_kind_in_namespace(&event.kind, namespace))
            {
                return Err(error(
                    StatusCode::FORBIDDEN,
                    "Ce namespace est réservé aux opérations du contrôle.",
                ));
            }
            Ok(Json(tenant.store.append(event).map_err(unavailable)?))
        })
        .await
}

/// The route is the source of delivery authority. Worker-controlled Event
/// fields never promote an ordinary ingestion request to the control budget.
async fn ingest_control_event(
    RunnerIdentity(tenant): RunnerIdentity,
    Json(event): Json<Event>,
) -> ApiResult<Json<StoredEvent>> {
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            event
                .validate()
                .map_err(|_| error(StatusCode::BAD_REQUEST, "Événement invalide."))?;
            if !crate::audit::control_delivery_kind(&event.kind) {
                return Err(error(
                    StatusCode::FORBIDDEN,
                    "Ce fait n'est pas admis sur la route d'audit du contrôle.",
                ));
            }
            Ok(Json(
                tenant
                    .store
                    .append_control_event(event)
                    .map_err(unavailable)?,
            ))
        })
        .await
}

async fn decide(
    RunnerIdentity(tenant): RunnerIdentity,
    State(s): State<GatewayState>,
    Json(request): Json<DecisionRequest>,
) -> ApiResult<Json<Value>> {
    tenant.write_quota()?;
    if !tenant.store.healthy() {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Intégrité de l'audit défaillante : exécution refusée.",
        ));
    }
    let lookup = request.clone();
    if let Some(stored) = tenant
        .blocking(false, move |tenant| {
            tenant.store.decision(&lookup, None).map_err(decision_error)
        })
        .await?
    {
        return Ok(decision_response(stored));
    }
    let (decision, jev) = fresh_policy(&s, &request).await;
    let stored = tenant
        .blocking(false, move |tenant| {
            tenant
                .store
                .decision(&request, Some((decision, jev)))
                .map_err(decision_error)
        })
        .await?
        .ok_or_else(|| error(StatusCode::SERVICE_UNAVAILABLE, "Décision durable absente."))?;
    Ok(decision_response(stored))
}

async fn fresh_policy(s: &GatewayState, request: &DecisionRequest) -> (Decision, Value) {
    let mut decision = policy::evaluate(&s.policy, request);
    let mut jev = s.jev.as_ref().map_or(Value::Null, |backend| {
        backend.metadata("skipped_local_policy")
    });
    if decision.outcome == Outcome::Allow
        && let Some(client) = &s.jev
    {
        match client.evaluate(request, &s.redactor).await {
            Ok(answer) => {
                if answer.outcome != Outcome::Allow {
                    decision = Decision {
                        outcome: Outcome::Deny,
                        engine: if client.id() == "typesafe" {
                            "jev"
                        } else {
                            client.id()
                        }
                        .into(),
                        rule: if client.id() == "typesafe" {
                            if answer.outcome == Outcome::Deny { "jev_deny" } else { "jev_review_fail_closed" }.into()
                        } else { format!("{}_{}", client.id(), if answer.outcome == Outcome::Deny { "deny" } else { "review_fail_closed" }) },
                        reason: match client.id() {
                            "rust_rules" if answer.outcome == Outcome::Deny => "Le catalogue Rust refuse cette invocation exacte ; aucune exécution.",
                            "rust_rules" => "Invocation hors du catalogue Rust incomplet ; revue requise et refus sans exécution.",
                            "local_openai" if answer.outcome == Outcome::Deny => "Le modèle local refuse cette demande ; aucune exécution.",
                            "local_openai" => "Le modèle local n'est pas qualifié pour autoriser ; revue requise et refus sans exécution.",
                            _ => "TypeSafe refuse ou ne confirme pas l'autorisation ; refus sans exécution.",
                        }.into(),
                    };
                }
                jev = answer.metadata;
            }
            Err(failure) => {
                decision = Decision {
                    outcome: Outcome::Deny,
                    engine: if client.id() == "typesafe" {
                        "jev"
                    } else {
                        client.id()
                    }
                    .into(),
                    rule: if client.id() == "typesafe" { "jev_unavailable_or_invalid" } else { "decision_backend_unavailable_or_invalid" }.into(),
                    reason: match client.id() {
                        "rust_rules" => "Catalogue Rust indisponible ou invalide ; vérifier sa configuration, refus par défaut.",
                        "local_openai" => "Réponse du modèle local indisponible ou invalide ; vérifier le raccord local, refus par défaut.",
                        _ => "Réponse TypeSafe indisponible, invalide ou incertaine ; refus par défaut.",
                    }.into(),
                };
                jev = client.metadata("unavailable_or_invalid");
                jev["error"] = json!(failure);
                if let Some(diagnostic) = failure.diagnostic() {
                    jev["diagnostic"] = json!(diagnostic);
                }
            }
        }
    }
    (decision, jev)
}

fn control_error(failure: anyhow::Error) -> (StatusCode, Json<Value>) {
    if let Some(response) = audit_capacity_failure(&failure) {
        return response;
    }
    let (status, code, message) = match failure.downcast_ref::<ControlFailure>() {
        Some(ControlFailure::InvalidRequest) => (
            StatusCode::BAD_REQUEST,
            "invalid_worker_request",
            "Requête de contrôle invalide.",
        ),
        Some(ControlFailure::Conflict) => (
            StatusCode::CONFLICT,
            "worker_conflict",
            "L'opération ou la révision ne correspond plus à l'état durable.",
        ),
        Some(ControlFailure::NotFound) => (
            StatusCode::NOT_FOUND,
            "worker_not_found",
            "Worker ou opération introuvable.",
        ),
        Some(ControlFailure::MigrationRequired) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "migration_required",
            "Migration du registre requise avant les opérations workers.",
        ),
        Some(ControlFailure::SupervisionInactive) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "supervision_inactive",
            "Supervision en maintenance : les nouveaux lancements restent bloqués jusqu'à une réactivation sûre du contrôle.",
        ),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "control_unavailable",
            "Contrôle ou audit indisponible : opération refusée.",
        ),
    };
    (status, Json(json!({"error":message,"code":code})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerPageQuery {
    cursor: Option<String>,
    limit: Option<u32>,
}
impl WorkerPageQuery {
    fn parse(value: Result<Query<Self>, QueryRejection>) -> ApiResult<Self> {
        let Query(page) =
            value.map_err(|_| control_error(ControlFailure::InvalidRequest.into()))?;
        if !(1..=100).contains(&page.limit.unwrap_or(50)) {
            return Err(control_error(ControlFailure::InvalidRequest.into()));
        }
        Ok(page)
    }
}

async fn reserve_worker(
    RunnerIdentity(tenant): RunnerIdentity,
    State(s): State<GatewayState>,
    body: Result<Json<AdmissionV1>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<crate::control::AdmissionResultV1>)> {
    tenant.write_quota()?;
    let Json(admission) = body.map_err(|_| control_error(ControlFailure::InvalidRequest.into()))?;
    admission.validate().map_err(control_error)?;
    // Replays are checked inside the same durable operation. They do not make
    // another external policy call or grant a new start permission.
    let operation_id = admission.operation_id.clone();
    let replay = tenant
        .blocking(false, move |tenant| {
            Ok(tenant
                .store
                .worker_operation(&operation_id)
                .map_err(control_error)?
                .is_some())
        })
        .await?;
    let (decision, jev) = if replay {
        (
            policy::evaluate(&s.policy, &admission.decision_request),
            Value::Null,
        )
    } else {
        fresh_policy(&s, &admission.decision_request).await
    };
    let result = tenant
        .blocking(false, move |tenant| {
            tenant
                .store
                .admit_worker(&admission, &s.policy, decision, jev, &tenant.worker_quota)
                .map_err(control_error)
        })
        .await?;
    let status = match result.refusal {
        Some(AdmissionRefusalV1::Policy) => StatusCode::FORBIDDEN,
        Some(AdmissionRefusalV1::Quota) => StatusCode::TOO_MANY_REQUESTS,
        None if result.replayed => StatusCode::OK,
        None => StatusCode::CREATED,
    };
    Ok((status, Json(result)))
}

async fn transition_worker(
    RunnerIdentity(tenant): RunnerIdentity,
    Path(id): Path<String>,
    body: Result<Json<WorkerMutationV1>, JsonRejection>,
) -> ApiResult<Json<crate::control::MutationResultV1>> {
    tenant
        .blocking(false, move |tenant| {
            tenant.write_quota()?;
            let Json(mutation) =
                body.map_err(|_| control_error(ControlFailure::InvalidRequest.into()))?;
            if mutation.worker_id != id {
                return Err(control_error(ControlFailure::InvalidRequest.into()));
            }
            Ok(Json(
                tenant
                    .store
                    .mutate_worker(&mutation)
                    .map_err(control_error)?,
            ))
        })
        .await
}

async fn workers(
    ReadIdentity(tenant, _): ReadIdentity,
    query: Result<Query<WorkerPageQuery>, QueryRejection>,
) -> ApiResult<Json<Value>> {
    tenant
        .blocking(true, move |tenant| {
            let page = WorkerPageQuery::parse(query)?;
            let limit = page.limit.unwrap_or(50);
            let items = tenant
                .store
                .workers_page(page.cursor.as_deref(), limit)
                .map_err(control_error)?;
            let next_cursor = if items.len() == limit as usize {
                let last = items.last().map(|worker| worker.worker_id.clone());
                if !tenant
                    .store
                    .workers_page(last.as_deref(), 1)
                    .map_err(control_error)?
                    .is_empty()
                {
                    last
                } else {
                    None
                }
            } else {
                None
            };
            Ok(Json(json!({"items":items,"next_cursor":next_cursor})))
        })
        .await
}

async fn worker(
    ReadIdentity(tenant, _): ReadIdentity,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::control::WorkerV1>> {
    tenant
        .blocking(true, move |tenant| {
            let worker = tenant
                .store
                .worker(&id)
                .map_err(control_error)?
                .ok_or_else(|| control_error(ControlFailure::NotFound.into()))?;
            Ok(Json(worker))
        })
        .await
}

async fn worker_operation(
    ReadIdentity(tenant, _): ReadIdentity,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::control::MutationReceiptV1>> {
    tenant
        .blocking(true, move |tenant| {
            Ok(Json(
                tenant
                    .store
                    .worker_operation(&id)
                    .map_err(control_error)?
                    .ok_or_else(|| control_error(ControlFailure::NotFound.into()))?,
            ))
        })
        .await
}

async fn worker_quota(
    ReadIdentity(tenant, _): ReadIdentity,
) -> ApiResult<Json<crate::control::QuotaUsageV1>> {
    tenant
        .blocking(true, move |tenant| {
            Ok(Json(
                tenant
                    .store
                    .worker_quota(&tenant.worker_quota)
                    .map_err(control_error)?,
            ))
        })
        .await
}

async fn worker_usage(
    ReadIdentity(tenant, _): ReadIdentity,
    query: Result<Query<WorkerPageQuery>, QueryRejection>,
) -> ApiResult<Json<Value>> {
    tenant
        .blocking(true, move |tenant| {
            let page = WorkerPageQuery::parse(query)?;
            let limit = page.limit.unwrap_or(50);
            let items = tenant
                .store
                .worker_usage_page(page.cursor.as_deref(), limit)
                .map_err(control_error)?;
            let next_cursor = if items.len() == limit as usize {
                let last = items.last().map(|allocation| allocation.attempt_id.clone());
                if !tenant
                    .store
                    .worker_usage_page(last.as_deref(), 1)
                    .map_err(control_error)?
                    .is_empty()
                {
                    last
                } else {
                    None
                }
            } else {
                None
            };
            Ok(Json(json!({"items":items,"next_cursor":next_cursor})))
        })
        .await
}

async fn list_events(
    ReadIdentity(tenant, _): ReadIdentity,
    Query(q): Query<EventQuery>,
) -> ApiResult<Json<Value>> {
    tenant
        .blocking(true, move |tenant| {
            let items = tenant.store.events(&q).map_err(|failure| {
                if failure.is::<crate::audit::InvalidEventFilter>() {
                    error(StatusCode::BAD_REQUEST, "Filtre invalide.")
                } else {
                    unavailable(failure)
                }
            })?;
            let next = if items.len() == q.page_limit() {
                items.last().map(|e| e.seq)
            } else {
                None
            };
            Ok(Json(json!({"items":items,"next_cursor":next})))
        })
        .await
}
async fn overview(
    ReadIdentity(tenant, _): ReadIdentity,
    State(s): State<GatewayState>,
) -> ApiResult<Json<Value>> {
    tenant
        .blocking(true, move |tenant| {
            let mut value = tenant.store.overview().map_err(unavailable)?;
            // Counters include the archived prefix; say how much of it is archived.
            value["retention"] =
                serde_json::to_value(tenant.store.audit_retention_ledger().map_err(unavailable)?)
                    .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Synthèse indisponible."))?;
            value["health"]["jev"] = json!(if s.jev.is_some() {
                "configured"
            } else {
                "disabled"
            });
            value["health"]["jev_details"] = s.jev.as_ref().map_or(Value::Null, |backend| {
                let mut metadata = backend.metadata("configured");
                metadata["reachability"] = json!(if backend.id() == "rust_rules" {
                    "not_applicable"
                } else {
                    "not_checked"
                });
                metadata
            });
            Ok(Json(value))
        })
        .await
}
async fn agents(ReadIdentity(tenant, _): ReadIdentity) -> ApiResult<Json<Value>> {
    tenant
        .blocking(true, move |tenant| {
            Ok(Json(
                json!({"items":tenant.store.entities(false).map_err(unavailable)?}),
            ))
        })
        .await
}
async fn sandboxes(ReadIdentity(tenant, _): ReadIdentity) -> ApiResult<Json<Value>> {
    tenant
        .blocking(true, move |tenant| {
            Ok(Json(
                json!({"items":tenant.store.entities(true).map_err(unavailable)?}),
            ))
        })
        .await
}
async fn integrity(ReadIdentity(tenant, _): ReadIdentity) -> ApiResult<Json<Value>> {
    tenant
        .blocking(true, move |tenant| {
            let unavailable_integrity =
                |_| error(StatusCode::SERVICE_UNAVAILABLE, "Intégrité indisponible.");
            let integrity = tenant.store.integrity().map_err(unavailable)?;
            let ledger = tenant.store.audit_retention_ledger().map_err(unavailable)?;
            let mut value = serde_json::to_value(&integrity).map_err(unavailable_integrity)?;
            // `event_count` stays the chain length; the stored count and the
            // archived prefix make a retained store explicit.
            value["stored_events"] = json!(
                i64::try_from(integrity.event_count).unwrap_or(i64::MAX) - ledger.reclaimed_events
            );
            value["retention"] = serde_json::to_value(&ledger).map_err(unavailable_integrity)?;
            Ok(Json(value))
        })
        .await
}

async fn identity(ReadIdentity(tenant, role): ReadIdentity) -> ApiResult<Json<Value>> {
    Ok(Json(json!({"tenant_id":tenant.tenant_id,"role":role})))
}

async fn stream(
    ReadIdentity(tenant, _): ReadIdentity,
    headers: HeaderMap,
    Query(q): Query<EventQuery>,
) -> ApiResult<Response> {
    let permit = tenant
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| error(StatusCode::TOO_MANY_REQUESTS, "Trop de flux simultanés."))?;
    let header_cursor = headers
        .get("last-event-id")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok());
    let start = q.after.or(header_cursor);
    let mut cursor = tenant
        .blocking(true, move |tenant| {
            Ok(start
                .unwrap_or(tenant.store.head().map_err(unavailable)?)
                .max(0))
        })
        .await?;
    let events = async_stream::stream! {
        let _permit=permit;
        loop {
            let query=EventQuery{after:Some(cursor),limit:Some(200),order:Some("asc".into()),..Default::default()};
            match tenant.blocking(true, move |tenant| tenant.store.events(&query).map_err(unavailable)).await {
                Ok(batch)=>{
                    let full=batch.len()==200;
                    for event in batch {cursor=event.seq;yield Ok::<_,Infallible>(SseEvent::default().id(event.seq.to_string()).event("audit").json_data(&event).expect("serializable event"));}
                    if full {continue;}
                },
                Err(_)=>{yield Ok(SseEvent::default().event("error").data("audit unavailable"));break;}
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    Ok(Sse::new(events)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(10))
                .text("heartbeat"),
        )
        .into_response())
}

async fn export(
    ReadIdentity(tenant, _): ReadIdentity,
    Query(query): Query<EventQuery>,
) -> ApiResult<Response> {
    let json_format = match query.format.as_deref().unwrap_or("jsonl") {
        "json" => true,
        "jsonl" => false,
        _ => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "Format attendu : json ou jsonl.",
            ));
        }
    };
    let (head, ledger) = tenant
        .blocking(true, |tenant| {
            Ok((
                tenant.store.head().map_err(unavailable)?,
                tenant.store.audit_retention_ledger().map_err(unavailable)?,
            ))
        })
        .await?;
    let mut filter = query;
    filter.order = Some("asc".into());
    filter.limit = Some(200);
    filter.after = Some(filter.after.unwrap_or(0).max(0));
    filter.before = Some(filter.before.unwrap_or(head + 1).min(head + 1));
    // An export that reaches into the archived prefix starts with a typed
    // notice: the reclaimed events are in the offline archives, not omitted.
    let notice = (ledger.segments > 0
        && filter.after.unwrap_or(0) < ledger.archived_through_seq)
        .then(|| {
            json!({
                "type": "ai-guard.audit.retention-notice.v1",
                "archived_through_seq": ledger.archived_through_seq,
                "segments": ledger.segments,
                "reclaimed_events": ledger.reclaimed_events,
                "last_archive_id": ledger.last.as_ref().map(|last| last.archive_id.clone()),
                "last_manifest_sha256": ledger.last.as_ref().map(|last| last.manifest_sha256.clone()),
                "message": "Events up to archived_through_seq that are not listed here were reclaimed into offline archives; verify them with agentsandbox audit-retention verify."
            })
            .to_string()
        });
    let retention_header = (ledger.segments > 0).then(|| {
        format!(
            "segments={}; archived-through={}; reclaimed-events={}; last-manifest={}",
            ledger.segments,
            ledger.archived_through_seq,
            ledger.reclaimed_events,
            ledger
                .last
                .as_ref()
                .map_or("none", |last| last.manifest_sha256.as_str())
        )
    });
    let body = async_stream::stream! {
        let mut first=true;
        if json_format{yield Ok::<_,std::io::Error>("[".to_string());}
        if let Some(notice)=notice{first=false;yield Ok(if json_format{notice}else{format!("{notice}\n")});}
        loop {
            let page_filter = filter.clone();
            match tenant.blocking(true, move |tenant| tenant.store.events(&page_filter).map_err(unavailable)).await {
                Ok(items)=>{if items.is_empty(){break;}for item in items {filter.after=Some(item.seq);let prefix=if json_format{if first{""}else{","}}else{""};let suffix=if json_format{""}else{"\n"};first=false;yield Ok(format!("{prefix}{}{suffix}",serde_json::to_string(&item).expect("serializable event")));}},
                Err(_)=>{yield Err(std::io::Error::other("audit export interrupted"));return;}
            }
        }
        if json_format{yield Ok("]".to_string());}
    };
    let mut response = Response::new(Body::from_stream(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(if json_format {
            "application/json"
        } else {
            "application/x-ndjson"
        }),
    );
    if let Some(value) = retention_header.and_then(|value| HeaderValue::from_str(&value).ok()) {
        response.headers_mut().insert(
            header::HeaderName::from_static("ai-guard-audit-retention"),
            value,
        );
    }
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static(if json_format {
            "attachment; filename=agentsandbox-audit.json"
        } else {
            "attachment; filename=agentsandbox-audit.jsonl"
        }),
    );
    Ok(response)
}

#[derive(Clone)]
struct DashboardState {
    upstream: reqwest::Url,
    viewer_token: Option<String>,
    client: reqwest::Client,
    oidc_web_origin: Option<String>,
    owned_sources: bool,
}
pub fn dashboard_router(
    upstream: &str,
    viewer_token: String,
    front: PathBuf,
) -> anyhow::Result<Router> {
    dashboard_router_with_auth(upstream, Some(viewer_token), front)
}

/// Shared presentation accepts only viewer identities verified by the gateway.
/// The authenticated credential, rather than request data, chooses its tenant.
pub fn dashboard_shared_router(upstream: &str, front: PathBuf) -> anyhow::Result<Router> {
    dashboard_router_with_auth(upstream, None, front)
}

fn dashboard_router_with_auth(
    upstream: &str,
    viewer_token: Option<String>,
    front: PathBuf,
) -> anyhow::Result<Router> {
    dashboard_router_with_oidc(upstream, viewer_token, front, None)
}
/// Explicit auth proxy activation uses the same public origin as the gateway.
pub fn dashboard_router_with_oidc(
    upstream: &str,
    viewer_token: Option<String>,
    front: PathBuf,
    oidc_web_origin: Option<String>,
) -> anyhow::Result<Router> {
    dashboard_router_with_human_mode(upstream, viewer_token, front, oidc_web_origin, false)
}

/// Trusted presentation configuration. The gateway independently enforces
/// the same route closure; this option never grants identity or permissions.
pub fn dashboard_router_with_human_mode(
    upstream: &str,
    viewer_token: Option<String>,
    front: PathBuf,
    oidc_web_origin: Option<String>,
    owned_sources: bool,
) -> anyhow::Result<Router> {
    anyhow::ensure!(
        !owned_sources || viewer_token.is_none(),
        "owned human dashboard requires shared presentation without viewer fallback"
    );
    if let Some(origin) = &oidc_web_origin {
        let parsed = reqwest::Url::parse(origin)?;
        anyhow::ensure!(
            origin == &parsed.origin().ascii_serialization()
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && (parsed.scheme() == "https"
                    || parsed.scheme() == "http"
                        && parsed
                            .host_str()
                            .and_then(|host| host
                                .trim_matches(['[', ']'])
                                .parse::<std::net::IpAddr>()
                                .ok())
                            .is_some_and(|ip| ip.is_loopback())),
            "invalid dashboard OIDC origin"
        );
    }
    let upstream = reqwest::Url::parse(upstream)?;
    anyhow::ensure!(
        ["http", "https"].contains(&upstream.scheme())
            && upstream.username().is_empty()
            && upstream.password().is_none()
            && upstream.query().is_none()
            && upstream.fragment().is_none()
            && upstream.path() == "/",
        "invalid gateway upstream"
    );
    let s = DashboardState {
        upstream,
        viewer_token,
        oidc_web_origin,
        owned_sources,
        client: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()?,
    };
    Ok(Router::new()
        .route(
            "/healthz",
            get(|| async { Json(json!({"status":"ok","service":"dashboard"})) }),
        )
        .route("/api/v1/status", get(dashboard_build_status))
        .route(
            "/api/v1/gateway/status",
            get(dashboard_gateway_build_status),
        )
        .route("/api/{*path}", get(dashboard_proxy))
        .route("/auth/oidc/status", get(dashboard_oidc_proxy))
        .route("/auth/oidc/start", post(dashboard_oidc_proxy))
        .route("/auth/oidc/callback", get(dashboard_oidc_proxy))
        .route("/auth/oidc/tokens", post(dashboard_oidc_proxy))
        .route("/auth/oidc/refresh", post(dashboard_oidc_proxy))
        .route("/auth/oidc/logout", post(dashboard_oidc_proxy))
        .route_service(
            "/auth/oidc/complete",
            tower_http::services::ServeFile::new(front.join("index.html")),
        )
        .fallback_service(tower_http::services::ServeDir::new(front))
        .layer(middleware::from_fn_with_state(
            s.clone(),
            dashboard_security_headers,
        ))
        .with_state(s))
}
async fn dashboard_build_status() -> Json<crate::build_identity::BuildStatusV1> {
    Json(crate::build_identity::BuildStatusV1::current(
        crate::build_identity::Component::Dashboard,
    ))
}

/// This request has one configured destination and carries no caller identity.
/// The complete send/read/decode is bounded independently from human proxies.
async fn dashboard_gateway_build_status(
    State(state): State<DashboardState>,
) -> ApiResult<Json<crate::build_identity::BuildStatusV1>> {
    use crate::build_identity::{BuildStatusV1, Component, MAX_STATUS_BYTES};
    let mut destination = state.upstream.clone();
    destination.set_path("/api/v1/status");
    destination.set_query(None);
    destination.set_fragment(None);
    let fetched = tokio::time::timeout(Duration::from_secs(5), async {
        let mut response = state
            .client
            .get(destination)
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| ())?;
        if response.status() != StatusCode::OK
            || response
                .content_length()
                .is_some_and(|length| length > MAX_STATUS_BYTES as u64)
            || response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                != Some("application/json")
        {
            return Err(());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
            if chunk.len() > MAX_STATUS_BYTES.saturating_sub(bytes.len()) {
                return Err(());
            }
            bytes.extend_from_slice(&chunk);
        }
        BuildStatusV1::from_json(&bytes, Component::Gateway).map_err(|_| ())
    })
    .await;
    match fetched {
        Ok(Ok(status)) => Ok(Json(status)),
        _ => Err(error(
            StatusCode::BAD_GATEWAY,
            "Gateway build status is unavailable.",
        )),
    }
}

async fn dashboard_security_headers(
    State(state): State<DashboardState>,
    request: Request,
    next: Next,
) -> Response {
    let callback = state.oidc_web_origin.is_some()
        && request.method() == "GET"
        && request.uri().path() == "/auth/oidc/callback";
    // A cross-site IdP redirect keeps that metadata through the callback's 303.
    // Only the public completion document is admitted, never its token exchange.
    let completion_navigation = state.oidc_web_origin.is_some()
        && request.method() == "GET"
        && request.uri().path() == "/auth/oidc/complete"
        && request.uri().query().is_none()
        && [
            ("sec-fetch-mode", "navigate"),
            ("sec-fetch-dest", "document"),
        ]
        .into_iter()
        .all(|(name, expected)| {
            let mut values = request.headers().get_all(name).iter();
            values.next().is_some_and(|value| value == expected) && values.next().is_none()
        });
    let trusted = state.oidc_web_origin.as_ref().is_some_and(|origin| {
        oidc::browser_post(request.method(), request.uri().path())
            && oidc::same_origin(request.headers(), origin)
            || request.method() == "GET"
                && request.uri().path() == "/auth/oidc/status"
                && oidc::public_status_origin(request.headers(), origin)
    });
    security_headers_inner(request, next, trusted, callback || completion_navigation).await
}
async fn dashboard_oidc_proxy(
    State(state): State<DashboardState>,
    request: Request,
) -> ApiResult<Response> {
    if state.owned_sources {
        let allowed = request.method() == "GET"
            && state.oidc_web_origin.is_none()
            && ["/auth/oidc/status", "/auth/oidc/cli-profile"].contains(&request.uri().path());
        if !allowed {
            return Err(human_sources::unavailable());
        }
        let mut destination = state.upstream.clone();
        destination.set_path(request.uri().path());
        let response = state
            .client
            .get(destination)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|_| human_sources::unavailable())?;
        let status = response.status();
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| human_sources::unavailable())?;
            if chunk.len() > 16_384 - bytes.len() {
                return Err(human_sources::unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok((status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response());
    }
    let enabled = state.oidc_web_origin.as_ref().ok_or_else(|| {
        human::failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "oidc_not_configured",
            "The dashboard OIDC proxy is disabled.",
            "contact_operator",
        )
    })?;
    let path = request.uri().path();
    let callback = request.method() == "GET" && path == "/auth/oidc/callback";
    if oidc::browser_post(request.method(), path) && !oidc::same_origin(request.headers(), enabled)
    {
        return Err(human::failure(
            StatusCode::FORBIDDEN,
            "origin_denied",
            "The authentication origin is not approved.",
            "authenticate",
        ));
    }
    if request.uri().query().is_some() && !callback {
        return Err(oidc::invalid_request());
    }
    let mut destination = state.upstream.clone();
    destination.set_path(path);
    destination.set_query(request.uri().query());
    let mut call = state
        .client
        .request(request.method().clone(), destination)
        .timeout(Duration::from_secs(35));
    for name in [
        header::ORIGIN,
        header::CONTENT_TYPE,
        header::AUTHORIZATION,
        header::HeaderName::from_static("sec-fetch-site"),
        header::HeaderName::from_static("sec-fetch-mode"),
    ] {
        let mut values = request.headers().get_all(&name).iter();
        if let Some(value) = values.next() {
            if values.next().is_some()
                || value.as_bytes().len()
                    > if name == header::AUTHORIZATION {
                        16_391
                    } else {
                        2048
                    }
            {
                return Err(oidc::invalid_request());
            }
            call = call.header(name, value);
        }
    }
    if let Some(cookie) = oidc::correlation_cookie(request.headers())? {
        call = call.header(header::COOKIE, format!("aiguard_oidc={cookie}"));
    }
    let body = tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(request.into_body(), 35_000),
    )
    .await
    .map_err(|_| oidc::invalid_request())?
    .map_err(|_| oidc::invalid_request())?;
    let upstream = call.body(body).send().await.map_err(|_| {
        human::failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "identity_unavailable",
            "The authentication gateway is unavailable.",
            "authenticate",
        )
    })?;
    let status = upstream.status();
    let headers = upstream.headers().clone();
    if !(status.is_client_error()
        || status.is_server_error()
        || status == StatusCode::OK
        || status == StatusCode::SEE_OTHER)
    {
        return Err(oidc::invalid_request());
    }
    let mut chunks = upstream.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| oidc::invalid_request())?;
        if bytes.len().saturating_add(chunk.len()) > 65_536 {
            return Err(oidc::invalid_request());
        }
        bytes.extend_from_slice(&chunk);
    }
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    if status == StatusCode::SEE_OTHER {
        if !callback
            || headers
                .get(header::LOCATION)
                .is_none_or(|v| v != "/auth/oidc/complete")
        {
            return Err(oidc::invalid_request());
        }
        response.headers_mut().insert(
            header::LOCATION,
            HeaderValue::from_static("/auth/oidc/complete"),
        );
    } else {
        if headers
            .get(header::CONTENT_TYPE)
            .is_none_or(|v| v != "application/json")
        {
            return Err(oidc::invalid_request());
        }
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    let mut cookies = headers.get_all(header::SET_COOKIE).iter();
    if let Some(cookie) = cookies.next() {
        if cookies.next().is_some()
            || !oidc::valid_response_cookie(cookie, enabled.starts_with("https://"))
        {
            return Err(oidc::invalid_request());
        }
        response
            .headers_mut()
            .insert(header::SET_COOKIE, cookie.clone());
    }
    Ok(response)
}
async fn dashboard_proxy(State(s): State<DashboardState>, request: Request) -> ApiResult<Response> {
    if s.owned_sources
        && (request.method() != "GET"
            || ![
                "/api/v1/human/identity",
                "/api/v1/human/resources",
                "/api/v1/human/events",
                "/api/v1/human/events/recent",
                "/api/v1/human/owned/brokers",
            ]
            .contains(&request.uri().path()))
    {
        return Err(human_sources::unavailable());
    }
    let canonical_path = request
        .uri()
        .path()
        .strip_prefix("/api/v1/")
        .map(|suffix| format!("/api/{suffix}"))
        .unwrap_or_else(|| request.uri().path().to_owned());
    let human_path =
        request.uri().path().starts_with("/api/v1/human/") && human_dashboard_path(&canonical_path);
    let federation_path = request.uri().path() == "/api/v1/federation/workers";
    let token = (if human_path {
        oidc::human_bearer(request.headers())
    } else {
        bearer(request.headers())
    })
    .ok_or_else(|| {
        if human_path {
            human::failure(
                StatusCode::UNAUTHORIZED,
                "authentication_required",
                "A human bearer proof is required.",
                "authenticate",
            )
        } else {
            error(StatusCode::UNAUTHORIZED, "Jeton de lecture requis.")
        }
    })?
    .to_owned();
    if !human_path
        && s.viewer_token
            .as_ref()
            .is_some_and(|expected| !token_matches(&token, expected))
    {
        return Err(error(StatusCode::UNAUTHORIZED, "Jeton invalide."));
    }
    if ![
        "/api/events",
        "/api/overview",
        "/api/agents",
        "/api/sandboxes",
        "/api/stream",
        "/api/export",
        "/api/integrity",
        "/api/identity",
        "/api/workers",
        "/api/quota",
        "/api/usage",
    ]
    .contains(&canonical_path.as_str())
        && !federation_path
        && !human_path
        && !canonical_path
            .strip_prefix("/api/workers/")
            .is_some_and(|id| {
                uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id)
            })
    {
        return Err(error(StatusCode::NOT_FOUND, "Route de lecture inconnue."));
    }
    if s.owned_sources {
        // No viewer fallback or legacy verification request precedes the
        // gateway's own modern authentication and context checks.
    } else if human_path {
        if canonical_path != "/api/human/identity" {
            verify_dashboard_human(&s, &token).await?;
        }
    } else {
        verify_dashboard_viewer(&s, &token).await?;
    }
    let mut url = s.upstream.clone();
    url.set_path(request.uri().path());
    url.set_query(request.uri().query());
    let mut call = s
        .client
        .request(request.method().clone(), url)
        .bearer_auth(&token);
    if human_path {
        call = call.timeout(Duration::from_secs(
            if canonical_path == "/api/human/stream" {
                305
            } else {
                15
            },
        ));
        let mut contexts = request.headers().get_all("x-aiguard-context").iter();
        if let Some(context) = contexts.next() {
            if contexts.next().is_some() || context.as_bytes().len() > 4096 {
                return Err(human::failure(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "The context header is ambiguous or exceeds its bound.",
                    "select_context",
                ));
            }
            call = call.header("x-aiguard-context", context);
        }
    }
    if let Some(cursor) = request.headers().get("last-event-id") {
        call = call.header("last-event-id", cursor);
    }
    let upstream = call.send().await.map_err(|_| {
        if human_path {
            human::failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_unavailable",
                "The human gateway is unavailable.",
                "retry",
            )
        } else {
            error(StatusCode::BAD_GATEWAY, "Collecteur indisponible.")
        }
    })?;
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let mut response = Response::new(Body::from_stream(
        upstream
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other)),
    ));
    *response.status_mut() = status;
    for key in [
        header::CONTENT_TYPE,
        header::CONTENT_DISPOSITION,
        header::CACHE_CONTROL,
    ] {
        if let Some(v) = headers.get(&key) {
            response.headers_mut().insert(key, v.clone());
        }
    }
    Ok(response)
}

fn human_dashboard_path(path: &str) -> bool {
    [
        "/api/human/identity",
        "/api/human/brokers",
        "/api/human/owned/brokers",
        "/api/human/workers",
        "/api/human/federation/workers",
        "/api/human/resources",
        "/api/human/events",
        "/api/human/events/recent",
        "/api/human/export",
        "/api/human/stream",
    ]
    .contains(&path)
        || path.strip_prefix("/api/human/events/").is_some_and(|id| {
            uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id)
        })
}

async fn verify_dashboard_human(state: &DashboardState, token: &str) -> ApiResult<()> {
    let mut url = state.upstream.clone();
    url.set_path("/api/v1/human/identity");
    let mut response = state
        .client
        .get(url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|_| {
            human::failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_unavailable",
                "Human identity verification is unavailable.",
                "retry",
            )
        })?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(human::failure(
            StatusCode::UNAUTHORIZED,
            "credential_invalid",
            "The human credential is invalid, revoked or expired.",
            "authenticate",
        ));
    }
    if !response.status().is_success() {
        return Err(dashboard_human_preflight_failure(response).await);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        human::failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "identity_unavailable",
            "Human identity verification is incomplete.",
            "retry",
        )
    })? {
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|size| size > crate::identity::HUMAN_MAX_PAGE_BYTES)
        {
            return Err(human::failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_unavailable",
                "The human identity response exceeds its bound.",
                "contact_operator",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let identity: crate::identity::HumanIdentityV1 =
        serde_json::from_slice(&bytes).map_err(|_| {
            human::failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_unavailable",
                "The human identity response is invalid.",
                "contact_operator",
            )
        })?;
    if identity.version != 1
        || !matches!(
            (&identity.authentication.method[..], &identity.principal),
            (
                "local_credential",
                crate::identity::PrincipalRefV1::HumanLocal { .. }
            ) | (
                "oidc_access_token",
                crate::identity::PrincipalRefV1::HumanOidc { .. }
            )
        )
        || identity.authentication.method == "oidc_access_token"
            && (uuid::Uuid::parse_str(&identity.authentication.credential_id).is_err()
                || identity
                    .authentication
                    .expires_at
                    .as_ref()
                    .is_none_or(|expiry| chrono::DateTime::parse_from_rfc3339(expiry).is_err()))
    {
        return Err(human::failure(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "A supported authenticated human identity is required.",
            "review_permissions",
        ));
    }
    Ok(())
}

async fn dashboard_human_preflight_failure(
    mut response: reqwest::Response,
) -> (StatusCode, Json<Value>) {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Envelope {
        error: Rejection,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Rejection {
        code: String,
        #[serde(rename = "message")]
        _message: String,
        retry_action: String,
    }
    let unavailable = || {
        human::failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "identity_unavailable",
            "Human identity verification was refused.",
            "retry",
        )
    };
    let status = response.status();
    let mut bytes = Vec::new();
    loop {
        // The original request's three-second total timeout also covers chunks.
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => return unavailable(),
        };
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|size| size > 4096)
        {
            return unavailable();
        }
        bytes.extend_from_slice(&chunk);
    }
    let Ok(envelope) = serde_json::from_slice::<Envelope>(&bytes) else {
        return unavailable();
    };
    if status == StatusCode::TOO_MANY_REQUESTS
        && envelope.error.code == "audit_read_budget_exhausted"
        && envelope.error.retry_action == "contact_operator"
    {
        // Both exhausted and disabled human-read budgets use this closed tuple.
        // Neither the upstream message nor any upstream header is forwarded.
        return human::failure(
            StatusCode::TOO_MANY_REQUESTS,
            "audit_read_budget_exhausted",
            "The human read audit budget does not permit this read. Preserve the audit and contact the operator.",
            "contact_operator",
        );
    }
    unavailable()
}

async fn verify_dashboard_viewer(s: &DashboardState, token: &str) -> ApiResult<()> {
    let mut url = s.upstream.clone();
    url.set_path("/api/v1/identity");
    let mut response = s
        .client
        .get(url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|_| {
            error(
                StatusCode::BAD_GATEWAY,
                "Vérification d'identité indisponible.",
            )
        })?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "Identité de lecture invalide.",
        ));
    }
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        return Err(store_io::capacity());
    }
    if !response.status().is_success() {
        return Err(error(
            StatusCode::BAD_GATEWAY,
            "Vérification d'identité refusée.",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| error(StatusCode::BAD_GATEWAY, "Réponse d'identité incomplète."))?
    {
        if bytes.len() + chunk.len() > 4096 {
            return Err(error(
                StatusCode::BAD_GATEWAY,
                "Réponse d'identité trop volumineuse.",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let identity: Value = serde_json::from_slice(&bytes)
        .map_err(|_| error(StatusCode::BAD_GATEWAY, "Réponse d'identité invalide."))?;
    if identity["role"] != "viewer" {
        return Err(error(
            StatusCode::FORBIDDEN,
            "Le dashboard accepte uniquement le rôle viewer.",
        ));
    }
    if !identity["tenant_id"]
        .as_str()
        .is_some_and(|id| validate_tenant_id(id).is_ok())
    {
        return Err(error(
            StatusCode::BAD_GATEWAY,
            "Périmètre d'identité invalide.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod ingestion_window_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tower::ServiceExt;

    fn request(path: &str, body: Value) -> Request {
        Request::builder()
            .method("POST")
            .uri(path)
            .header("authorization", "Bearer synthetic-runner")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    // The RED used this same real route/SQLite schedule and received429 on
    // /decide. Only the clock/count are seeded, independent of host throughput.
    #[tokio::test]
    async fn exhausted_ingestion_does_not_spend_the_control_decision_window() {
        for ingest_path in ["/api/events", "/api/v1/events"] {
            let directory = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let store = Arc::new(
                Store::open(
                    &directory.path().join("audit.sqlite"),
                    Redactor::new(vec![]),
                    1000,
                )
                .unwrap(),
            );
            let state = GatewayState::for_tenants(
                vec![TenantGateway {
                    tenant_id: "local".into(),
                    store: store.clone(),
                    runner_token: "synthetic-runner".into(),
                    viewer_token: "synthetic-viewer".into(),
                }],
                Policy::default(),
                None,
                Redactor::new(vec![]),
            )
            .unwrap();
            // Hold a single window without relying on machine throughput. The
            // future Instant is confined to this unit test and never persisted.
            *state.tenants[0].ingestion_rate.lock().unwrap() =
                (Instant::now() + Duration::from_secs(60), 199);
            let router = gateway_router(state);
            let mut event = Event::new("command.started");
            // Worker-controlled fields impersonate a trusted producer. Their
            // route still selects ingestion and cannot spend control capacity.
            event.source = "runner".into();
            let identity = event.id.clone();
            let ingested = router
                .clone()
                .oneshot(request(ingest_path, json!(event)))
                .await
                .unwrap();
            assert_eq!(ingested.status(), StatusCode::OK);
            assert!(
                store
                    .events(&EventQuery::default())
                    .unwrap()
                    .iter()
                    .any(|row| row.event.id == identity)
            );
            let decision_id = uuid::Uuid::new_v4();
            let refused = router
                .clone()
                .oneshot(request(ingest_path, json!(Event::new("command.started"))))
                .await
                .unwrap();
            assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(refused.headers()[header::RETRY_AFTER], "1");
            let body = axum::body::to_bytes(refused.into_body(), 4096)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
                "write_rate_limited"
            );
            let control = router
                .clone()
                .oneshot(request(
                    "/api/v1/control/events",
                    json!(Event::new("command.started")),
                ))
                .await
                .unwrap();
            assert_eq!(control.status(), StatusCode::OK);
            let decision = router
                .oneshot(request(
                    "/api/v1/decide",
                    json!({"request_id":decision_id,"profile":"strict","command":{"argv":["id"],"cwd":"/workspace"}}),
                ))
                .await
                .unwrap();
            assert_eq!(
                decision.status(),
                StatusCode::OK,
                "worker-driven ingestion refused the independent control route"
            );
            assert!(
                store
                    .events(&EventQuery::default())
                    .unwrap()
                    .iter()
                    .any(|row| row.event.kind == "security.decision"
                        && row.event.data["request_id"] == decision_id.to_string())
            );
        }
    }

    #[tokio::test]
    async fn control_route_is_runner_only_closed_and_tenant_scoped() {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let a = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("a.sqlite"),
                "a",
                Redactor::new(vec![]),
                1000,
            )
            .unwrap(),
        );
        let b = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("b.sqlite"),
                "b",
                Redactor::new(vec![]),
                1000,
            )
            .unwrap(),
        );
        let state = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "a".into(),
                    store: a.clone(),
                    runner_token: "synthetic-runner".into(),
                    viewer_token: "synthetic-viewer".into(),
                },
                TenantGateway {
                    tenant_id: "b".into(),
                    store: b.clone(),
                    runner_token: "other-runner".into(),
                    viewer_token: "other-viewer".into(),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        let router = gateway_router(state);
        for (token, status) in [
            ("synthetic-viewer", StatusCode::FORBIDDEN),
            ("unrecognized-worker-token", StatusCode::UNAUTHORIZED),
        ] {
            let polled = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let observed = polled.clone();
            let request = Request::builder()
                .method("POST")
                .uri("/api/v1/control/events")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from_stream(futures_util::stream::once(async move {
                    observed.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok::<_, std::io::Error>("invalid JSON body")
                })))
                .unwrap();
            assert_eq!(
                router.clone().oneshot(request).await.unwrap().status(),
                status
            );
            assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
        }
        for kind in [
            "file.created",
            "control.fake",
            "worker.fake",
            "security.decision",
            "usage.allocation",
            "command.started.unapproved",
        ] {
            let response = router
                .clone()
                .oneshot(request("/api/v1/control/events", json!(Event::new(kind))))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{kind}");
        }
        let mut event = Event::new("command.started");
        // Session metadata cannot select tenant B's database.
        event.session_id = Some("tenant-b-session".into());
        let identity = event.id.clone();
        let response = router
            .clone()
            .oneshot(request("/api/v1/control/events", json!(event)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            a.events(&EventQuery::default())
                .unwrap()
                .iter()
                .any(|row| row.event.id == identity)
        );
        assert!(
            !b.events(&EventQuery::default())
                .unwrap()
                .iter()
                .any(|row| row.event.id == identity)
        );
        let response = router
            .oneshot(request(
                "/api/control/events",
                json!(Event::new("command.started")),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn retry_hint_never_relabels_quota_or_durable_audit_refusals() {
        for body in [
            json!({"refusal":"quota"}),
            json!({"error":{"code":"audit_write_budget_exhausted","retry_action":"contact_operator"}}),
            json!({"error":"x".repeat(8192)}),
        ] {
            let expected = serde_json::to_vec(&body).unwrap();
            let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
            response
                .headers_mut()
                .insert("x-test-preserved", HeaderValue::from_static("yes"));
            let response = write_rate_retry_header(response).await;
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(response.headers()["x-test-preserved"], "yes");
            assert!(response.headers().get(header::RETRY_AFTER).is_none());
            assert_eq!(
                axum::body::to_bytes(response.into_body(), 16384)
                    .await
                    .unwrap()
                    .as_ref(),
                expected
            );
        }
    }
}
