//! Human worker registry boundary on the real HTTP router and private tenant SQLite.
//! No runtime or model is started; these tests never publish fixture activity to a product stack.
#[path = "human_workers_api/federation.rs"]
mod federation;
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker_registry::{BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRuntimeKindV1},
    control::*,
    identity::*,
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::Arc,
};
use uuid::Uuid;

fn id() -> String {
    Uuid::new_v4().to_string()
}

/// Optional QA-only exact response handoff. The caller supplies a private QA
/// directory; no path, token or transcript is exported by normal test runs.
fn write_legacy_wire(bytes: &[u8], name: &str, token: Option<&str>) {
    assert!(bytes.len() <= HUMAN_MAX_PAGE_BYTES);
    assert!(matches!(
        name,
        "nil-session.json" | "invalid-runtime-date.json" | "large-exit-code.json"
    ));
    let Some(path) = std::env::var_os("AIGUARD_HUMAN_WORKERS_WIRE_DIR") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    assert!(
        path.is_absolute(),
        "wire directory must be the absolute QA-owned private path"
    );
    let directory = agentsandbox::control_fs::Directory::open_existing(&path).unwrap();
    if let Some(token) = token {
        assert!(!token.is_empty());
        assert!(
            !bytes
                .windows(token.len())
                .any(|part| part == token.as_bytes()),
            "a fixture credential must never enter the wire artifact"
        );
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory.path().join(name))
        .unwrap();
    assert!(file.metadata().unwrap().is_file());
    assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    directory.revalidate().unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    directory.revalidate().unwrap();
}

fn worker_change(worker: &WorkerV1, transition: WorkerTransitionV1) -> WorkerMutationV1 {
    WorkerMutationV1 {
        version: 1,
        operation_id: id(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    }
}

fn runtime_inspection(worker: &WorkerV1) -> RuntimeInspectionV1 {
    RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: "c".repeat(64),
        stable_name: worker.attempt.stable_name.clone(),
        volume_name: worker.attempt.volume_name.clone(),
        image_id: worker.launch.image_id.clone(),
        spec_hash: worker.spec_hash.clone(),
        cpu_millis: worker.launch.cpu_millis,
        memory_bytes: worker.launch.memory_bytes,
        pids_limit: worker.launch.pids_limit,
        network: worker.launch.network,
        security_preset: worker.launch.security_preset,
        restart_policy: worker.launch.restart_policy,
    }
}

/// These are actual historical schema2 control mutations, not injected rows or
/// a runtime claim. The resulting audit is migrated normally before human read.
fn legacy_started_worker(store: &Store) -> WorkerV1 {
    let mut worker = reserve_with_protocol(store, &id(), true);
    for transition in [
        WorkerTransitionV1::Prepare,
        WorkerTransitionV1::CreateIssued,
    ] {
        worker = store
            .mutate_worker(&worker_change(&worker, transition))
            .unwrap()
            .worker;
    }
    let inspection = runtime_inspection(&worker);
    worker = store
        .mutate_worker(&worker_change(
            &worker,
            WorkerTransitionV1::Created { inspection },
        ))
        .unwrap()
        .worker;
    let claim = store
        .mutate_worker(&worker_change(
            &worker,
            WorkerTransitionV1::StartClaim { emitter_id: id() },
        ))
        .unwrap();
    assert_eq!(claim.disposition, Some(ClaimDispositionV1::EmitOnce));
    assert_eq!(claim.worker.state, WorkerStateV1::StartIssued);
    assert!(
        store.integrity().unwrap().valid,
        "actual admission, Created and StartClaim must replay before the observation"
    );
    claim.worker
}

fn reserve(store: &Store, authority: &str) -> WorkerV1 {
    reserve_with_protocol(store, authority, false)
}

fn reserve_with_protocol(store: &Store, authority: &str, legacy: bool) -> WorkerV1 {
    reserve_with_session(store, authority, legacy, None)
}

