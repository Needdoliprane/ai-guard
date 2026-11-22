//! Candidate A0 refusals over real owned sources. NOT_RUN until separate QA.
//! These tests do not participate in the frozen two-parent a47 baseline.
use super::*;
use crate::{
    control::{
        AdmissionV1, ControlProtocolV1, DecisionPhaseV1, DecisionReceiptV1, LaunchSpecV1,
        NetworkV1, QuotaPolicyV1, RestartPolicyV1, RuntimeKindV1, SecurityPresetV1,
        SupervisedAdmissionV1, WorkerStateV1, WorkerV1, review_binding::*,
    },
    human_authority::startup::HumanOwnedStartupBundle,
    model::{CommandSpec, DecisionRequest, Outcome},
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::http::{HeaderMap, Method, Request as HttpRequest, header};
use std::{io::Write, path::PathBuf, sync::mpsc, thread};
use tower::ServiceExt;

const OBSERVE: Duration = Duration::from_secs(120);
const HUMAN: &str = "synthetic-A0-local-human";
const WORKER: &str = "synthetic-A0-workload-worker-token-00000000";
const B_RUNNER: &str = "synthetic-B-runner";

fn gateway(
    bundle: HumanOwnedStartupBundle,
    credentials: crate::control_fs::PrivateFile,
    tokens: Vec<(String, String, String)>,
) -> GatewayState {
    GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap()
}

fn tenant(state: &GatewayState, id: &str) -> usize {
    state
        .tenants
        .iter()
        .position(|entry| entry.tenant_id == id)
        .unwrap()
}

pub(in crate::api::human_sources) fn command(
    bundle: &HumanOwnedStartupBundle,
    id: &str,
) -> ProvisionRunnerReviewBindingV1 {
    let descriptor = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some(id))
        .unwrap();
    let mut command = crate::control::review_binding::tests::provision();
    command.binding.tenant_id = id.into();
    command.policy.tenant_id = id.into();
    command.binding.credential_id = uuid::Uuid::from_u128(match id {
        "A" => 100,
        "B" => 102,
        _ => unreachable!(),
    })
    .to_string();
    command.binding.control_namespace_uuid = descriptor.durable_namespace_id.to_string();
    command.policy.control_namespace_uuid = command.binding.control_namespace_uuid.clone();
    command.binding.closure_id = bundle.owner.closure().closure_id.to_string();
    command.binding.closure_revision = bundle.owner.closure().closure_revision;
    command.binding.closure_digest = bundle.owner.closure().closure_digest.clone();
    command.binding.control_config_public_digest = bundle
        .owner
        .closure()
        .configuration
        .public_config_digest
        .clone();
    command.policy.shares.clear();
    command.policy.owner_permissions = vec![ReviewSessionPermissionV1::Content];
    command.binding.session_binding_policy_digest = command.policy.digest().unwrap();
    command
}

fn identity_change(bundle: &HumanOwnedStartupBundle, id: &str, change: IdentityChangeV1) {
    let source_id = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some(id))
        .unwrap()
        .source_id;
    let source = bundle.owner.source_token(source_id).unwrap();
    let epoch = bundle.owner.try_write_epoch().unwrap();
    bundle
        .stores
        .get(&source_id)
        .unwrap()
        .mutate_identity_scoped(
            &epoch,
            &source,
            &IdentityMutationV1 {
                version: 1,
                operation_id: uuid::Uuid::new_v4().to_string(),
                actor: PrincipalRefV1::LegacyControl {
                    credential_id: "synthetic-offline-control".into(),
                    role: LegacyControlRoleV1::Runner,
                },
                change,
            },
        )
        .unwrap();
}

