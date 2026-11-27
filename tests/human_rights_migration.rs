//! Explicit human-rights capability marker, its backup and the capabilities it
//! admits, with real private SQLite stores and the operator CLI.
mod support;
use agentsandbox::{audit::Store, identity::*, redaction::Redactor};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    process::{Command, Stdio},
};
use uuid::Uuid;

const NEW_CAPABILITIES: [HumanCapabilityV1; 4] = [
    HumanCapabilityV1::ReadMemberships,
    HumanCapabilityV1::ReadConfiguration,
    HumanCapabilityV1::LaunchSession,
    HumanCapabilityV1::SendSessionMessage,
];

fn migrated(path: &Path, through_workers: bool) -> Store {
    let store = Store::open_for_tenant(path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    let steps: Vec<fn(&Store, &Path) -> anyhow::Result<agentsandbox::control::MigrationReportV1>> = vec![
        Store::migrate_control_v3,
        Store::migrate_identity_v4,
        Store::migrate_human_access_v5,
        Store::migrate_workload_v6,
        Store::migrate_human_brokers_v7,
        Store::migrate_kubernetes_create_v8,
        Store::migrate_kubernetes_policy_v9,
        Store::migrate_human_workers_v10,
        Store::migrate_kubernetes_session_reservations_v11,
        Store::migrate_broker_checkpoints_v12,
        Store::migrate_kubernetes_recovery_v13,
    ];
    let count = if through_workers { steps.len() } else { 7 };
    for (index, step) in steps.into_iter().take(count).enumerate() {
        step(&store, &path.with_extension(format!("before{}", index + 3))).unwrap();
    }
    store
}
fn control(change: IdentityChangeV1) -> IdentityMutationV1 {
    by(
        PrincipalRefV1::LegacyControl {
            credential_id: "human-rights-migration-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    )
}
fn by(actor: PrincipalRefV1, change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor,
        change,
    }
}
fn preparation(capabilities: &[HumanCapabilityV1]) -> (LocalHumanProvisionV1, MembershipV1) {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: Uuid::new_v4().to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let token = generate_local_credential().unwrap();
    let membership = MembershipV1 {
        version: 1,
        membership_id: Uuid::new_v4().to_string(),
        principal: principal.clone(),
        tenant_id: "tenant-a".into(),
        organization_scope: "organization-a".into(),
        revision: 1,
        active: true,
        capabilities: capabilities.iter().copied().collect(),
    };
    let provision = LocalHumanProvisionV1 {
        version: 1,
        mutations: [
            control(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Human rights reader".into(),
                    revision: 1,
                    active: true,
                },
            }),
            control(IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: LocalCredentialV1 {
                    version: 1,
                    credential_id: Uuid::new_v4().to_string(),
                    principal,
                    sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                    revision: 1,
                    active: true,
                    expires_at: None,
                },
            }),
            control(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: membership.clone(),
            }),
        ],
    };
    (provision, membership)
}
fn refusal(result: anyhow::Result<impl std::fmt::Debug>) -> Option<IdentityFailureV1> {
    result
        .unwrap_err()
        .downcast_ref::<IdentityFailureV1>()
        .copied()
}
fn migrate_cli(path: &Path, backup: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["migrate-human-rights", "--data"])
        .arg(path)
        .args(["--tenant-id", "tenant-a", "--backup"])
        .arg(backup)
        .args(["--confirm-control-stopped", "--max-events", "1000"])
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn new_capabilities_require_the_backed_up_marker_and_survive_reopen() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    let before = store.integrity().unwrap();
    assert!(!store.human_rights_installed().unwrap());
    for capability in NEW_CAPABILITIES {
        let (prepared, _) = preparation(&[capability]);
        assert_eq!(
            refusal(store.provision_local_human(&prepared)),
            Some(IdentityFailureV1::MigrationRequired),
            "{capability:?} must wait for the explicit marker"
        );
    }
    assert_eq!(store.integrity().unwrap().head_hash, before.head_hash);
    drop(store);

    let backup = files.path().join("before-rights.backup");
    let output = migrate_cli(&path, &backup);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["from_version"], 13);
    assert_eq!(
        report["to_version"], 13,
        "the marker never changes the format"
    );
    assert!(report["genesis_seq"].as_i64().unwrap() > before.event_count as i64);

    let upgraded =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(upgraded.supervision_state().unwrap().schema_version, 13);
    assert!(upgraded.human_rights_installed().unwrap());
    assert_eq!(
        upgraded.integrity().unwrap().event_count,
        before.event_count + 1
    );
    let mut granted = Vec::new();
    for capability in NEW_CAPABILITIES {
        let (prepared, membership) = preparation(&[capability]);
        upgraded.provision_local_human(&prepared).unwrap();
        granted.push(membership);
    }
    for membership in &granted {
        assert_eq!(
            upgraded
                .identity_memberships(&membership.principal)
                .unwrap(),
            vec![membership.clone()]
        );
    }
    let head = upgraded.head().unwrap();
    drop(upgraded);
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(reopened.head().unwrap(), head);
    assert!(
        reopened
            .migrate_human_rights_v2(&files.path().join("second.backup"))
            .is_err(),
        "the marker is installed once"
    );
    assert_eq!(reopened.head().unwrap(), head);
    drop(reopened);

    let saved =
        Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(saved.integrity().unwrap().head_hash, before.head_hash);
    assert!(!saved.human_rights_installed().unwrap());
    let (prepared, _) = preparation(&[HumanCapabilityV1::ReadMemberships]);
    assert_eq!(
        refusal(saved.provision_local_human(&prepared)),
        Some(IdentityFailureV1::MigrationRequired)
    );
}

