//! Real gateway HTTP and tenant SQLite projections; no runtime is launched.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRevocationV1, BrokerRuntimeKindV1,
    },
    control::*,
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
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

const BROKER: &str = "22222222-2222-4222-8222-222222222222";
const AUTHORITY: &str = "11111111-1111-4111-8111-111111111111";
const UNREGISTERED: &str = "55555555-5555-4555-8555-555555555555";

/// Opt-in QA handoff of exact HTTP bytes, never a product exporter.
async fn wire_json(response: reqwest::Response, name: &str) -> Value {
    let bytes = response.bytes().await.unwrap();
    assert!(
        bytes.len() <= 512 * 1024,
        "real wire response must fit its documented bound"
    );
    if let Some(path) = std::env::var_os("AIGUARD_FEDERATION_WIRE_DIR") {
        assert!(matches!(
            name,
            "modern-a.json" | "modern-b.json" | "legacy.json"
        ));
        let path = std::path::PathBuf::from(path);
        assert!(path.is_absolute());
        let directory = agentsandbox::control_fs::Directory::open_existing(&path).unwrap();
        for token in [
            "a".repeat(48),
            "b".repeat(48),
            "v".repeat(48),
            "w".repeat(48),
        ] {
            assert!(
                !bytes
                    .windows(token.len())
                    .any(|part| part == token.as_bytes()),
                "fixture proof must never enter the wire artifact"
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
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        directory.revalidate().unwrap();
    }
    serde_json::from_slice(&bytes).unwrap()
}

fn store(root: &Path, tenant: &str) -> Arc<Store> {
    let path = root.join(format!("{tenant}.sqlite"));
    let store = Store::open_for_tenant(&path, tenant, Redactor::new(vec![]), 10_000).unwrap();
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
    Arc::new(store)
}

fn grant(store: &Store, authority: &str, revision: Option<u64>) -> String {
    let incarnation = Uuid::new_v4().to_string();
    let credential = Uuid::new_v4().to_string();
    store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: BROKER.into(),
                incarnation_id: incarnation.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(Uuid::new_v4().as_bytes())),
            expires_at: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: BROKER.into(),
            expected_revision: revision,
            credential_id: credential,
            incarnation_id: incarnation.clone(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: authority.into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 120,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    incarnation
}

fn reserve(store: &Store, authority: &str) -> WorkerV1 {
    reserve_with_protocol(store, authority, false)
}

fn reserve_with_protocol(store: &Store, authority: &str, legacy: bool) -> WorkerV1 {
    let session = Uuid::new_v4().to_string();
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
            egress: None,
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

struct Fixture {
    base: String,
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
    async fn new() -> Self {
        Self::with_legacy_a(false).await
    }
    async fn with_legacy_a(legacy: bool) -> Self {
        let files = support::private_tempdir();
        let a = if legacy {
            Arc::new(
                Store::open_for_tenant(
                    &files.path().join("tenant-a.sqlite"),
                    "tenant-a",
                    Redactor::new(vec![]),
                    10_000,
                )
                .unwrap(),
            )
        } else {
            store(files.path(), "tenant-a")
        };
        let b = store(files.path(), "tenant-b");
        let state = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: a.clone(),
                    runner_token: "a".repeat(48),
                    viewer_token: "v".repeat(48),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b.clone(),
                    runner_token: "b".repeat(48),
                    viewer_token: "w".repeat(48),
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
            a,
            b,
            task,
            _files: files,
        }
    }
    async fn get(&self, path: &str, token: &str) -> reqwest::Response {
        Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
            .get(format!("{}{path}", self.base))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn federation_projects_workers_and_historical_brokers_in_authenticated_tenant() {
    let f = Fixture::new().await;
    let a_incarnation = grant(&f.a, AUTHORITY, None);
    let b_incarnation = grant(&f.b, AUTHORITY, None);
    let a1 = reserve(&f.a, AUTHORITY);
    let a2 = reserve(&f.a, UNREGISTERED);
    let b1 = reserve(&f.b, AUTHORITY);
    let a_head = f.a.head().unwrap();
    let b_head = f.b.head().unwrap();
    for (token, ids, incarnation) in [
        (
            "v".repeat(48),
            vec![a1.worker_id.clone(), a2.worker_id.clone()],
            a_incarnation,
        ),
        ("w".repeat(48), vec![b1.worker_id.clone()], b_incarnation),
    ] {
        let response = f.get("/api/v1/federation/workers?limit=100", &token).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "new consumed federation route must exist"
        );
        let label = if token == "v".repeat(48) {
            "modern-a.json"
        } else {
            "modern-b.json"
        };
        let body = wire_json(response, label).await;
        assert_eq!(body["version"], 1);
        assert_eq!(body["source"], "tenant_worker_broker_registry");
        assert_eq!(body["consistency"], "snapshot_per_page");
        assert_eq!(body["runtime_inventory"], "unknown");
        assert_eq!(body["registry_status"], "available");
        assert!(
            chrono::DateTime::parse_from_rfc3339(body["evaluated_at"].as_str().unwrap()).is_ok()
        );
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), ids.len());
        for item in items {
            assert!(ids.iter().any(|id| item["worker"]["worker_id"] == *id));
            assert_eq!(item["observation_broker"]["status"], "authority_unknown");
            let association = &item["launch_broker"];
            if item["worker"]["launch"]["authority_id"] == AUTHORITY {
                assert_eq!(association["status"], "registered");
                assert_eq!(association["broker"]["broker_id"], BROKER);
                assert_eq!(association["broker"]["current_incarnation_id"], incarnation);
                assert_eq!(association["broker"]["state"], "unseen");
                assert_eq!(association["in_current_grant"], true);
                assert_eq!(association["runtime_matches"], true);
                assert!(association["broker"].get("credential_id").is_none());
            } else {
                assert_eq!(association["status"], "unregistered");
                assert!(association["broker"].is_null());
            }
        }
        assert!(body["next_cursor"].is_null());
    }
    assert_eq!(
        f.a.head().unwrap(),
        a_head,
        "technical reads do not invent audit mutations"
    );
    assert_eq!(f.b.head().unwrap(), b_head);
    // The old strict DTO is unchanged, including on its real detail route.
    let old = f
        .get(
            &format!("/api/v1/workers/{}", a1.worker_id),
            &"v".repeat(48),
        )
        .await;
    assert_eq!(old.status(), StatusCode::OK);
    assert_eq!(
        old.json::<WorkerV1>().await.unwrap().worker_id,
        a1.worker_id
    );
}

#[tokio::test]
async fn federation_preserves_removed_authority_ownership_and_bounds_scope_and_pages() {
    let f = Fixture::new().await;
    grant(&f.a, AUTHORITY, None);
    let selected = reserve(&f.a, AUTHORITY);
    let sibling = reserve(&f.a, UNREGISTERED);
    let current_incarnation = grant(&f.a, "replacement-authority", Some(1));
    f.a.revoke_broker(&BrokerRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        broker_id: BROKER.into(),
        expected_revision: 2,
    })
    .unwrap();
    let mut cursor = None;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let path = format!(
            "/api/v1/federation/workers?limit=1{}",
            cursor
                .as_ref()
                .map(|c| format!("&cursor={c}"))
                .unwrap_or_default()
        );
        let response = f.get(&path, &"v".repeat(48)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert!(seen.insert(item["worker"]["worker_id"].as_str().unwrap().to_owned()));
        if item["worker"]["worker_id"] == selected.worker_id {
            assert_eq!(item["launch_broker"]["status"], "registered");
            assert_eq!(item["launch_broker"]["in_current_grant"], false);
            assert_eq!(item["launch_broker"]["broker"]["state"], "revoked");
            assert_eq!(
                item["launch_broker"]["broker"]["current_incarnation_id"],
                current_incarnation
            );
        }
        cursor = body["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
        assert!(seen.len() < 3, "bounded keyset must make progress");
    }
    assert_eq!(
        seen,
        std::collections::BTreeSet::from([selected.worker_id, sibling.worker_id])
    );
    for suffix in [
        "?limit=0",
        "?limit=101",
        "?cursor=../other",
        "?tenant_id=tenant-b",
        "?authority_id=another-authority",
    ] {
        assert_eq!(
            f.get(
                &format!("/api/v1/federation/workers{suffix}"),
                &"v".repeat(48)
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        f.get("/api/v1/federation/workers", "unknown-proof")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.get("/api/v1/federation/workers", &"a".repeat(48))
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn federation_keeps_real_legacy_workers_with_explicit_registry_migration_limit() {
    let f = Fixture::with_legacy_a(true).await;
    let worker = reserve_with_protocol(&f.a, AUTHORITY, true);
    let head = f.a.head().unwrap();
    let old = f
        .get(
            &format!("/api/v1/workers/{}", worker.worker_id),
            &"v".repeat(48),
        )
        .await;
    assert_eq!(
        old.status(),
        StatusCode::OK,
        "legacy worker is a real durable control fact"
    );
    assert_eq!(
        old.json::<WorkerV1>().await.unwrap().worker_id,
        worker.worker_id
    );
    let response = f.get("/api/v1/federation/workers", &"v".repeat(48)).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "structural registry absence must preserve the worker view"
    );
    let body = wire_json(response, "legacy.json").await;
    assert_eq!(body["registry_status"], "migration_required");
    assert_eq!(body["runtime_inventory"], "unknown");
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["worker"]["worker_id"], worker.worker_id);
    for association in ["launch_broker", "observation_broker"] {
        assert_eq!(
            body["items"][0][association]["status"],
            "registry_unavailable"
        );
        assert!(body["items"][0][association]["broker"].is_null());
        assert!(body["items"][0][association]["in_current_grant"].is_null());
    }
    assert_eq!(
        f.a.head().unwrap(),
        head,
        "read does not migrate or fabricate an audit event"
    );
}