pub(in crate::api::human_sources) fn provision_b(bundle: &HumanOwnedStartupBundle) {
    let command = command(bundle, "B");
    identity_change(
        bundle,
        "B",
        IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: command.policy.owner.clone(),
                display_name: "Synthetic A0 B owner".into(),
                revision: 1,
                active: true,
            },
        },
    );
    identity_change(
        bundle,
        "B",
        IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: MembershipV1 {
                version: 1,
                membership_id: uuid::Uuid::new_v4().to_string(),
                principal: command.policy.owner.clone(),
                tenant_id: "B".into(),
                organization_scope: command.policy.organization_scope.clone(),
                revision: 1,
                active: true,
                capabilities: [HumanCapabilityV1::ReadContent].into_iter().collect(),
            },
        },
    );
    let source_id = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("B"))
        .unwrap()
        .source_id;
    let source = bundle.owner.source_token(source_id).unwrap();
    let epoch = bundle.owner.try_write_epoch().unwrap();
    let result = bundle
        .stores
        .get(&source_id)
        .unwrap()
        .provision_review_binding(&epoch, &source, &command)
        .unwrap();
    assert!(result.snapshot.binding.active);
    assert_eq!(
        result.snapshot.binding.credential_id,
        uuid::Uuid::from_u128(102).to_string()
    );
}

pub(in crate::api::human_sources) fn local_credential(
    bundle: &HumanOwnedStartupBundle,
    id: &str,
    token: &str,
    active: bool,
) {
    let principal = command(bundle, id).policy.owner;
    identity_change(
        bundle,
        id,
        IdentityChangeV1::Credential {
            expected_revision: 0,
            binding: LocalCredentialV1 {
                version: 1,
                credential_id: uuid::Uuid::new_v4().to_string(),
                principal,
                sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                revision: 1,
                active,
                expires_at: None,
            },
        },
    );
}

async fn response(
    router: &Router,
    method: &str,
    path: &str,
    tokens: &[&str],
    pending_body: bool,
) -> (StatusCode, HeaderMap, Value) {
    let mut request = HttpRequest::builder().method(method).uri(path);
    for token in tokens {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = if pending_body {
        Body::from_stream(futures_util::stream::pending::<
            Result<axum::body::Bytes, std::io::Error>,
        >())
    } else {
        Body::empty()
    };
    let response =
        tokio::time::timeout(OBSERVE, router.clone().oneshot(request.body(body).unwrap()))
            .await
            .expect("HTTP observation budget elapsed, not a product refusal")
            .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 512 * 1024)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    for token in tokens {
        assert!(
            !String::from_utf8_lossy(&bytes).contains(token),
            "response must not expose a credential"
        );
    }
    (status, headers, body)
}

fn no_protocol(body: &Value) {
    for key in [
        "schema_version",
        "control_namespace_uuid",
        "tenant_id",
        "role",
        "protocols",
        "active",
    ] {
        assert!(
            body.get(key).is_none(),
            "refusal published protocol field {key}"
        );
    }
}

async fn require_protocol(router: &Router, bearer: &str, id: &str) -> ControlProtocolV1 {
    let (status, headers, body) =
        response(router, "GET", RESERVED_PROTOCOL, &[bearer], false).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "healthy real fixture diagnostic must succeed: {body}"
    );
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    let protocol: ControlProtocolV1 = serde_json::from_value(body).unwrap();
    protocol.validate().unwrap();
    assert_eq!(protocol.tenant_id, id);
    assert_eq!(protocol.role, "runner");
    assert!(
        !protocol.active,
        "a diagnostic must not activate the existing inactive lease"
    );
    protocol
}

async fn drain(state: &GatewayState) {
    let retirement = state.human_retirement().unwrap();
    retirement.retire();
    tokio::time::timeout(OBSERVE, retirement.drain())
        .await
        .unwrap()
        .unwrap();
}

async fn drain_invalid_owner(state: &GatewayState) {
    // Permanent retained-file/source mismatch latches the real owner. Closing
    // releases accepted work but must not rename that invalid state to success.
    assert!(
        !state
            .owned_human
            .as_ref()
            .unwrap()
            .owner
            .admission_available()
    );
    let retirement = state.human_retirement().unwrap();
    retirement.retire();
    let error = tokio::time::timeout(OBSERVE, retirement.drain())
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<crate::human_authority::HumanAuthorityFailure>(),
        Some(&crate::human_authority::HumanAuthorityFailure::OwnerInvalid)
    );
}