#[test]
fn marker_requires_the_worker_registry_marker_and_leaves_the_store_unchanged() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, false);
    let before = store.integrity().unwrap();
    assert!(
        store
            .migrate_human_rights_v2(&files.path().join("schema9.backup"))
            .is_err()
    );
    assert_eq!(store.supervision_state().unwrap().schema_version, 9);
    assert_eq!(store.integrity().unwrap().head_hash, before.head_hash);
    assert!(!store.human_rights_installed().unwrap());
}

#[test]
fn legacy_session_prepare_capability_is_explicit_and_survives_reopen() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let backup = files.path().join("before-session-prepare.sqlite");
    let store = migrated(&path, false);
    store
        .migrate_human_workers_v10(&files.path().join("before-store10.sqlite"))
        .unwrap();
    let open = |path: &Path| {
        Store::open_existing_for_tenant(path, "tenant-a", Redactor::new(vec![]), 1000).unwrap()
    };
    let marker = agentsandbox::audit::EventQuery {
        kind: Some("control.legacy_session_prepare.genesis".into()),
        limit: Some(2),
        ..Default::default()
    };
    let enable = || {
        Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .arg("enable-legacy-session-prepare")
            .arg("--data")
            .arg(&path)
            .arg("--tenant-id")
            .arg("tenant-a")
            .arg("--backup")
            .arg(&backup)
            .arg("--max-events")
            .arg("1000")
            .arg("--confirm-control-stopped")
            .output()
            .unwrap()
    };
    let before_rights = store.head().unwrap();
    drop(store);
    assert!(!enable().status.success());
    assert!(!backup.exists());
    let store = open(&path);
    assert_eq!(store.head().unwrap(), before_rights);
    store
        .migrate_human_rights_v2(&files.path().join("before-rights.sqlite"))
        .unwrap();
    let before = store.integrity().unwrap();
    let quota = store
        .worker_quota(&agentsandbox::control::QuotaPolicyV1::default())
        .unwrap();
    drop(store);

    let enabled = enable();
    assert!(
        enabled.status.success(),
        "the explicit Legacy capability installation failed"
    );
    let report: agentsandbox::control::MigrationReportV1 =
        serde_json::from_slice(&enabled.stdout).unwrap();
    assert_eq!((report.from_version, report.to_version), (10, 10));
    let store = open(&path);
    assert!(store.integrity().unwrap().valid);
    let markers = store.events(&marker).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].seq, report.genesis_seq);
    assert_eq!(markers[0].hash, report.genesis_hash);
    assert_eq!(
        store.integrity().unwrap().event_count,
        before.event_count + 1
    );
    assert!(store.workers().unwrap().is_empty());
    let reopened_quota = store.worker_quota(&quota.policy).unwrap();
    assert_eq!(reopened_quota.held_workers, quota.held_workers);
    assert_eq!(reopened_quota.held_cpu_millis, quota.held_cpu_millis);
    assert_eq!(reopened_quota.held_memory_bytes, quota.held_memory_bytes);
    let head = store.integrity().unwrap().head_hash;
    assert_eq!(report.genesis_hash, head);
    drop(store);

    let saved = open(&backup);
    assert_eq!(saved.integrity().unwrap().head_hash, before.head_hash);
    assert!(saved.events(&marker).unwrap().is_empty());
    drop(saved);
    assert!(!enable().status.success());
    let store = open(&path);
    assert_eq!(store.integrity().unwrap().head_hash, head);
    store
        .migrate_kubernetes_session_reservations_v11(&files.path().join("before-store11.sqlite"))
        .unwrap();
    store
        .migrate_broker_checkpoints_v12(&files.path().join("before-store12.sqlite"))
        .unwrap();
    store
        .migrate_kubernetes_recovery_v13(&files.path().join("before-store13.sqlite"))
        .unwrap();
    drop(store);
    let upgraded = open(&path);
    assert_eq!(upgraded.supervision_state().unwrap().schema_version, 13);
    let markers = upgraded.events(&marker).unwrap();
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].hash, head);
    assert!(upgraded.integrity().unwrap().valid);
}