fn reserve_with_session(
    store: &Store,
    authority: &str,
    legacy: bool,
    session: Option<&str>,
) -> WorkerV1 {
    let session = session
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let request = DecisionRequest {
        request_id: Uuid::new_v4().to_string(),
        profile: "strict".into(),
        command: CommandSpec {
            argv: vec!["/usr/bin/id".into()],
            cwd: "/workspace".into(),
        },
        session_id: Some(session.clone()),
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let policy = Policy::default();
    let decision = policy::evaluate(&policy, &request);
    let stored = store
        .decision(&request, Some((decision.clone(), json!(null))))
        .unwrap()
        .unwrap();
    let admission = AdmissionV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id: request.request_id.clone(),
            request_hash: policy::request_hash(&request),
            phase: DecisionPhaseV1::Initial,
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        },
        launch: LaunchSpecV1 {
            version: 1,
            session_id: session,
            command: request.command.clone(),
            profile: request.profile.clone(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: authority.into(),
            image_ref: "worker:fixture".into(),
            image_id: format!("sha256:{}", "d".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 60,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: "e".repeat(64),
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
        },
        decision_request: request,
    };
    let compact = Uuid::new_v4().simple().to_string();
    let supervised = SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: format!("{compact}{compact}"),
        admission,
    };
    supervised.validate().expect(
        "fixture must satisfy the existing fixed launch and supervised admission contracts",
    );
    let result = if legacy {
        assert_eq!(store.supervision_state().unwrap().schema_version, 2);
        store.admit_worker(
            &supervised.admission,
            &policy,
            decision,
            json!(null),
            &QuotaPolicyV1::default(),
        )
    } else {
        store.admit_supervised(
            &supervised,
            &policy,
            decision,
            json!(null),
            &QuotaPolicyV1::default(),
        )
    }
    .unwrap();
    assert!(
        result.accepted,
        "real admission must succeed before reading federation"
    );
    result.worker.unwrap()
}