// Same real decision/admission sequence as tests/support/relay_admission_fixture.rs.
// It reserves durable quota/IDs only; no runtime or Running relay is asserted.
pub(in crate::api::human_sources) fn reserve_workload_fixture_worker(store: &Store) -> WorkerV1 {
    let session_id = uuid::Uuid::new_v4().to_string();
    let request = DecisionRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        profile: "strict".into(),
        command: CommandSpec {
            argv: vec!["/usr/bin/id".into()],
            cwd: "/workspace".into(),
        },
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        session_id: Some(session_id.clone()),
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let policy = Policy::default();
    let decision = crate::policy::evaluate(&policy, &request);
    assert_eq!(decision.outcome, Outcome::Allow);
    let stored = store
        .decision(&request, Some((decision, json!(null))))
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.event.decision.as_ref().unwrap().outcome,
        Outcome::Allow
    );
    let admission = SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: "ab".repeat(32),
        admission: AdmissionV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            receipt: DecisionReceiptV1 {
                version: 1,
                request_id: request.request_id.clone(),
                request_hash: crate::policy::request_hash(&request),
                phase: DecisionPhaseV1::Initial,
                audit_seq: stored.seq,
                audit_hash: stored.hash.clone(),
            },
            launch: LaunchSpecV1 {
                version: 1,
                session_id,
                command: request.command.clone(),
                profile: request.profile.clone(),
                runtime_kind: RuntimeKindV1::Docker,
                authority_id: uuid::Uuid::new_v4().to_string(),
                image_ref: "worker:fixture".into(),
                image_id: format!("sha256:{}", "aa".repeat(32)),
                network: NetworkV1::None,
                cpu_millis: 2000,
                memory_bytes: 1_073_741_824,
                pids_limit: 256,
                timeout_seconds: 600,
                security_preset: SecurityPresetV1::FixedSandboxV2,
                restart_policy: RestartPolicyV1::No,
                workspace_manifest_digest: "bb".repeat(32),
                workspace_import_policy_version: 1,
                provider: "generic".into(),
                capture_output: false,
                credential_binding_id: None,
                credential_names: vec![],
                provider_resolution: None,
            },
            decision_request: request,
        },
    };
    let result = store
        .admit_supervised(
            &admission,
            &policy,
            crate::policy::evaluate(&policy, &admission.admission.decision_request),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap();
    assert!(result.accepted);
    assert!(!result.replayed);
    assert!(result.refusal.is_none());
    assert!(result.receipt.audit_seq > stored.seq);
    let worker = result
        .worker
        .expect("actual admission must reserve a worker");
    let persisted = store.worker(&worker.worker_id).unwrap().unwrap();
    assert_eq!(persisted.worker_id, worker.worker_id);
    assert_eq!(persisted.attempt.attempt_id, worker.attempt.attempt_id);
    assert!(persisted.quota_held);
    assert_eq!(persisted.state, WorkerStateV1::Reserved);
    assert!(persisted.attempt.container_id.is_none());
    persisted
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_nonrunner_absent_and_duplicate_headers_refuse_before_store() {
    let mut reserved = None;
    let mut fixture = Store::owned_control_fixture_with_legacy_setup_for_test(14, |legacy| {
        reserved = Some(reserve_workload_fixture_worker(legacy));
    });
    let worker = reserved.expect("real pre-migration worker reservation must complete");
    assert!(worker.quota_held);
    assert_eq!(worker.state, WorkerStateV1::Reserved);
    assert!((32..=512).contains(&WORKER.len()));
    assert!(!WORKER.chars().any(char::is_whitespace));
    let (bundle, credentials, tokens) = fixture.take_startup();
    local_credential(&bundle, "A", HUMAN, true);
    {
        let source_id = bundle
            .owner
            .closure()
            .sources
            .iter()
            .find(|source| source.tenant_id.as_deref() == Some("A"))
            .unwrap()
            .source_id;
        let source = bundle.owner.source_token(source_id).unwrap();
        let epoch = bundle.owner.try_write_epoch().unwrap();
        let access = HumanSourceAccess::write(&epoch, &source).unwrap();
        let store = bundle.stores.get(&source_id).unwrap();
        let credential_id = uuid::Uuid::new_v4().to_string();
        let subject = WorkloadSubjectV1::Worker {
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            generation: 1,
        };
        let result = store
            .issue_workload_binding_scoped(
                &access,
                &WorkloadBindingIssueV1 {
                    version: 1,
                    operation_id: uuid::Uuid::new_v4().to_string(),
                    credential_id: credential_id.clone(),
                    expected_revision: 0,
                    subject: subject.clone(),
                    sha256_digest: hex::encode(Sha256::digest(WORKER.as_bytes())),
                    expires_at: (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
                },
            )
            .unwrap();
        assert_eq!(result.revision, 1);
        assert_eq!(result.credential_id, credential_id);
        assert!(!result.replayed);
        assert!(result.audit_seq > 0);
        epoch.check().unwrap();
        drop(access);
        drop(epoch);
        let epoch = bundle.owner.try_read_epoch().unwrap();
        let access = HumanSourceAccess::read(&epoch, &source).unwrap();
        let digest: [u8; 32] = Sha256::digest(WORKER.as_bytes()).into();
        let owner = store
            .workload_digest_binding_scoped(&access, &digest)
            .unwrap()
            .unwrap();
        assert_eq!(owner.credential_id, credential_id);
        assert_eq!(owner.subject, subject);
        epoch.check().unwrap();
    }
    // Actual issuance and checked historical ownership, not a Running relay
    // authentication: the owned lease remains inactive and no runtime ran.
    println!("A0_WORKER_FIXTURE_RESERVED_BOUND_NOT_RELAY_RUNNING");
    let state = gateway(bundle, credentials, tokens);
    let router = gateway_router(state.clone());
    require_protocol(&router, RUNNER, "A").await;
    let observations = state
        .tenants
        .iter()
        .map(|entry| crate::audit::read_test_observer::observe(entry.store.sqlite().unwrap()))
        .collect::<Vec<_>>();
    for headers in [
        vec![],
        vec!["synthetic-unknown"],
        vec!["synthetic-A-viewer"],
        vec![HUMAN],
        vec![WORKER],
        vec![RUNNER, RUNNER],
    ] {
        let (status, _, body) = response(&router, "GET", RESERVED_PROTOCOL, &headers, false).await;
        assert!(
            matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
            "identity/role refusal, not a broken-source503: {status} {body}"
        );
        no_protocol(&body);
        assert!(
            observations
                .iter()
                .all(|observed| observed.try_recv().is_err())
        );
    }
    // Only the presented credential changed; the same genuine authority still works.
    require_protocol(&router, RUNNER, "A").await;
    drain(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_historical_human_collision_refuses_before_either_store() {
    for (source_tenant, active) in [("A", true), ("B", false)] {
        let mut fixture = Store::owned_control_fixture_for_test(14);
        let (bundle, credentials, tokens) = fixture.take_startup();
        provision_b(&bundle);
        local_credential(&bundle, source_tenant, RUNNER, active);
        let state = gateway(bundle, credentials.clone(), tokens);
        let owned = state.owned_human.as_ref().unwrap();
        let digest: [u8; 32] = Sha256::digest(RUNNER.as_bytes()).into();
        assert!(
            owned.candidates.local_source(&digest).is_err(),
            "real startup index must classify the actual historical collision"
        );
        let credential = crate::api::authenticate_review_runner_fixture(
            credentials,
            owned.owner.clone(),
            state
                .tenants
                .iter()
                .map(|entry| {
                    (
                        entry.tenant_id.clone(),
                        Arc::clone(entry.store.sqlite().unwrap()),
                    )
                })
                .collect(),
            RUNNER,
        )
        .unwrap();
        let a = tenant(&state, "A");
        {
            let epoch = owned.owner.try_read_epoch().unwrap();
            state.tenants[a]
                .store
                .sqlite()
                .unwrap()
                .assert_owned_control_fixture_binding_for_test(
                    &epoch,
                    &owned.sources[a],
                    &credential,
                    14,
                );
        }
        drop(credential);
        let router = gateway_router(state.clone());
        require_protocol(&router, B_RUNNER, "B").await;
        let observations = state
            .tenants
            .iter()
            .map(|entry| crate::audit::read_test_observer::observe(entry.store.sqlite().unwrap()))
            .collect::<Vec<_>>();
        let (status, _, body) = response(&router, "GET", RESERVED_PROTOCOL, &[RUNNER], false).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        no_protocol(&body);
        assert!(
            observations
                .iter()
                .all(|observed| observed.try_recv().is_err())
        );
        require_protocol(&router, B_RUNNER, "B").await;
        drain(&state).await;
    }
}

#[test]
fn owned_control_missing_retained_source_refuses_startup() {
    let mut fixture = Store::owned_control_fixture_for_test(14);
    let (mut bundle, credentials, tokens) = fixture.take_startup();
    let id = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("A"))
        .unwrap()
        .source_id;
    let epoch = bundle.owner.try_read_epoch().unwrap();
    epoch
        .validate_source(&bundle.owner.source_token(id).unwrap())
        .unwrap();
    drop(epoch);
    let removed = bundle.stores.remove(&id).unwrap();
    let result = GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    );
    assert!(
        matches!(result, Err(ref error) if error.to_string().contains("retained human store absent"))
    );
    drop(removed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_revoked_review_binding_cannot_publish_a_protocol() {
    let mut fixture = Store::owned_control_fixture_for_test(14);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let original = command(&bundle, "A");
    let state = gateway(bundle, credentials, tokens);
    let router = gateway_router(state.clone());
    require_protocol(&router, RUNNER, "A").await;
    let owned = state.owned_human.as_ref().unwrap();
    let a = tenant(&state, "A");
    let b = tenant(&state, "B");
    {
        let epoch = owned.owner.try_write_epoch().unwrap();
        let result = state.tenants[a]
            .store
            .sqlite()
            .unwrap()
            .revoke_review_binding(
                &epoch,
                &owned.sources[a],
                &RevokeRunnerReviewBindingV1 {
                    version: 1,
                    operation_id: uuid::Uuid::new_v4().to_string(),
                    operator_delegation_id: original.operator_delegation_id.clone(),
                    binding_id: original.binding.binding_id.clone(),
                    expected_revision: 1,
                    expected_binding_digest: original.binding.digest().unwrap(),
                    reason: ReviewBindingRevocationReasonV1::OperatorRevoked,
                },
            )
            .unwrap();
        assert!(!result.snapshot.binding.active);
        assert_eq!(result.snapshot.binding.revision, 2);
    }
    let observed_b =
        crate::audit::read_test_observer::observe(state.tenants[b].store.sqlite().unwrap());
    for _ in 0..2 {
        let (status, _, body) = response(&router, "GET", RESERVED_PROTOCOL, &[RUNNER], false).await;
        assert!(
            matches!(
                status,
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::SERVICE_UNAVAILABLE
            ),
            "revoked authority was not refused: {status} {body}"
        );
        no_protocol(&body);
    }
    assert!(observed_b.try_recv().is_err());
    drain(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_changed_retained_registry_refuses_before_sql() {
    let mut fixture = Store::owned_control_fixture_for_test(14);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let path = credentials.path().to_owned();
    let state = gateway(bundle, credentials, tokens);
    let router = gateway_router(state.clone());
    require_protocol(&router, RUNNER, "A").await;
    let observations = state
        .tenants
        .iter()
        .map(|entry| crate::audit::read_test_observer::observe(entry.store.sqlite().unwrap()))
        .collect::<Vec<_>>();
    // Valid JSON whitespace changes the actual retained private file, not DTO fields.
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(b"\n").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let (status, _, body) = response(&router, "GET", RESERVED_PROTOCOL, &[RUNNER], false).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    no_protocol(&body);
    assert!(
        observations
            .iter()
            .all(|observed| observed.try_recv().is_err())
    );
    drain_invalid_owner(&state).await;
}

struct MovedSource {
    source: PathBuf,
    retained: PathBuf,
    restored: bool,
}
impl MovedSource {
    fn hold(source: PathBuf) -> Self {
        let retained = source.with_extension("a0-private-held");
        assert!(!retained.exists());
        std::fs::rename(&source, &retained).unwrap();
        Self {
            source,
            retained,
            restored: false,
        }
    }
    fn restore(mut self) -> std::io::Result<()> {
        std::fs::rename(&self.retained, &self.source)?;
        self.restored = true;
        Ok(())
    }
}
impl Drop for MovedSource {
    fn drop(&mut self) {
        // Both paths are under this test's fresh private fixture root only.
        if !self.restored {
            let _ = std::fs::rename(&self.retained, &self.source);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_missing_source_path_refuses_without_neighbor_sql() {
    let mut fixture = Store::owned_control_fixture_for_test(14);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let state = gateway(bundle, credentials, tokens);
    let router = gateway_router(state.clone());
    require_protocol(&router, RUNNER, "A").await;
    let owned = state.owned_human.as_ref().unwrap();
    let a = tenant(&state, "A");
    let observations = state
        .tenants
        .iter()
        .map(|entry| crate::audit::read_test_observer::observe(entry.store.sqlite().unwrap()))
        .collect::<Vec<_>>();
    let held = MovedSource::hold(owned.sources[a].descriptor().storage_binding.path());
    let (status, _, body) = response(&router, "GET", RESERVED_PROTOCOL, &[RUNNER], false).await;
    held.restore().unwrap();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    no_protocol(&body);
    assert!(
        observations
            .iter()
            .all(|observed| observed.try_recv().is_err())
    );
    drain_invalid_owner(&state).await;
}

struct HeldMutex {
    release: Option<mpsc::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl HeldMutex {
    fn hold(store: Arc<Store>) -> Self {
        let (ready, entered) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let worker = thread::spawn(move || store.modern_test_hold_mutex(ready, released));
        let held = Self {
            release: Some(release),
            worker: Some(worker),
        };
        entered.recv_timeout(OBSERVE).unwrap();
        held
    }
    fn close(mut self) {
        self.release.take().unwrap().send(()).unwrap();
        self.worker.take().unwrap().join().unwrap();
    }
}
impl Drop for HeldMutex {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_own_mutex_refuses_while_held_and_preserves_timeout713() {
    let mut fixture = Store::owned_control_fixture_for_test(14);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let state = gateway(bundle, credentials, tokens);
    let router = gateway_router(state.clone());
    require_protocol(&router, RUNNER, "A").await;
    let a = tenant(&state, "A");
    let b = tenant(&state, "B");
    let store = Arc::clone(state.tenants[a].store.sqlite().unwrap());
    assert!(store.healthy());
    store.owned_control_set_busy_timeout_for_test(713).unwrap();
    let observed_b =
        crate::audit::read_test_observer::observe(state.tenants[b].store.sqlite().unwrap());
    let held = HeldMutex::hold(Arc::clone(&store));
    let result = tokio::time::timeout(
        OBSERVE,
        response(&router, "GET", RESERVED_PROTOCOL, &[RUNNER], false),
    )
    .await;
    held.close();
    let (status, _, body) =
        result.expect("a no-wait refusal must complete while its own mutex is still held");
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    no_protocol(&body);
    assert!(store.healthy());
    assert_eq!(store.owned_control_busy_timeout_for_test().unwrap(), 713);
    assert!(observed_b.try_recv().is_err());
    require_protocol(&router, RUNNER, "A").await;
    assert!(store.healthy());
    assert_eq!(store.owned_control_busy_timeout_for_test().unwrap(), 713);
    drain(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_own_sqlite_busy_refuses_and_restores_timeout713() {
    let mut fixture = Store::owned_control_fixture_for_test(14);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let state = gateway(bundle, credentials, tokens);
    let router = gateway_router(state.clone());
    require_protocol(&router, RUNNER, "A").await;
    let owned = state.owned_human.as_ref().unwrap();
    let a = tenant(&state, "A");
    let b = tenant(&state, "B");
    let store = &state.tenants[a].store;
    assert!(store.healthy());
    store
        .sqlite()
        .unwrap()
        .owned_control_set_busy_timeout_for_test(713)
        .unwrap();
    let observed_b =
        crate::audit::read_test_observer::observe(state.tenants[b].store.sqlite().unwrap());
    let connection =
        rusqlite::Connection::open(owned.sources[a].descriptor().storage_binding.path()).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (status, _, body) = response(&router, "GET", RESERVED_PROTOCOL, &[RUNNER], false).await;
    assert!(
        !connection.is_autocommit(),
        "the real competing writer is still held at the refusal"
    );
    let timeout_after_refusal = store
        .sqlite()
        .unwrap()
        .owned_control_busy_timeout_for_test()
        .unwrap();
    connection.execute_batch("ROLLBACK").unwrap();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    no_protocol(&body);
    assert_eq!(timeout_after_refusal, 713);
    assert!(store.healthy());
    assert!(observed_b.try_recv().is_err());
    require_protocol(&router, RUNNER, "A").await;
    assert!(store.healthy());
    assert_eq!(
        store
            .sqlite()
            .unwrap()
            .owned_control_busy_timeout_for_test()
            .unwrap(),
        713
    );
    drain(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_b_sdk_succeeds_while_a_mutex_and_sqlite_are_unavailable() {
    let mut fixture = Store::owned_control_fixture_for_test(14);
    let (bundle, credentials, tokens) = fixture.take_startup();
    provision_b(&bundle);
    let state = gateway(bundle, credentials, tokens);
    let owned = state.owned_human.as_ref().unwrap();
    let a = tenant(&state, "A");
    let b = tenant(&state, "B");
    let namespace = owned.sources[b]
        .descriptor()
        .durable_namespace_id
        .to_string();
    let router = gateway_router(state.clone());
    require_protocol(&router, B_RUNNER, "B").await;
    let observed_a =
        crate::audit::read_test_observer::observe(state.tenants[a].store.sqlite().unwrap());
    let observed_b =
        crate::audit::read_test_observer::observe(state.tenants[b].store.sqlite().unwrap());
    let connection =
        rusqlite::Connection::open(owned.sources[a].descriptor().storage_binding.path()).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    let held = HeldMutex::hold(Arc::clone(state.tenants[a].store.sqlite().unwrap()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let mut server = HttpServer {
        stop: Some(stop),
        task: Some(task),
    };
    let client = AuditClient::new(&url, B_RUNNER).unwrap();
    let result = tokio::time::timeout(OBSERVE, client.control_supervision_identity()).await;
    let a_sql = observed_a.try_iter().collect::<Vec<_>>();
    let b_sql = observed_b.try_iter().collect::<Vec<_>>();
    let writer_still_held = !connection.is_autocommit();
    held.close();
    connection.execute_batch("ROLLBACK").unwrap();
    drop(client);
    server.close().await;
    drain(&state).await;
    let protocol = result
        .expect("B observation budget, not a latency claim")
        .unwrap();
    assert_eq!(protocol.tenant_id, "B");
    assert_eq!(
        protocol.control_namespace_uuid.as_deref(),
        Some(namespace.as_str())
    );
    assert_eq!(protocol.role, "runner");
    assert_eq!(protocol.schema_version, 14);
    assert!(writer_still_held && a_sql.is_empty());
    assert!(
        !b_sql.is_empty(),
        "successful B diagnostic must use its real checked Store"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_public_head_mutations_and_unknown_routes_keep_their_boundaries() {
    let mut fixture = Store::owned_control_fixture_for_test(14);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let state = gateway(bundle, credentials, tokens);
    let router = gateway_router(state.clone());
    require_protocol(&router, RUNNER, "A").await;
    let observations = state
        .tenants
        .iter()
        .map(|entry| crate::audit::read_test_observer::observe(entry.store.sqlite().unwrap()))
        .collect::<Vec<_>>();
    for (method, path, expected) in [
        (
            "GET",
            "/api/v1/control/protocol",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "HEAD",
            "/api/v1/control/protocol",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "POST",
            "/control/api/v1/workers/supervised-reservations",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "POST",
            "/control/api/v1/control/events",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "POST",
            "/control/api/v1/decide",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        ("GET", "/api/v1/a0-unknown", StatusCode::NOT_FOUND),
        ("GET", "/control/api/v1/a0-unknown", StatusCode::NOT_FOUND),
        ("POST", RESERVED_PROTOCOL, StatusCode::METHOD_NOT_ALLOWED),
        (
            "POST",
            "/api/v1/control/protocol",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
    ] {
        let (status, headers, body) = response(&router, method, path, &[RUNNER], true).await;
        assert_eq!(status, expected, "{method} {path}: {body}");
        if status == StatusCode::SERVICE_UNAVAILABLE {
            assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
        }
        no_protocol(&body);
    }
    let (status, _, body) = response(&router, "HEAD", RESERVED_PROTOCOL, &[RUNNER], true).await;
    assert!(matches!(
        status,
        StatusCode::SERVICE_UNAVAILABLE | StatusCode::METHOD_NOT_ALLOWED
    ));
    no_protocol(&body);
    assert!(
        observations
            .iter()
            .all(|observed| observed.try_recv().is_err())
    );
    // A public request cannot mint the private transport classification in a header.
    let request = HttpRequest::builder()
        .uri("/api/v1/control/protocol")
        .header(header::AUTHORIZATION, format!("Bearer {RUNNER}"))
        .header("x-ai-guard-control-transport", "runner")
        .body(Body::empty())
        .unwrap();
    let result = router.clone().oneshot(request).await.unwrap();
    assert_eq!(result.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        observations
            .iter()
            .all(|observed| observed.try_recv().is_err())
    );
    // A free tenant hint cannot select B or borrow its Store.
    let request = HttpRequest::builder()
        .uri(RESERVED_PROTOCOL)
        .header(header::AUTHORIZATION, format!("Bearer {RUNNER}"))
        .header("x-tenant-id", "B")
        .body(Body::empty())
        .unwrap();
    let response = tokio::time::timeout(OBSERVE, router.clone().oneshot(request))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 512 * 1024)
        .await
        .unwrap();
    let protocol: ControlProtocolV1 = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(protocol.tenant_id, "A");
    let a = tenant(&state, "A");
    let namespace = state.owned_human.as_ref().unwrap().sources[a]
        .descriptor()
        .durable_namespace_id
        .to_string();
    assert_eq!(
        protocol.control_namespace_uuid.as_deref(),
        Some(namespace.as_str())
    );
    assert!(observations[tenant(&state, "B")].try_recv().is_err());
    require_protocol(&router, RUNNER, "A").await;
    drain(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_retired_owner_refuses_before_any_new_store_work() {
    let mut fixture = Store::owned_control_fixture_for_test(14);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let state = gateway(bundle, credentials, tokens);
    let router = gateway_router(state.clone());
    require_protocol(&router, RUNNER, "A").await;
    let observations = state
        .tenants
        .iter()
        .map(|entry| crate::audit::read_test_observer::observe(entry.store.sqlite().unwrap()))
        .collect::<Vec<_>>();
    let retirement = state.human_retirement().unwrap();
    retirement.retire();
    let (status, _, body) = response(&router, "GET", RESERVED_PROTOCOL, &[RUNNER], false).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    no_protocol(&body);
    assert!(
        observations
            .iter()
            .all(|observed| observed.try_recv().is_err())
    );
    tokio::time::timeout(OBSERVE, retirement.drain())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_unknown_diagnostic_formats_remain_closed() {
    let mut fixture = Store::owned_control_fixture_for_test(14);
    let (bundle, credentials, tokens) = fixture.take_startup();
    let state = gateway(bundle, credentials, tokens);
    let protocol = require_protocol(&gateway_router(state.clone()), RUNNER, "A").await;
    for format in [0, 16, u32::MAX] {
        for modern_shape in [false, true] {
            let mut value = serde_json::to_value(&protocol).unwrap();
            value["schema_version"] = json!(format);
            if !modern_shape {
                value["control_namespace_uuid"] = Value::Null;
                value["protocols"] = json!([]);
                value["active"] = json!(false);
            }
            let invalid: ControlProtocolV1 = serde_json::from_value(value).unwrap();
            assert!(
                invalid.validate().is_err(),
                "unsupported diagnostic format {format}, modern_shape={modern_shape}"
            );
        }
    }
    drain(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_control_legacy_protocol_keeps_runner_viewer_and_head_behavior() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let store = Store::open_for_tenant(
        &directory.path().join("legacy.sqlite"),
        "legacy-A",
        Redactor::new(vec![]),
        10_000,
    )
    .unwrap();
    store
        .owned_control_migrate_legacy_fixture_for_test(directory.path())
        .unwrap();
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "legacy-A".into(),
            runner_token: "synthetic-legacy-runner".into(),
            viewer_token: "synthetic-legacy-viewer".into(),
            store: Arc::new(store),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    assert!(state.owned_human.is_none());
    let router = gateway_router(state);
    for (bearer, role) in [
        ("synthetic-legacy-runner", "runner"),
        ("synthetic-legacy-viewer", "viewer"),
    ] {
        let (status, _, body) =
            response(&router, "GET", "/api/v1/control/protocol", &[bearer], false).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let protocol: ControlProtocolV1 = serde_json::from_value(body).unwrap();
        protocol.validate().unwrap();
        assert_eq!(protocol.schema_version, 13);
        assert_eq!(protocol.tenant_id, "legacy-A");
        assert_eq!(protocol.role, role);
        let (status, _, body) = response(
            &router,
            Method::HEAD.as_str(),
            "/api/v1/control/protocol",
            &[bearer],
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_null());
    }
}