#[tokio::test]
async fn legacy_local_confirmation_survives_reopen_without_admission() {
    use agentsandbox::{
        api::{GatewayState, TenantGateway, gateway_router},
        broker_registry::{BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRuntimeKindV1},
        control::{
            AdmissionV1, DecisionPhaseV1, DecisionReceiptV1, LaunchSpecV1, SupervisedAdmissionV1,
            SupervisionProtocolV1,
        },
        model::DecisionRequest,
        policy::{self, Policy},
        workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
    };
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use std::sync::Arc;
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    use tower::ServiceExt;

    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let target_file = files.path().join("target.json");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&files.path().join("before-rights.sqlite"))
        .unwrap();
    store
        .enable_legacy_session_prepare_v1(&files.path().join("before-prepare.sqlite"))
        .unwrap();
    let (mut provision, membership) = preparation(&[HumanCapabilityV1::LaunchSession]);
    let credential = generate_local_credential().unwrap();
    let IdentityChangeV1::Credential { binding, .. } = &mut provision.mutations[1].change else {
        panic!("the fixture must retain its local credential");
    };
    binding.sha256_digest = hex::encode(Sha256::digest(credential.as_bytes()));
    store.provision_local_human(&provision).unwrap();
    let broker_id = Uuid::new_v4();
    let incarnation_id = Uuid::new_v4();
    let credential_id = Uuid::new_v4();
    let authority_id = Uuid::new_v4();
    let expires_at = "2100-01-01T00:00:00.000Z";
    store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential_id.to_string(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: broker_id.to_string(),
                incarnation_id: incarnation_id.to_string(),
            },
            sha256_digest: hex::encode(Sha256::digest(b"synthetic legacy prepare broker")),
            expires_at: expires_at.into(),
        })
        .unwrap();
    store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: broker_id.to_string(),
            expected_revision: None,
            credential_id: credential_id.to_string(),
            incarnation_id: incarnation_id.to_string(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: authority_id.to_string(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 120,
            not_after: expires_at.into(),
        })
        .unwrap();
    let before = store.head().unwrap();
    drop(store);

    let target_id = Uuid::new_v4();
    let mut input = serde_json::json!({
        "expected_revision": 0,
        "target": {
            "version": 1,
            "catalogue": {
                "version": 1, "launch_target_id": target_id, "revision": 1,
                "tenant_id": "tenant-a", "organization_scope": membership.organization_scope,
                "active": true, "launchers": [membership.principal],
            },
            "broker_id": broker_id, "incarnation_id": incarnation_id, "broker_grant_revision": 1,
            "broker_profile_id": "fixture-profile",
            "launch": {
                "version": 1, "session_id": target_id, "command": {"argv": ["/usr/bin/id", "-u"], "cwd": "/workspace"},
                "profile": "observe", "runtime_kind": "docker", "authority_id": authority_id,
                "image_ref": "worker:fixture", "image_id": format!("sha256:{}", "a".repeat(64)),
                "network": "none", "cpu_millis": 2000, "memory_bytes": 1073741824_u64, "pids_limit": 256,
                "timeout_seconds": 60, "security_preset": "fixed_sandbox_v2", "restart_policy": "no",
                "workspace_manifest_digest": "b".repeat(64), "workspace_import_policy_version": 1,
                "provider": "generic", "capture_output": false, "credential_binding_id": null, "credential_names": [],
            }
        }
    });
    let apply = |input: &serde_json::Value| {
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&target_file)
            .unwrap()
            .write_all(&serde_json::to_vec(input).unwrap())
            .unwrap();
        Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .args(["human", "provision-legacy-session-target", "--data"])
            .arg(&path)
            .args([
                "--tenant-id",
                "tenant-a",
                "--max-events",
                "1000",
                "--confirm-control-stopped",
                "--target-file",
            ])
            .arg(&target_file)
            .output()
            .unwrap()
    };
    let accepted = apply(&input);
    assert!(
        accepted.status.success(),
        "the offline target command refused the canonical fixture"
    );
    let receipt: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(receipt["target_id"], target_id.to_string());
    assert_eq!(receipt["replayed"], false);
    let replay = apply(&input);
    assert!(replay.status.success());
    let replay: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replay["audit_hash"], receipt["audit_hash"]);
    assert_eq!(replay["replayed"], true);
    let mut divergent = input.clone();
    divergent["target"]["broker_profile_id"] = "other-profile".into();
    assert!(!apply(&divergent).status.success());
    let reopen = || {
        Arc::new(
            Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000)
                .unwrap(),
        )
    };
    let reopened = reopen();
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(reopened.head().unwrap(), before + 1);
    assert!(reopened.workers().unwrap().is_empty());

    async fn call(
        router: &Router,
        path: &str,
        credential: &str,
        selected: Option<(&str, &serde_json::Value)>,
    ) -> (StatusCode, serde_json::Value) {
        let mut request = Request::builder()
            .uri(path)
            .header("authorization", format!("Bearer {credential}"));
        let body = if let Some((context, input)) = selected {
            request = request
                .method("POST")
                .header("x-aiguard-context", context)
                .header("content-type", "application/json");
            Body::from(serde_json::to_vec(input).unwrap())
        } else {
            Body::empty()
        };
        let response = router
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 65_536).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }
    let make_router = |store: Arc<Store>| {
        gateway_router(
            GatewayState::for_tenants(
                vec![TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store,
                    runner_token: "synthetic-g04-runner".repeat(3),
                    viewer_token: "synthetic-g04-viewer".repeat(3),
                }],
                agentsandbox::policy::Policy::default(),
                None,
                Redactor::new(vec![]),
            )
            .unwrap(),
        )
    };
    let add_unrelated_account = |store: &Store| {
        let before = store.identity_authorization_revision().unwrap();
        let principal = PrincipalRefV1::HumanLocal {
            installation_id: Uuid::new_v4().to_string(),
            account_id: Uuid::new_v4().to_string(),
        };
        let receipt = store
            .mutate_identity(&control(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Unrelated fixture account".into(),
                    revision: 1,
                    active: true,
                },
            }))
            .unwrap();
        assert_eq!(receipt.authorization_revision, before + 1);
        principal
    };
    let router = make_router(reopened.clone());
    let (status, who) = call(&router, "/api/v1/human/identity", &credential, None).await;
    assert_eq!(status, StatusCode::OK);
    let context = who["contexts"][0]["id"].as_str().unwrap();
    let prepare_op = Uuid::new_v4();
    let prepared = serde_json::json!({
        "version": 1, "operation_id": prepare_op, "launch_target_id": target_id, "expected_target_revision": 1,
        "expires_at": (chrono::Utc::now() + chrono::Duration::minutes(2)).to_rfc3339(),
    });
    let (status, prepare_receipt) = call(
        &router,
        "/api/v1/human/legacy/sessions/prepare",
        &credential,
        Some((context, &prepared)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the local Legacy Prepare route must retain the fixed target"
    );
    let confirm = serde_json::json!({
        "version": 1, "operation_id": Uuid::new_v4(), "launch_target_id": target_id, "expected_target_revision": 1,
        "preparation_operation_id": prepare_op, "preparation_input_hash": prepare_receipt["input_hash"],
    });
    add_unrelated_account(&reopened);
    let head = reopened.head().unwrap();
    for (path, input) in [
        ("/api/v1/human/legacy/sessions/prepare", &prepared),
        ("/api/v1/human/legacy/sessions/confirm", &confirm),
    ] {
        let (status, body) = call(&router, path, &credential, Some((context, input))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "context_revoked");
    }
    let (status, who) = call(&router, "/api/v1/human/identity", &credential, None).await;
    assert_eq!(status, StatusCode::OK);
    let context = who["contexts"][0]["id"].as_str().unwrap();
    let (status, mut replay) = call(
        &router,
        "/api/v1/human/legacy/sessions/prepare",
        &credential,
        Some((context, &prepared)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["replayed"], true);
    replay["replayed"] = false.into();
    assert_eq!(replay, prepare_receipt);
    assert_eq!(reopened.head().unwrap(), head);
    let (status, receipt) = call(
        &router,
        "/api/v1/human/legacy/sessions/confirm",
        &credential,
        Some((context, &confirm)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(receipt["replayed"], false);
    let session_id = receipt["session_id"].as_str().unwrap().to_owned();
    let binding_ref = receipt["binding_ref"].as_str().unwrap();
    assert!(Uuid::parse_str(binding_ref).is_ok());
    let mut launch: LaunchSpecV1 =
        serde_json::from_value(input["target"]["launch"].clone()).unwrap();
    launch.session_id = session_id.clone();
    let decision_request = DecisionRequest {
        request_id: Uuid::new_v4().to_string(),
        profile: launch.profile.clone(),
        command: launch.command.clone(),
        project_id: None,
        sandbox_id: Some(session_id.clone()),
        agent_id: None,
        session_id: Some(session_id.clone()),
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let decision = reopened
        .decision(
            &decision_request,
            Some((
                policy::evaluate(&Policy::default(), &decision_request),
                serde_json::Value::Null,
            )),
        )
        .unwrap()
        .unwrap();
    let manual = SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: "d".repeat(64),
        admission: AdmissionV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            launch,
            receipt: DecisionReceiptV1 {
                version: 1,
                request_id: decision_request.request_id.clone(),
                request_hash: policy::request_hash(&decision_request),
                phase: DecisionPhaseV1::Initial,
                audit_seq: decision.seq,
                audit_hash: decision.hash,
            },
            decision_request,
        },
    };
    manual.validate().unwrap();
    let head = reopened.head().unwrap();
    for (path, payload) in [
        (
            "/control/api/v1/workers/reservations",
            serde_json::to_vec(&manual.admission).unwrap(),
        ),
        (
            "/control/api/v1/workers/supervised-reservations",
            serde_json::to_vec(&manual).unwrap(),
        ),
    ] {
        let request = Request::post(path)
            .header(
                "authorization",
                format!("Bearer {}", "synthetic-g04-runner".repeat(3)),
            )
            .header("content-type", "application/json")
            .body(Body::from(payload))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }
    assert_eq!(reopened.head().unwrap(), head);
    drop((router, reopened));
    let reopened = reopen();
    let other_principal = add_unrelated_account(&reopened);
    let router = make_router(reopened.clone());
    let (status, who) = call(&router, "/api/v1/human/identity", &credential, None).await;
    assert_eq!(status, StatusCode::OK);
    let head = reopened.head().unwrap();
    let context = who["contexts"][0]["id"].as_str().unwrap();
    let (status, mut replay) = call(
        &router,
        "/api/v1/human/legacy/sessions/confirm",
        &credential,
        Some((context, &confirm)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["replayed"], true);
    replay["replayed"] = false.into();
    assert_eq!(replay, receipt);
    assert!(reopened.workers().unwrap().is_empty());
    assert_eq!(
        reopened
            .worker_quota(&agentsandbox::control::QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
    assert!(
        reopened
            .identity_resource_grant(&ResourceRefV1 {
                kind: ResourceKindV1::Session,
                resource_id: session_id
            })
            .unwrap()
            .is_none()
    );
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(reopened.head().unwrap(), head);

    let authorization_revision = reopened.identity_authorization_revision().unwrap();
    drop((router, reopened));
    let original_launchers = input["target"]["catalogue"]["launchers"].clone();
    input["expected_revision"] = 1.into();
    input["target"]["catalogue"]["revision"] = 2.into();
    input["target"]["catalogue"]["launchers"] = serde_json::json!([other_principal]);
    assert!(
        apply(&input).status.success(),
        "replace only the target ACL"
    );

    let reopened = reopen();
    assert_eq!(
        reopened.identity_authorization_revision().unwrap(),
        authorization_revision
    );
    let router = make_router(reopened.clone());
    let head = reopened.integrity().unwrap().head_hash;
    let (status, who) = call(&router, "/api/v1/human/identity", &credential, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        who["contexts"][0]["capabilities"],
        serde_json::json!(["launch_session"])
    );
    let context = who["contexts"][0]["id"].as_str().unwrap();
    for (path, input) in [
        ("/api/v1/human/legacy/sessions/prepare", &prepared),
        ("/api/v1/human/legacy/sessions/confirm", &confirm),
    ] {
        let (status, body) = call(&router, path, &credential, Some((context, input))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "permission_denied");
    }
    assert_eq!(reopened.integrity().unwrap().head_hash, head);

    drop((router, reopened));
    input["expected_revision"] = 2.into();
    input["target"]["catalogue"]["revision"] = 3.into();
    input["target"]["catalogue"]["launchers"] = original_launchers;
    assert!(apply(&input).status.success(), "restore the target ACL");
    let reopened = reopen();
    let router = make_router(reopened.clone());
    let head = reopened.head().unwrap();
    let (status, who) = call(&router, "/api/v1/human/identity", &credential, None).await;
    assert_eq!(status, StatusCode::OK);
    let context = who["contexts"][0]["id"].as_str().unwrap();
    for (path, input, original) in [
        (
            "/api/v1/human/legacy/sessions/prepare",
            &prepared,
            &prepare_receipt,
        ),
        ("/api/v1/human/legacy/sessions/confirm", &confirm, &receipt),
    ] {
        let (status, mut body) = call(&router, path, &credential, Some((context, input))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["replayed"], true);
        body["replayed"] = false.into();
        assert_eq!(&body, original);
    }
    assert_eq!(reopened.head().unwrap(), head);

    let mut restricted = membership;
    restricted.revision += 1;
    restricted.capabilities = [HumanCapabilityV1::ReadMetadata].into_iter().collect();
    reopened
        .mutate_identity(&control(IdentityChangeV1::Membership {
            expected_revision: restricted.revision - 1,
            membership: restricted,
        }))
        .unwrap();
    let head = reopened.head().unwrap();
    let (status, who) = call(&router, "/api/v1/human/identity", &credential, None).await;
    assert_eq!(status, StatusCode::OK);
    let context = who["contexts"][0]["id"].as_str().unwrap();
    for (path, input) in [
        ("/api/v1/human/legacy/sessions/prepare", &prepared),
        ("/api/v1/human/legacy/sessions/confirm", &confirm),
    ] {
        let (status, body) = call(&router, path, &credential, Some((context, input))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "permission_denied");
    }
    assert_eq!(reopened.head().unwrap(), head);
}

/// A broad property: the downcast store also fails the schema checks of format 9.
/// The marker checks themselves are proven against forged markers in
/// src/audit/human_rights_tests.rs.
#[test]
fn a_schema_downcast_cannot_hide_the_human_rights_marker() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 9).unwrap();
    drop(db);
    assert!(
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).is_err(),
        "the human rights marker is invalid on a store without worker access"
    );
}

#[test]
fn organization_administrators_cannot_introduce_control_only_capabilities() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    // The administrator holds what it delegates: delegation never escalates.
    let (admin, admin_membership) = preparation(&[
        HumanCapabilityV1::ManageMemberships,
        HumanCapabilityV1::ReadMetadata,
        HumanCapabilityV1::ReadContent,
    ]);
    store.provision_local_human(&admin).unwrap();
    let (member, member_membership) = preparation(&[HumanCapabilityV1::ReadMetadata]);
    store.provision_local_human(&member).unwrap();
    let grant = |capabilities: &[HumanCapabilityV1], revision: u64| {
        by(
            admin_membership.principal.clone(),
            IdentityChangeV1::Membership {
                expected_revision: revision,
                membership: MembershipV1 {
                    revision: revision + 1,
                    capabilities: capabilities.iter().copied().collect(),
                    ..member_membership.clone()
                },
            },
        )
    };
    for capability in NEW_CAPABILITIES {
        let head = store.head().unwrap();
        assert_eq!(
            refusal(
                store.mutate_identity(&grant(&[HumanCapabilityV1::ReadMetadata, capability], 1))
            ),
            Some(IdentityFailureV1::PermissionDenied),
            "an organization administrator must not grant {capability:?}"
        );
        assert_eq!(store.head().unwrap(), head);
    }
    // The existing organization delegation is unchanged by the fence.
    store
        .mutate_identity(&grant(
            &[
                HumanCapabilityV1::ReadMetadata,
                HumanCapabilityV1::ReadContent,
            ],
            1,
        ))
        .unwrap();
    // Trusted control may still introduce a control-only capability.
    store
        .mutate_identity(&control(IdentityChangeV1::Membership {
            expected_revision: 2,
            membership: MembershipV1 {
                revision: 3,
                capabilities: [
                    HumanCapabilityV1::ReadMetadata,
                    HumanCapabilityV1::ReadContent,
                    HumanCapabilityV1::LaunchSession,
                ]
                .into_iter()
                .collect(),
                ..member_membership.clone()
            },
        }))
        .unwrap();
}

fn membership_change(
    actor: Option<&PrincipalRefV1>,
    membership: &MembershipV1,
    capabilities: &[HumanCapabilityV1],
    active: bool,
) -> IdentityMutationV1 {
    let change = IdentityChangeV1::Membership {
        expected_revision: membership.revision,
        membership: MembershipV1 {
            revision: membership.revision + 1,
            active,
            capabilities: capabilities.iter().copied().collect(),
            ..membership.clone()
        },
    };
    match actor {
        Some(actor) => by(actor.clone(), change),
        None => control(change),
    }
}
fn next(
    membership: &MembershipV1,
    capabilities: &[HumanCapabilityV1],
    active: bool,
) -> MembershipV1 {
    MembershipV1 {
        revision: membership.revision + 1,
        active,
        capabilities: capabilities.iter().copied().collect(),
        ..membership.clone()
    }
}

#[test]
fn a_mutation_cannot_introduce_a_fenced_capability_before_the_marker() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    let (member, membership) = preparation(&[HumanCapabilityV1::ReadMetadata]);
    store.provision_local_human(&member).unwrap();
    let head = store.head().unwrap();
    for capability in NEW_CAPABILITIES.into_iter().chain([
        HumanCapabilityV1::ApproveEgress,
        HumanCapabilityV1::ApproveLaunch,
    ]) {
        assert_eq!(
            refusal(store.mutate_identity(&membership_change(
                None,
                &membership,
                &[HumanCapabilityV1::ReadMetadata, capability],
                true
            ))),
            Some(IdentityFailureV1::MigrationRequired),
            "trusted control cannot add {capability:?} before the marker either"
        );
        assert_eq!(store.head().unwrap(), head);
    }
}