fn mutation(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: id(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "human-workers-test-trusted-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}

fn store(path: &Path, tenant: &str) -> Arc<Store> {
    let store = Store::open_for_tenant(path, tenant, Redactor::new(vec![]), 10_000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("before3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("before5"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("before6"))
        .unwrap();
    store
        .migrate_human_brokers_v7(&path.with_extension("before7"))
        .unwrap();
    Arc::new(store)
}

fn provision(store: &Store, capabilities: &[HumanCapabilityV1]) -> String {
    let token = generate_local_credential().unwrap();
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: id(),
    };
    store
        .provision_local_human(&LocalHumanProvisionV1 {
            version: 1,
            mutations: [
                mutation(IdentityChangeV1::Account {
                    expected_revision: 0,
                    account: HumanAccountV1 {
                        version: 1,
                        principal: principal.clone(),
                        display_name: "Named worker registry test reader".into(),
                        revision: 1,
                        active: true,
                    },
                }),
                mutation(IdentityChangeV1::Credential {
                    expected_revision: 0,
                    binding: LocalCredentialV1 {
                        version: 1,
                        credential_id: id(),
                        principal: principal.clone(),
                        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                        revision: 1,
                        active: true,
                        expires_at: None,
                    },
                }),
                mutation(IdentityChangeV1::Membership {
                    expected_revision: 0,
                    membership: MembershipV1 {
                        version: 1,
                        membership_id: id(),
                        principal,
                        tenant_id: store.tenant_id().into(),
                        organization_scope: "worker-registry-test-organization".into(),
                        revision: 1,
                        active: true,
                        capabilities: capabilities.iter().copied().collect(),
                    },
                }),
            ],
        })
        .unwrap();
    token
}

struct Fixture {
    base: String,
    client: Client,
    a: Arc<Store>,
    b: Arc<Store>,
    task: tokio::task::JoinHandle<()>,
    _files: tempfile::TempDir,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    fn migrate_legacy_a_to10(&self) {
        let path = self._files.path().join("a.sqlite");
        self.a
            .migrate_control_v3(&path.with_extension("before3"))
            .unwrap();
        self.a
            .migrate_identity_v4(&path.with_extension("before4"))
            .unwrap();
        self.a
            .migrate_human_access_v5(&path.with_extension("before5"))
            .unwrap();
        self.a
            .migrate_workload_v6(&path.with_extension("before6"))
            .unwrap();
        self.a
            .migrate_human_brokers_v7(&path.with_extension("before7"))
            .unwrap();
        self.upgrade10();
    }
    fn upgrade10(&self) {
        for (store, name) in [(&self.a, "a"), (&self.b, "b")] {
            let path = self._files.path().join(format!("{name}.sqlite"));
            store
                .migrate_kubernetes_create_v8(&path.with_extension("before8"))
                .unwrap();
            store
                .migrate_kubernetes_policy_v9(&path.with_extension("before9"))
                .unwrap();
            store
                .migrate_human_workers_v10(&path.with_extension("before10"))
                .unwrap();
        }
    }
    async fn new() -> Self {
        Self::with_read_budget(None, None).await
    }
    async fn with_read_budget(principal_events: Option<u64>, tenant_events: Option<u64>) -> Self {
        Self::with_options(principal_events, tenant_events, false).await
    }
    async fn legacy_a() -> Self {
        Self::with_options(None, None, true).await
    }
    async fn with_options(
        principal_events: Option<u64>,
        tenant_events: Option<u64>,
        legacy_a: bool,
    ) -> Self {
        let files = support::private_tempdir();
        let a = if legacy_a {
            Arc::new(
                Store::open_for_tenant(
                    &files.path().join("a.sqlite"),
                    "tenant-a",
                    Redactor::new(vec![]),
                    10_000,
                )
                .unwrap(),
            )
        } else {
            store(&files.path().join("a.sqlite"), "tenant-a")
        };
        let mut policy = a.capacity_policy().clone();
        if let Some(events) = principal_events {
            policy.human_principal.events = events;
        }
        if let Some(events) = tenant_events {
            policy.human_tenant.events = events;
        }
        let a = Arc::new(
            Arc::try_unwrap(a)
                .ok()
                .unwrap()
                .with_capacity_policy(policy)
                .unwrap(),
        );
        let b = store(&files.path().join("b.sqlite"), "tenant-b");
        let state = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: a.clone(),
                    runner_token: "human-workers-test-runner-a".repeat(3),
                    viewer_token: "human-workers-test-viewer-a".repeat(3),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b.clone(),
                    runner_token: "human-workers-test-runner-b".repeat(3),
                    viewer_token: "human-workers-test-viewer-b".repeat(3),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, gateway_router(state)).await.unwrap();
        });
        Self {
            base,
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
            a,
            b,
            task,
            _files: files,
        }
    }

    async fn get(
        &self,
        path: &str,
        token: Option<&str>,
        context: Option<&str>,
    ) -> (StatusCode, Value) {
        self.get_wire(path, token, context, None).await
    }

    async fn get_wire(
        &self,
        path: &str,
        token: Option<&str>,
        context: Option<&str>,
        wire_name: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = self.client.get(format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(context) = context {
            request = request.header("X-AiGuard-Context", context);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let bytes = response.bytes().await.unwrap();
        assert!(bytes.len() <= HUMAN_MAX_PAGE_BYTES);
        if let Some(name) = wire_name {
            write_legacy_wire(&bytes, name, token);
        }
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn context(&self, token: &str, mode: &str) -> String {
        let (status, identity) = self.get("/api/v1/human/identity", Some(token), None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the real local human must authenticate"
        );
        assert_eq!(identity["authentication"]["method"], "local_credential");
        identity["contexts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|context| context["mode"] == mode)
            .expect("the control must offer the requested real context")["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

#[tokio::test]
async fn worker_route_authenticates_before_accepting_query_and_rejects_technical_proofs() {
    let f = Fixture::new().await;
    let heads = (f.a.head().unwrap(), f.b.head().unwrap());
    for token in [
        None,
        Some("invalid-personal-proof".to_owned()),
        Some("human-workers-test-viewer-a".repeat(3)),
        Some("human-workers-test-runner-a".repeat(3)),
    ] {
        let (status, _) = f
            .get(
                "/api/v1/human/workers?limit=invalid",
                token.as_deref(),
                None,
            )
            .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "the dedicated human route must exist and authenticate before query parsing"
        );
    }
    assert_eq!((f.a.head().unwrap(), f.b.head().unwrap()), heads);
}

#[tokio::test]
async fn metadata_capabilities_never_authorize_tenant_worker_registry_reads() {
    let f = Fixture::new().await;
    let token = provision(
        &f.a,
        &[
            HumanCapabilityV1::ReadMetadata,
            HumanCapabilityV1::ReadTenantMetadata,
        ],
    );
    for mode in ["personal", "administration"] {
        let context = f.context(&token, mode).await;
        let heads = (f.a.head().unwrap(), f.b.head().unwrap());
        let (status, body) = f
            .get("/api/v1/human/workers", Some(&token), Some(&context))
            .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "metadata is never tenant worker registry authority"
        );
        assert_eq!(body["error"]["code"], "permission_denied");
        assert_eq!((f.a.head().unwrap(), f.b.head().unwrap()), heads);
    }
}

#[tokio::test]
async fn a_real_broker_administrator_has_no_implicit_worker_registry_permission() {
    let f = Fixture::new().await;
    let token = provision(&f.a, &[HumanCapabilityV1::ReadBrokerRegistry]);
    let context = f.context(&token, "administration").await;
    let (status, brokers) = f
        .get("/api/v1/human/brokers", Some(&token), Some(&context))
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "positive control: this is an admitted infrastructure administrator"
    );
    assert_eq!(brokers["source"], "tenant_broker_registry");
    assert!(brokers["read_receipt"]["audit_event_id"].is_string());
    let heads = (f.a.head().unwrap(), f.b.head().unwrap());
    let (status, body) = f
        .get("/api/v1/human/workers", Some(&token), Some(&context))
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a broker registry grant cannot be recycled as worker authority"
    );
    assert_eq!(body["error"]["code"], "permission_denied");
    assert_eq!((f.a.head().unwrap(), f.b.head().unwrap()), heads);
}

#[tokio::test]
async fn a_context_issued_to_another_human_and_tenant_cannot_select_worker_scope() {
    let f = Fixture::new().await;
    let token_a = provision(&f.a, &[HumanCapabilityV1::ReadTenantMetadata]);
    let token_b = provision(&f.b, &[HumanCapabilityV1::ReadTenantMetadata]);
    let context_b = f.context(&token_b, "administration").await;
    let heads = (f.a.head().unwrap(), f.b.head().unwrap());
    let (status, body) = f
        .get("/api/v1/human/workers", Some(&token_a), Some(&context_b))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "context_invalid");
    assert_eq!((f.a.head().unwrap(), f.b.head().unwrap()), heads);
}

#[test]
fn worker_registry_permission_is_an_explicit_closed_capability() {
    let capability = serde_json::from_value::<HumanCapabilityV1>(json!("read_worker_registry"))
        .expect("the worker registry needs its own explicit capability");
    assert_ne!(capability, HumanCapabilityV1::ReadMetadata);
    assert_ne!(capability, HumanCapabilityV1::ReadTenantMetadata);
    assert_ne!(capability, HumanCapabilityV1::ReadBrokerRegistry);
    assert_eq!(
        serde_json::to_value(capability).unwrap(),
        json!("read_worker_registry")
    );
}

#[tokio::test]
async fn explicit_worker_reader_gets_only_own_tenant_metadata_and_durable_receipts() {
    let f = Fixture::new().await;
    f.upgrade10();
    let token = provision(&f.a, &[HumanCapabilityV1::ReadWorkerRegistry]);
    let context = f.context(&token, "administration").await;
    let personal = f.context(&token, "personal").await;
    let authority = id();
    let grant = BrokerGrantMutationV1 {
        version: 1,
        operation_id: id(),
        broker_id: id(),
        expected_revision: None,
        credential_id: id(),
        incarnation_id: id(),
        authorities: vec![BrokerAuthorityV1 {
            authority_id: authority.clone(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        }],
        lease_seconds: 30,
        not_after: "2100-01-01T00:00:00.000Z".into(),
    };
    f.a.mutate_broker_grant(&grant).unwrap();
    let first = reserve(&f.a, &authority);
    let second = reserve(&f.a, &id());
    let foreign = reserve(&f.b, &authority);
    let before = f.a.head().unwrap();
    let (status, page) = f
        .get(
            "/api/v1/human/workers?limit=1",
            Some(&token),
            Some(&context),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(f.a.head().unwrap(), before + 1);
    assert_eq!(page["context_id"], context);
    assert_eq!(page["source"], "tenant_worker_broker_registry");
    assert_eq!(page["consistency"], "snapshot_per_page");
    assert_eq!(page["runtime_inventory"], "unknown");
    assert_eq!(page["registry_status"], "available");
    assert_eq!(page["has_more"], true);
    assert!(page["read_receipt"]["audit_event_id"].is_string());
    let cursor = page["next_cursor"].as_str().unwrap();
    let (status, next) = f
        .get(
            &format!("/api/v1/human/workers?limit=1&cursor={cursor}"),
            Some(&token),
            Some(&context),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(next["has_more"], false);
    let mut ids = vec![];
    for item in [page["items"][0].clone(), next["items"][0].clone()] {
        let worker_id = item["worker_id"].as_str().unwrap().to_owned();
        assert_ne!(worker_id, foreign.worker_id);
        assert!(worker_id == first.worker_id || worker_id == second.worker_id);
        assert_eq!(item["session_provenance"], "declared");
        assert_eq!(item["last_observation"], Value::Null);
        assert!(item["revision"].is_string());
        assert!(item["memory_bytes"].is_string());
        for forbidden in [
            "launch",
            "command",
            "argv",
            "env",
            "credential_binding_id",
            "credential_names",
            "container_id",
            "stable_name",
            "volume_name",
            "cleanup_error",
            "unknown_reason",
        ] {
            assert!(
                item.get(forbidden).is_none(),
                "forbidden technical/content field {forbidden}"
            );
        }
        if worker_id == first.worker_id {
            assert_eq!(item["launch_broker"]["status"], "registered");
            assert_eq!(
                item["launch_broker"]["broker"]["broker_id"],
                grant.broker_id
            );
        } else {
            assert_eq!(item["launch_broker"]["status"], "unregistered");
        }
        ids.push(worker_id);
    }
    assert_ne!(ids[0], ids[1]);
    for path in [
        format!("/api/v1/human/workers?limit=2&cursor={cursor}"),
        "/api/v1/human/workers?tenant_id=tenant-b".into(),
        "/api/v1/human/workers?limit=0".into(),
    ] {
        assert_eq!(
            f.get(&path, Some(&token), Some(&context)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        f.get("/api/v1/human/workers", Some(&token), Some(&personal))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.get("/api/v1/human/brokers", Some(&token), Some(&context))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert!(f.a.integrity().unwrap().valid);
    let reopened = Store::open_existing_for_tenant(
        &f._files.path().join("a.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        10_000,
    )
    .unwrap();
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(reopened.head().unwrap(), f.a.head().unwrap());
}

#[tokio::test]
async fn worker_receipts_share_durable_human_principal_and_tenant_budgets() {
    for tenant_bound in [false, true] {
        let f = Fixture::with_read_budget(Some(3), Some(if tenant_bound { 4 } else { 100 })).await;
        f.upgrade10();
        let caps = [
            HumanCapabilityV1::ReadBrokerRegistry,
            HumanCapabilityV1::ReadWorkerRegistry,
        ];
        let token = provision(&f.a, &caps);
        let other = provision(&f.a, &caps);
        let context = f.context(&token, "administration").await;
        assert_eq!(
            f.get("/api/v1/human/brokers", Some(&token), Some(&context))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            f.get("/api/v1/human/workers", Some(&token), Some(&context))
                .await
                .0,
            StatusCode::OK
        );
        let before = f.a.head().unwrap();
        let (status, error) = f
            .get("/api/v1/human/workers", Some(&token), Some(&context))
            .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(error["error"]["code"], "audit_read_budget_exhausted");
        assert_eq!(f.a.head().unwrap(), before);
        let (status, identity) = f.get("/api/v1/human/identity", Some(&other), None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a second principal retains its own share"
        );
        let other_context = identity["contexts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|context| context["mode"] == "administration")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        assert_eq!(
            f.get("/api/v1/human/workers", Some(&other), Some(other_context))
                .await
                .0,
            if tenant_bound {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::OK
            }
        );
        let reopened = Store::open_existing_for_tenant(
            &f._files.path().join("a.sqlite"),
            "tenant-a",
            Redactor::new(vec![]),
            10_000,
        )
        .unwrap()
        .with_capacity_policy(f.a.capacity_policy().clone())
        .unwrap();
        assert!(
            reopened.integrity().unwrap().valid,
            "worker reads must replay as typed human reads"
        );
        assert_eq!(reopened.head().unwrap(), f.a.head().unwrap());
        let restarted = gateway_router(
            GatewayState::for_tenants(
                vec![TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: Arc::new(reopened),
                    runner_token: "worker-budget-restart-runner".repeat(3),
                    viewer_token: "worker-budget-restart-viewer".repeat(3),
                }],
                Policy::default(),
                None,
                Redactor::new(vec![]),
            )
            .unwrap(),
        );
        use tower::ServiceExt;
        let response = restarted
            .oneshot(
                axum::http::Request::get("/api/v1/human/identity")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "reopen and fresh gateway memory do not reset this principal's durable read budget"
        );
    }
}

#[tokio::test]
async fn a_revoked_worker_context_and_cursor_never_publish_cached_rows() {
    let f = Fixture::new().await;
    f.upgrade10();
    let token = provision(&f.a, &[HumanCapabilityV1::ReadWorkerRegistry]);
    let (status, identity) = f.get("/api/v1/human/identity", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let principal: PrincipalRefV1 = serde_json::from_value(identity["principal"].clone()).unwrap();
    let context = f.context(&token, "administration").await;
    reserve(&f.a, &id());
    reserve(&f.a, &id());
    let (status, page) = f
        .get(
            "/api/v1/human/workers?limit=1",
            Some(&token),
            Some(&context),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let cursor = page["next_cursor"].as_str().unwrap();
    let mut membership = f.a.identity_memberships(&principal).unwrap().remove(0);
    membership.revision += 1;
    membership.capabilities.clear();
    f.a.mutate_identity(&mutation(IdentityChangeV1::Membership {
        expected_revision: membership.revision - 1,
        membership,
    }))
    .unwrap();
    let head = f.a.head().unwrap();
    let (status, error) = f
        .get(
            &format!("/api/v1/human/workers?limit=1&cursor={cursor}"),
            Some(&token),
            Some(&context),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error["error"]["code"], "context_revoked");
    assert!(error.get("items").is_none());
    assert_eq!(f.a.head().unwrap(), head);
}

#[tokio::test]
async fn organization_membership_admin_cannot_introduce_the_worker_registry_capability() {
    let f = Fixture::new().await;
    f.upgrade10();
    let token = provision(&f.a, &[HumanCapabilityV1::ManageMemberships]);
    let (_, identity) = f.get("/api/v1/human/identity", Some(&token), None).await;
    let principal: PrincipalRefV1 = serde_json::from_value(identity["principal"].clone()).unwrap();
    let mut membership = f.a.identity_memberships(&principal).unwrap().remove(0);
    membership.revision += 1;
    membership
        .capabilities
        .insert(HumanCapabilityV1::ReadWorkerRegistry);
    let mut input = mutation(IdentityChangeV1::Membership {
        expected_revision: membership.revision - 1,
        membership,
    });
    input.actor = principal;
    let head = f.a.head().unwrap();
    assert_eq!(
        f.a.mutate_identity(&input)
            .unwrap_err()
            .downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::PermissionDenied)
    );
    assert_eq!(f.a.head().unwrap(), head);
}

#[tokio::test]
async fn legacy_canonical_nil_session_remains_readable_after_a_real_admission() {
    let f = Fixture::new().await;
    f.upgrade10();
    let nil = Uuid::nil().to_string();
    let worker = reserve_with_session(&f.a, &id(), false, Some(&nil));
    assert_eq!(
        worker.launch.session_id, nil,
        "the admitted Request and Launch retain the same canonical nil session"
    );
    assert!(f.a.integrity().unwrap().valid);
    let token = provision(&f.a, &[HumanCapabilityV1::ReadWorkerRegistry]);
    let context = f.context(&token, "administration").await;
    assert!(
        f.a.integrity().unwrap().valid,
        "the admitted source is valid immediately before the human projection"
    );
    eprintln!(
        "human_workers_legacy case=nil-session admission=accepted source_integrity=valid schema=10 state={:?}",
        worker.state
    );
    let (status, body) = f
        .get_wire(
            "/api/v1/human/workers",
            Some(&token),
            Some(&context),
            Some("nil-session.json"),
        )
        .await;
    eprintln!(
        "human_workers_legacy case=nil-session human_http={} human_error={}",
        status.as_u16(),
        body["error"]["code"].as_str().unwrap_or("none")
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "a canonical legacy-accepted nil session must not make the human registry unavailable"
    );
    assert_eq!(body["items"][0]["worker_id"], worker.worker_id);
    assert_eq!(body["items"][0]["session_id"], nil);
    assert_eq!(body["items"][0]["session_provenance"], "declared");
}

#[tokio::test]
async fn legacy_uninterpretable_runtime_date_is_typed_unknown_instead_of_failing_the_page() {
    let f = Fixture::legacy_a().await;
    let worker = legacy_started_worker(&f.a);
    let observation = RuntimeObservationV1 {
        inspection: runtime_inspection(&worker),
        observed_at: "2026-10-29T10:00:02Z".into(),
        status: RuntimeStatusV1::Exited,
        running: Some(false),
        started_at: Some("unavailable".into()),
        finished_at: Some("2026-10-29T10:00:01Z".into()),
        exit_code: Some(0),
        oom_killed: Some(false),
        restart_count: Some(0),
    };
    let changed =
        f.a.mutate_worker(&worker_change(
            &worker,
            WorkerTransitionV1::Observe { observation },
        ))
        .unwrap()
        .worker;
    assert_eq!(changed.state, WorkerStateV1::InDoubt);
    assert!(changed.quota_held);
    assert_eq!(
        changed
            .last_observation
            .as_ref()
            .unwrap()
            .started_at
            .as_deref(),
        Some("unavailable")
    );
    assert!(
        f.a.integrity().unwrap().valid,
        "the actual historical Observe and InDoubt snapshot replay"
    );
    f.migrate_legacy_a_to10();
    let token = provision(&f.a, &[HumanCapabilityV1::ReadWorkerRegistry]);
    let context = f.context(&token, "administration").await;
    assert!(
        f.a.integrity().unwrap().valid,
        "the migrated source is valid immediately before the human projection"
    );
    eprintln!(
        "human_workers_legacy case=invalid-runtime-date admission=accepted transitions=Created,StartClaim,Observe source_integrity=valid schema=10 state=in_doubt"
    );
    let (status, body) = f
        .get_wire(
            "/api/v1/human/workers",
            Some(&token),
            Some(&context),
            Some("invalid-runtime-date.json"),
        )
        .await;
    eprintln!(
        "human_workers_legacy case=invalid-runtime-date human_http={} human_error={}",
        status.as_u16(),
        body["error"]["code"].as_str().unwrap_or("none")
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "a legacy-accepted indeterminate runtime date must not make the human registry unavailable"
    );
    let item = &body["items"][0];
    assert_eq!(item["worker_id"], changed.worker_id);
    assert_eq!(item["state"], "in_doubt");
    assert!(item["last_observation"]["started_at"].is_null());
    assert_eq!(item["last_observation"]["started_at_status"], "unknown");
    assert!(
        !body.to_string().contains("unavailable"),
        "do not publish arbitrary raw invalid runtime date text"
    );
}

#[tokio::test]
async fn legacy_large_exit_code_is_an_exact_decimal_in_the_human_wire_contract() {
    let f = Fixture::legacy_a().await;
    let worker = legacy_started_worker(&f.a);
    let exact = 9_007_199_254_740_993i64;
    let observation = RuntimeObservationV1 {
        inspection: runtime_inspection(&worker),
        observed_at: "2026-10-29T10:00:02Z".into(),
        status: RuntimeStatusV1::Exited,
        running: Some(false),
        started_at: Some("2026-10-29T10:00:00Z".into()),
        finished_at: Some("2026-10-29T10:00:01Z".into()),
        exit_code: Some(exact),
        oom_killed: Some(false),
        restart_count: Some(0),
    };
    let changed =
        f.a.mutate_worker(&worker_change(
            &worker,
            WorkerTransitionV1::Observe { observation },
        ))
        .unwrap()
        .worker;
    assert_eq!(changed.state, WorkerStateV1::Terminal);
    assert_eq!(
        changed.last_observation.as_ref().unwrap().exit_code,
        Some(exact)
    );
    assert!(
        f.a.integrity().unwrap().valid,
        "the historical integer and real terminal transition replay exactly"
    );
    f.migrate_legacy_a_to10();
    let token = provision(&f.a, &[HumanCapabilityV1::ReadWorkerRegistry]);
    let context = f.context(&token, "administration").await;
    assert!(
        f.a.integrity().unwrap().valid,
        "the migrated source is valid immediately before the human projection"
    );
    eprintln!(
        "human_workers_legacy case=large-exit-code admission=accepted transitions=Created,StartClaim,Observe source_integrity=valid schema=10 state=terminal exact_exit=9007199254740993"
    );
    let (status, body) = f
        .get_wire(
            "/api/v1/human/workers",
            Some(&token),
            Some(&context),
            Some("large-exit-code.json"),
        )
        .await;
    eprintln!(
        "human_workers_legacy case=large-exit-code human_http={} human_error={}",
        status.as_u16(),
        body["error"]["code"].as_str().unwrap_or("none")
    );
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"][0]["worker_id"], changed.worker_id);
    assert_eq!(
        body["items"][0]["last_observation"]["exit_code"].as_str(),
        Some("9007199254740993"),
        "an i64 value above JS's safe range must travel as an exact decimal string"
    );
}
