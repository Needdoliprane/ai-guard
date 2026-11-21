//! Exercise the native R2 operator CLI against the real broker API and Store.
use super::{CLUSTER_UID, NAMESPACE_UID, command, private_write, run, support};
use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker_registry::{BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRuntimeKindV1},
    control::{
        MigrationReportV1, QuotaPolicyV1,
        kubernetes_session::{
            KubernetesSessionEffectFenceV2, KubernetesSessionReservationApprovalV2,
            KubernetesSessionReservationCloseReasonV2, KubernetesSessionReservationIntentV2,
            KubernetesSessionReservationPolicyV2, KubernetesSessionReservationStateDataV2,
            KubernetesSessionReservationStateV2,
        },
    },
    policy::Policy,
    provider::{
        AuthMode, BOOTSTRAP_ENV, BOOTSTRAP_PROGRAM, ChoiceSource, ClientKind, ClientTransport,
        ProviderChoiceProvenanceV1, ProviderResolutionV1,
    },
    redaction::Redactor,
    runtime::kubernetes::{
        KubernetesAuthorityV1,
        session_profile::{
            KubernetesSessionBindingV2, KubernetesSessionIdsV2, KubernetesSessionInvocationV2,
            KubernetesSessionProfileV2, KubernetesSessionResourcesV2,
        },
    },
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
    workspace::Manifest,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{io::ErrorKind, path::Path, sync::Arc};
use tokio::{net::TcpListener, task::JoinSet};
use uuid::Uuid;

const TENANT: &str = "tenant-a";
const SECRET: &str = "public-session-r2-fixture-broker-token-00000000000000000";