#[test]
fn approve_launch_enters_a_legacy_history_only_after_the_marker() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    let (approver, _) = preparation(&[
        HumanCapabilityV1::ReadMetadata,
        HumanCapabilityV1::ApproveLaunch,
    ]);
    assert_eq!(
        refusal(store.provision_local_human(&approver)),
        Some(IdentityFailureV1::MigrationRequired)
    );
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    store.provision_local_human(&approver).unwrap();
    let head = store.head().unwrap();
    drop(store);
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(reopened.head().unwrap(), head);
}

#[test]
fn control_only_capabilities_are_kept_or_removed_never_reintroduced_by_an_administrator() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    let (admin, admin_membership) = preparation(&[
        HumanCapabilityV1::ManageMemberships,
        HumanCapabilityV1::ReadMetadata,
        HumanCapabilityV1::ReadContent,
    ]);
    store.provision_local_human(&admin).unwrap();
    let (member, member_membership) = preparation(&[
        HumanCapabilityV1::ReadMetadata,
        HumanCapabilityV1::LaunchSession,
    ]);
    store.provision_local_human(&member).unwrap();
    let actor = Some(&admin_membership.principal);
    use HumanCapabilityV1 as C;
    // Keep: the existing control-only right survives another change.
    store
        .mutate_identity(&membership_change(
            actor,
            &member_membership,
            &[C::ReadMetadata, C::ReadContent, C::LaunchSession],
            true,
        ))
        .unwrap();
    let kept = next(
        &member_membership,
        &[C::ReadMetadata, C::ReadContent, C::LaunchSession],
        true,
    );
    // Remove: allowed.
    store
        .mutate_identity(&membership_change(actor, &kept, &[C::ReadMetadata], true))
        .unwrap();
    let removed = next(&kept, &[C::ReadMetadata], true);
    // Re-add after removal: refused.
    let head = store.head().unwrap();
    assert_eq!(
        refusal(store.mutate_identity(&membership_change(
            actor,
            &removed,
            &[C::ReadMetadata, C::LaunchSession],
            true
        ))),
        Some(IdentityFailureV1::PermissionDenied)
    );
    assert_eq!(store.head().unwrap(), head);
    // Reactivation of an inactive membership that holds it: refused.
    store
        .mutate_identity(&membership_change(
            None,
            &removed,
            &[C::ReadMetadata, C::LaunchSession],
            false,
        ))
        .unwrap();
    let inactive = next(&removed, &[C::ReadMetadata, C::LaunchSession], false);
    assert_eq!(
        refusal(store.mutate_identity(&membership_change(
            actor,
            &inactive,
            &[C::ReadMetadata, C::LaunchSession],
            true
        ))),
        Some(IdentityFailureV1::PermissionDenied),
        "reactivating a control-only right is introducing it again"
    );
    // Self-grant of a new capability: refused.
    assert_eq!(
        refusal(store.mutate_identity(&membership_change(
            actor,
            &admin_membership,
            &[
                C::ManageMemberships,
                C::ReadMetadata,
                C::ReadContent,
                C::ReadMemberships
            ],
            true
        ))),
        Some(IdentityFailureV1::PermissionDenied)
    );
    drop(store);
    Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
}

fn known_session(store: &Store, session: &str) {
    let mut event = agentsandbox::model::Event::new("fixture.authorized");
    event.session_id = Some(session.into());
    store.append(event).unwrap();
}
fn session_grant(
    owner: &PrincipalRefV1,
    session: &str,
    shares: Vec<ResourceShareV1>,
    revision: u64,
) -> ResourceGrantV1 {
    ResourceGrantV1 {
        version: 1,
        tenant_id: "tenant-a".into(),
        organization_scope: "organization-a".into(),
        resource: ResourceRefV1 {
            kind: ResourceKindV1::Session,
            resource_id: session.into(),
        },
        owner: owner.clone(),
        revision,
        active: true,
        shares,
    }
}

#[test]
fn launch_grant_does_not_revoke_unrelated_contexts() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    let (a, a_membership) = preparation(&[HumanCapabilityV1::ReadMetadata]);
    store.provision_local_human(&a).unwrap();
    let (b, b_membership) = preparation(&[HumanCapabilityV1::ReadMetadata]);
    store.provision_local_human(&b).unwrap();
    let revision = store.identity_authorization_revision().unwrap();
    let scope = store
        .authorize_human_context(
            &b_membership.principal,
            &b_membership.membership_id,
            b_membership.revision,
            ContextModeV1::Personal,
        )
        .unwrap();
    known_session(&store, "session-launched-by-a");
    store
        .mutate_identity(&control(IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: session_grant(&a_membership.principal, "session-launched-by-a", vec![], 1),
        }))
        .unwrap();
    assert_eq!(
        store.identity_authorization_revision().unwrap(),
        revision,
        "a first binding only adds rights"
    );
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "session-launched-by-a".into(),
    };
    assert!(
        store
            .authorized_resource_grant(&scope, &resource, ResourcePermissionV1::Metadata)
            .unwrap()
            .is_none(),
        "B's context is still current and still does not see A's session"
    );
    drop(store);
    Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
}