fn fixture_store(root: &Path) -> Store {
    type Migration = fn(&Store, &Path) -> anyhow::Result<MigrationReportV1>;
    let store = Store::open_for_tenant(
        &root.join("tenant.sqlite"),
        TENANT,
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let migrations: [Migration; 9] = [
        Store::migrate_control_v3,
        Store::migrate_identity_v4,
        Store::migrate_human_access_v5,
        Store::migrate_workload_v6,
        Store::migrate_human_brokers_v7,
        Store::migrate_kubernetes_create_v8,
        Store::migrate_kubernetes_policy_v9,
        Store::migrate_human_workers_v10,
        Store::migrate_kubernetes_session_reservations_v11,
    ];
    for (before, migrate) in (2..11).zip(migrations) {
        assert_eq!(
            migrate(&store, &root.join(format!("before-{before}.sqlite")))
                .unwrap()
                .to_version,
            before + 1,
        );
    }
    store
}

fn session_profile(
    authority: &KubernetesAuthorityV1,
    ids: KubernetesSessionIdsV2,
    image: &str,
) -> KubernetesSessionProfileV2 {
    let binding = KubernetesSessionBindingV2::new(authority, ids).unwrap();
    let provider = ProviderResolutionV1 {
        version: 1,
        client: ClientKind::Codex,
        auth_mode: AuthMode::Api,
        transport: ClientTransport::Batch,
        profile_id: "default".into(),
        profile_revision: 1,
        model: None,
        credential_binding_id: Uuid::new_v4(),
        credential_names: vec![BOOTSTRAP_ENV.into()],
        invocation_preset_version: 1,
        provenance: ProviderChoiceProvenanceV1 {
            client: ChoiceSource::Cli,
            auth_mode: ChoiceSource::Cli,
            profile: ChoiceSource::Cli,
            model: ChoiceSource::Cli,
        },
    };
    let args = [
        "exec",
        "--client",
        "codex",
        "--auth-mode",
        "api",
        "--transport",
        "batch",
        "--preset-version",
        "1",
        "--prompt",
        "public session R2 fixture",
    ]
    .map(str::to_owned);
    KubernetesSessionProfileV2::build(
        authority,
        &binding,
        image,
        KubernetesSessionResourcesV2 {
            cpu_millis: 500,
            memory_mib: 128,
            temporary_mib: 8,
            home_mib: 8,
            deadline_seconds: 60,
        },
        &provider,
        KubernetesSessionInvocationV2::new(BOOTSTRAP_PROGRAM, &args).unwrap(),
        &Manifest::new(),
    )
    .unwrap()
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct SessionReport {
    version: u32,
    reservation_id: String,
    local_phase: String,
    // The wire includes this field even when the operation reports null.
    #[serde(deserialize_with = "Deserialize::deserialize")]
    central_state: Option<KubernetesSessionReservationStateDataV2>,
    execution_available: bool,
    runtime_state: String,
}

async fn invoke(root: &Path, action: &str, reservation_id: &str) -> SessionReport {
    let mut command = command();
    command
        .args(["kubernetes", "session", action, "--control-file"])
        .arg(root.join("control.json"))
        .arg("--journal-dir")
        .arg(root.join("journal"));
    if action == "prepare" {
        command.arg("--intent-file").arg(root.join("intent.json"));
    }
    let output = run(command, root).await;
    assert!(
        output.status.success(),
        "session {action} exit {:?}",
        output.status.code()
    );
    assert!(output.stderr.is_empty(), "session {action} emitted stderr");
    assert!(output.stdout.ends_with(b"\n") && output.stdout.len() <= 8193);
    assert!(
        !output
            .stdout
            .windows(SECRET.len())
            .any(|bytes| bytes == SECRET.as_bytes())
    );
    // Deserialize one complete object with no trailing output or unknown fields.
    let report: SessionReport = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report.version, 2);
    assert_eq!(report.reservation_id, reservation_id);
    assert!(!report.execution_available);
    assert_eq!(report.runtime_state, "not_observed");
    report
}

pub(super) async fn exercise() {
    let directory = support::private_tempdir();
    let root = directory.path().canonicalize().unwrap();
    let store = Arc::new(fixture_store(&root));
    let control_namespace_uuid: Uuid = store
        .supervision_state()
        .unwrap()
        .control_namespace_uuid
        .unwrap()
        .parse()
        .unwrap();
    let broker_id = Uuid::new_v4();
    let incarnation_id = Uuid::new_v4();
    let credential_id = Uuid::new_v4();
    let expires = "2100-01-01T00:00:00.000Z";
    // Hold an unused endpoint of our own so any attempted Kubernetes transport
    // stays inside the test and is detectable. This listener serves no requests.
    let api_canary = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    api_canary.set_nonblocking(true).unwrap();
    let authority = KubernetesAuthorityV1::new(
        Uuid::new_v4(),
        &format!("https://{}", api_canary.local_addr().unwrap()),
        CLUSTER_UID,
        TENANT,
        NAMESPACE_UID,
        &"a".repeat(64),
    )
    .unwrap();
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
            sha256_digest: hex::encode(Sha256::digest(SECRET)),
            expires_at: expires.into(),
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
                authority_id: authority.authority_id().to_string(),
                runtime_kind: BrokerRuntimeKindV1::Kubernetes,
            }],
            lease_seconds: 120,
            not_after: expires.into(),
        })
        .unwrap();
    let image = format!("worker@sha256:{}", "b".repeat(64));
    let profile = session_profile(
        &authority,
        KubernetesSessionIdsV2 {
            installation_id: Uuid::new_v4(),
            control_namespace_uuid,
            broker_id,
            incarnation_id,
            session_id: Uuid::new_v4(),
            attempt_id: Uuid::new_v4(),
        },
        &image,
    );
    let intent = KubernetesSessionReservationIntentV2::new(
        &Uuid::new_v4().to_string(),
        &Uuid::new_v4().to_string(),
        &authority,
        &profile,
    )
    .unwrap();
    let quota = QuotaPolicyV1::default();
    let policy = KubernetesSessionReservationPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        approvals: vec![KubernetesSessionReservationApprovalV2 {
            authority_id: authority.authority_id().to_string(),
            authority_fingerprint: authority.fingerprint(),
            image,
            client: ClientKind::Codex,
            auth_mode: AuthMode::Api,
            transport: ClientTransport::Batch,
            profile_id: "default".into(),
            profile_revision: 1,
        }],
    };
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: TENANT.into(),
            store: store.clone(),
            runner_token: "r".repeat(48),
            viewer_token: "v".repeat(48),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![SECRET.into()]),
    )
    .unwrap()
    .with_kubernetes_session_reservation_policies(vec![(TENANT.into(), policy)])
    .unwrap();
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let gateway = format!("http://{}", listener.local_addr().unwrap());
    let mut server = JoinSet::new();
    let router = gateway_router(state);
    server.spawn(async move { axum::serve(listener, router).await.unwrap() });

    let token = root.join("broker-token");
    private_write(&token, SECRET.as_bytes());
    private_write(
        &root.join("intent.json"),
        &serde_json::to_vec(&intent).unwrap(),
    );
    private_write(&root.join("control.json"), &serde_json::to_vec(&json!({
        "version":2,"gateway":gateway,"tenant_id":TENANT,"control_namespace_uuid":control_namespace_uuid,
        "broker_id":broker_id,"incarnation_id":incarnation_id,"credential_id":credential_id,
        "broker_token_file":token
    })).unwrap());

    let id = intent.reservation_id();
    let fence = KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 };
    let initial_head = store.head().unwrap();
    let prepared = invoke(&root, "prepare", id).await;
    assert_eq!(prepared.local_phase, "prepared");
    assert!(prepared.central_state.is_none());
    assert_eq!(store.worker_quota(&quota).unwrap().held_workers, 1);
    let reserved_head = store.head().unwrap();
    assert_eq!(reserved_head, initial_head + 1);

    let status = invoke(&root, "status", id).await;
    assert_eq!(status.local_phase, "prepared");
    assert_eq!(
        status.central_state.unwrap(),
        KubernetesSessionReservationStateDataV2 {
            state: KubernetesSessionReservationStateV2::Reserved,
            revision: 1,
            quota_held: true,
            effect_fence: fence.clone(),
            last_close: None,
        }
    );
    assert_eq!(store.head().unwrap(), reserved_head);

    let abandoned = invoke(&root, "abandon", id).await;
    assert_eq!(abandoned.local_phase, "abandoned");
    let closed = abandoned.central_state.as_ref().unwrap();
    assert_eq!(
        closed.state,
        KubernetesSessionReservationStateV2::ClosedBeforeEffect
    );
    assert_eq!(closed.revision, 2);
    assert!(!closed.quota_held);
    assert_eq!(closed.effect_fence, fence);
    let close = closed.last_close.as_ref().unwrap();
    assert_eq!(
        close.reason,
        KubernetesSessionReservationCloseReasonV2::PreparationAbandoned
    );
    assert_eq!(close.reservation_id, id);
    assert_eq!(
        close.reservation_input_hash,
        intent.input_hash(TENANT).unwrap()
    );
    assert_eq!(close.expected_revision, 1);
    assert!(!Uuid::parse_str(&close.operation_id).unwrap().is_nil());
    let closed_head = store.head().unwrap();
    assert_eq!(closed_head, reserved_head + 1);
    assert_eq!(invoke(&root, "abandon", id).await, abandoned);
    assert_eq!(invoke(&root, "status", id).await, abandoned);
    assert_eq!(store.head().unwrap(), closed_head);
    assert_eq!(store.worker_quota(&quota).unwrap().held_workers, 0);
    assert!(store.worker_usage().unwrap().is_empty());
    assert!(store.integrity().unwrap().valid);
    assert_eq!(
        api_canary.accept().unwrap_err().kind(),
        ErrorKind::WouldBlock
    );
    server.shutdown().await;
}