#[test]
fn grant_reduction_still_revokes_contexts() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    let (a, a_membership) = preparation(&[HumanCapabilityV1::ReadMetadata]);
    store.provision_local_human(&a).unwrap();
    let (b, b_membership) = preparation(&[HumanCapabilityV1::ReadMetadata]);
    store.provision_local_human(&b).unwrap();
    known_session(&store, "session-shared-then-withdrawn");
    let shared = session_grant(
        &a_membership.principal,
        "session-shared-then-withdrawn",
        vec![ResourceShareV1 {
            principal: b_membership.principal.clone(),
            permissions: [ResourcePermissionV1::Metadata].into_iter().collect(),
        }],
        1,
    );
    store
        .mutate_identity(&control(IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: shared.clone(),
        }))
        .unwrap();
    let scope = store
        .authorize_human_context(
            &b_membership.principal,
            &b_membership.membership_id,
            b_membership.revision,
            ContextModeV1::Personal,
        )
        .unwrap();
    let resource = shared.resource.clone();
    assert!(
        store
            .authorized_resource_grant(&scope, &resource, ResourcePermissionV1::Metadata)
            .unwrap()
            .is_some()
    );
    let revision = store.identity_authorization_revision().unwrap();
    store
        .mutate_identity(&control(IdentityChangeV1::ResourceGrant {
            expected_revision: 1,
            grant: ResourceGrantV1 {
                revision: 2,
                shares: vec![],
                ..shared
            },
        }))
        .unwrap();
    assert_eq!(
        store.identity_authorization_revision().unwrap(),
        revision + 1
    );
    assert_eq!(
        refusal(store.authorized_resource_grant(&scope, &resource, ResourcePermissionV1::Metadata)),
        Some(IdentityFailureV1::ContextRevoked),
        "a context issued before the withdrawal no longer reads"
    );
}

fn cli_error(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn the_operator_maintenance_requires_its_attestation_and_explains_each_refusal() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let head = migrated(&path, true).head().unwrap();
    let unattested = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["migrate-human-rights", "--data"])
        .arg(&path)
        .args(["--tenant-id", "tenant-a", "--backup"])
        .arg(files.path().join("unattested.backup"))
        .args(["--max-events", "1000"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!unattested.status.success());
    assert!(!files.path().join("unattested.backup").exists());
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(reopened.head().unwrap(), head);
    assert!(!reopened.human_rights_installed().unwrap());
    drop(reopened);

    assert!(
        migrate_cli(&path, &files.path().join("first.backup"))
            .status
            .success()
    );
    let again = migrate_cli(&path, &files.path().join("second.backup"));
    assert!(!again.status.success());
    assert!(
        cli_error(&again).contains("already installed"),
        "{}",
        cli_error(&again)
    );

    let old_path = files.path().join("schema9.sqlite");
    drop(migrated(&old_path, false));
    let old = migrate_cli(&old_path, &files.path().join("schema9.backup"));
    assert!(!old.status.success());
    assert!(
        cli_error(&old).contains("formats 10 to 13"),
        "{}",
        cli_error(&old)
    );
}

#[test]
fn approval_rights_pass_only_from_a_holder() {
    use HumanCapabilityV1 as C;
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    let (plain, plain_membership) = preparation(&[C::ManageMemberships, C::ReadMetadata]);
    store.provision_local_human(&plain).unwrap();
    let (holder, holder_membership) = preparation(&[
        C::ManageMemberships,
        C::ReadMetadata,
        C::ApproveLaunch,
        C::ApproveEgress,
    ]);
    store.provision_local_human(&holder).unwrap();
    let (member, mut member_membership) = preparation(&[C::ReadMetadata]);
    store.provision_local_human(&member).unwrap();
    let plain_actor = Some(&plain_membership.principal);
    let holder_actor = Some(&holder_membership.principal);
    for capability in [C::ApproveLaunch, C::ApproveEgress] {
        let head = store.head().unwrap();
        assert_eq!(
            refusal(store.mutate_identity(&membership_change(
                plain_actor,
                &member_membership,
                &[C::ReadMetadata, capability],
                true
            ))),
            Some(IdentityFailureV1::PermissionDenied),
            "a non-holder never passes {capability:?} to another member"
        );
        assert_eq!(
            refusal(store.mutate_identity(&membership_change(
                plain_actor,
                &plain_membership,
                &[C::ManageMemberships, C::ReadMetadata, capability],
                true
            ))),
            Some(IdentityFailureV1::PermissionDenied),
            "a non-holder never grants {capability:?} to itself"
        );
        assert_eq!(store.head().unwrap(), head);
        let mut granted = member_membership.capabilities.clone();
        granted.insert(capability);
        let granted: Vec<C> = granted.into_iter().collect();
        store
            .mutate_identity(&membership_change(
                holder_actor,
                &member_membership,
                &granted,
                true,
            ))
            .unwrap_or_else(|error| panic!("a holder passes {capability:?}: {error:#}"));
        member_membership = next(&member_membership, &granted, true);
    }
    // Trusted control removes a right that the former holder cannot restore.
    store
        .mutate_identity(&membership_change(
            None,
            &holder_membership,
            &[C::ManageMemberships, C::ReadMetadata, C::ApproveLaunch],
            true,
        ))
        .unwrap();
    let removed = next(
        &holder_membership,
        &[C::ManageMemberships, C::ReadMetadata, C::ApproveLaunch],
        true,
    );
    assert_eq!(
        refusal(store.mutate_identity(&membership_change(
            holder_actor,
            &removed,
            &[
                C::ManageMemberships,
                C::ReadMetadata,
                C::ApproveLaunch,
                C::ApproveEgress
            ],
            true
        ))),
        Some(IdentityFailureV1::PermissionDenied),
        "approve_egress is not self-reactivated after removal"
    );
    drop(store);
    // Every accepted change replays at reopen.
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(reopened.healthy());
    assert_eq!(
        reopened.identity_memberships(&removed.principal).unwrap(),
        vec![removed]
    );
}

#[test]
fn rebinding_a_tombstoned_resource_advances_the_revision() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = migrated(&path, true);
    store
        .migrate_human_rights_v2(&path.with_extension("exact13"))
        .unwrap();
    let (a, a_membership) = preparation(&[HumanCapabilityV1::ReadMetadata]);
    store.provision_local_human(&a).unwrap();
    known_session(&store, "session-bound-twice");
    let bound = session_grant(&a_membership.principal, "session-bound-twice", vec![], 1);
    store
        .mutate_identity(&control(IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: bound.clone(),
        }))
        .unwrap();
    let first = store.identity_authorization_revision().unwrap();
    store
        .mutate_identity(&control(IdentityChangeV1::ResourceGrant {
            expected_revision: 1,
            grant: ResourceGrantV1 {
                revision: 2,
                active: false,
                ..bound.clone()
            },
        }))
        .unwrap();
    let tombstoned = store.identity_authorization_revision().unwrap();
    assert_eq!(tombstoned, first + 1, "a tombstone withdraws rights");
    store
        .mutate_identity(&control(IdentityChangeV1::ResourceGrant {
            expected_revision: 2,
            grant: ResourceGrantV1 {
                revision: 3,
                ..bound
            },
        }))
        .unwrap();
    assert_eq!(
        store.identity_authorization_revision().unwrap(),
        tombstoned + 1,
        "only a first binding keeps the revision; a re-binding is a new authorization"
    );
    drop(store);
    Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
}
