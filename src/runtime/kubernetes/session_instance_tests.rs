//! Pure projections and a loopback TLS name inspection. Pod and PVC bodies are
//! declared fixtures: they never qualify Kubernetes, a provisioner or RWOP.
use super::*;
use crate::{
    control::kubernetes_session::KubernetesSessionReservationIntentV2,
    control_fs::PrivateFile,
    runtime::kubernetes::{
        KubernetesCredentials,
        admission::transport_tests::TlsMaterial,
        session::tests::fixture_at,
        session_pod::{
            session_workspace_claim_name,
            tests::{POD_UID, admitted, collect_keys, rendered_case, rendered_case_with, replace},
        },
    },
    tls::{ServerIdentity, serve},
};
use axum::{Router, body::Body, extract::State, http::Request, response::Response};
use std::{
    collections::BTreeSet,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;

pub(crate) const CLAIM_UID: &str = "55555555-5555-4555-8555-555555555555";
pub(crate) const OBSERVED_AT: &str = "2026-11-12T08:00:00.000Z";
const OTHER_UID: &str = "66666666-6666-4666-8666-666666666666";
const CONTAINER_ID: &str = "containerd://0123456789abcdef";
const IMAGE_ID: &str = "sha256:fedcba9876543210";
const TOKEN: &str = "public-session-name-inspection-fixture-token-0000000000";

/// Declared fixture of a bound ReadWriteOncePod claim.
pub(crate) fn bound_pvc(namespace: &str, name: &str, uid: &str) -> Value {
    json!({
        "apiVersion":"v1","kind":"PersistentVolumeClaim",
        "metadata":{"name":name,"namespace":namespace,"uid":uid,"resourceVersion":"901",
            "creationTimestamp":"2026-11-12T07:59:00Z",
            "finalizers":["kubernetes.io/pvc-protection"]},
        "spec":{"accessModes":["ReadWriteOncePod"],"resources":{"requests":{"storage":"1Gi"}},
            "storageClassName":"csi-rwop","volumeMode":"Filesystem",
            "volumeName":format!("pvc-{uid}")},
        "status":{"phase":"Bound","accessModes":["ReadWriteOncePod"],"capacity":{"storage":"1Gi"}}
    })
}
/// Claim named for the intent's attempt, observed through its authority.
pub(crate) fn bound_claim(
    intent: &KubernetesSessionReservationIntentV2,
    uid: &str,
) -> KubernetesSessionWorkspaceClaimV2 {
    let name = session_workspace_claim_name(intent.ids().unwrap().attempt_id);
    let pvc = bound_pvc(intent.authority().namespace(), &name, uid);
    parse_workspace_claim(intent.authority(), &name, pvc, OBSERVED_AT).unwrap()
}
fn with_container(pod: &Value, state: Value, restarts: u64) -> Value {
    let mut pod = pod.clone();
    pod["status"] = json!({"phase":"Running","containerStatuses":[{"name":"worker",
        "restartCount":restarts,"containerID":CONTAINER_ID,"imageID":IMAGE_ID,
        "state":state}]});
    pod
}

#[test]
fn observation_binds_exact_uid_spec_and_container_identity() {
    let rendered = rendered_case();
    let authority = rendered.intent.authority();
    let created = admitted(&rendered.request);
    let instance = validate_declared_admitted_session_pod(
        &rendered.request,
        &serde_json::to_vec(&created).unwrap(),
    )
    .unwrap();
    assert_eq!(instance.pod_uid(), POD_UID);
    assert_eq!(instance.created_resource_version(), "42");
    assert_eq!(instance.target(), rendered.request.target());
    let parse = |pod: Value| parse_session_observation(authority, &instance, pod);
    // Held: the exact admitted object, its gate, no node and no container.
    let held = parse(created.clone()).unwrap();
    assert_eq!(held.pod_uid(), POD_UID);
    assert_eq!(held.resource_version(), "42");
    assert_eq!(held.state(), &SessionObservationStateV2::Held);
    // The same name with another UID is another instance.
    assert_eq!(
        parse(replace(&created, "/metadata/uid", json!(OTHER_UID))).err(),
        Some(KubernetesSessionRuntimeError::InstanceReplaced)
    );
    // A gate with a node binding is not the admitted held object.
    assert_eq!(
        parse(replace(&created, "/spec/nodeName", json!("node-a"))).err(),
        Some(KubernetesSessionRuntimeError::ProfileChanged)
    );
    let mut unscheduled = created.clone();
    assert!(
        unscheduled["spec"]
            .as_object_mut()
            .unwrap()
            .remove("schedulingGates")
            .is_some()
    );
    unscheduled["metadata"]["resourceVersion"] = json!("43");
    assert_eq!(
        parse(unscheduled.clone()).unwrap().state(),
        &SessionObservationStateV2::ReleasedUnscheduled
    );
    let mut released = unscheduled.clone();
    released["spec"]["nodeName"] = json!("node-a");
    let running_state = json!({"running":{"startedAt":"2026-11-12T08:01:00Z"}});
    let running = with_container(&released, running_state.clone(), 0);
    let observed = parse(running.clone()).unwrap();
    assert_eq!(observed.resource_version(), "43");
    match observed.state() {
        SessionObservationStateV2::Scheduled {
            node_name,
            container:
                SessionContainerStateV2::Running {
                    container_id,
                    image_id,
                    started_at,
                },
        } => {
            assert_eq!(node_name, "node-a");
            assert_eq!(container_id, CONTAINER_ID);
            assert_eq!(image_id.as_deref(), Some(IMAGE_ID));
            assert_eq!(started_at.to_rfc3339(), "2026-11-12T08:01:00+00:00");
        }
        other => panic!("unexpected released observation: {other:?}"),
    }
    // Any restart voids the exact container identity.
    assert_eq!(
        parse(with_container(&released, running_state, 1)).err(),
        Some(KubernetesSessionRuntimeError::InstanceRestarted)
    );
    // Spec drift after release is a profile change, never the same instance.
    let other_image = format!("local/other@sha256:{}", "c".repeat(64));
    assert_eq!(
        parse(replace(
            &running,
            "/spec/containers/0/image",
            json!(other_image)
        ))
        .err(),
        Some(KubernetesSessionRuntimeError::ProfileChanged)
    );
    // A deletion request is observed, never terminal.
    let deleting = replace(
        &running,
        "/metadata/deletionTimestamp",
        json!("2026-11-12T08:02:00Z"),
    );
    match parse(deleting).unwrap().state() {
        SessionObservationStateV2::Deleting {
            container: SessionContainerStateV2::Running { container_id, .. },
        } => assert_eq!(container_id, CONTAINER_ID),
        other => panic!("unexpected deleting observation: {other:?}"),
    }
    // Terminated keeps the exact container identity and the declared code.
    let terminated = with_container(
        &released,
        json!({"terminated":{"exitCode":66,"reason":"OOMKilled",
            "startedAt":"2026-11-12T08:01:00Z","finishedAt":"2026-11-12T08:01:30Z"}}),
        0,
    );
    match parse(terminated).unwrap().state() {
        SessionObservationStateV2::Scheduled {
            container:
                SessionContainerStateV2::Terminated {
                    container_id,
                    exit_code,
                    oom_killed,
                    started_at,
                    finished_at,
                },
            ..
        } => {
            assert_eq!(container_id, CONTAINER_ID);
            assert_eq!(*exit_code, Some(66));
            assert_eq!(*oom_killed, Some(true));
            assert!(started_at.is_some() && started_at < finished_at);
        }
        other => panic!("unexpected terminated observation: {other:?}"),
    }
    // A waiting container is a fact, not progress or identity.
    let waiting = with_container(
        &released,
        json!({"waiting":{"reason":"ContainerCreating"}}),
        0,
    );
    assert!(matches!(
        parse(waiting).unwrap().state(),
        SessionObservationStateV2::Scheduled {
            container: SessionContainerStateV2::Waiting,
            ..
        }
    ));
    // The instance is not interpretable through another authority.
    let elsewhere = KubernetesAuthorityV1::new(
        Uuid::new_v4(),
        "https://127.0.0.1:7443",
        &authority.cluster_namespace_uid,
        authority.namespace(),
        &authority.namespace_uid,
        &"b".repeat(64),
    )
    .unwrap();
    assert_eq!(
        parse_session_observation(&elsewhere, &instance, created).err(),
        Some(KubernetesSessionRuntimeError::Kubernetes(
            KubernetesError::InvalidAuthority
        ))
    );
}

#[test]
fn workspace_claim_requires_bound_read_write_once_pod_and_retains_uid() {
    let authority = KubernetesAuthorityV1::new(
        Uuid::from_u128(1),
        "https://127.0.0.1:6443",
        "11111111-1111-4111-8111-111111111111",
        "tenant-a",
        "22222222-2222-4222-8222-222222222222",
        &"a".repeat(64),
    )
    .unwrap();
    let name = session_workspace_claim_name(Uuid::from_u128(9));
    let valid = bound_pvc("tenant-a", &name, CLAIM_UID);
    let claim = parse_workspace_claim(&authority, &name, valid.clone(), OBSERVED_AT).unwrap();
    assert_eq!(
        serde_json::to_value(&claim).unwrap(),
        json!({"version":1,"authority_fingerprint":authority.fingerprint(),
            "namespace":"tenant-a","name":name,"uid":CLAIM_UID,"resource_version":"901",
            "storage_class_name":"csi-rwop","volume_name":format!("pvc-{CLAIM_UID}"),
            "capacity":"1Gi","access_mode":"ReadWriteOncePod","observed_at":OBSERVED_AT})
    );
    assert!(claim.is_for(&authority, &name));
    // The journal form revalidates to the same descriptive claim only.
    let wire = serde_json::to_value(&claim).unwrap();
    assert_eq!(
        KubernetesSessionWorkspaceClaimV2::retained_from_value(wire.clone()).unwrap(),
        claim
    );
    for (field, value) in [
        ("access_mode", json!("ReadWriteOnce")),
        ("uid", json!(Uuid::nil().to_string())),
        ("observed_at", json!("2026-11-12T08:00:00Z")),
        ("capacity", json!("lots")),
        ("unexpected", json!(true)),
    ] {
        assert_eq!(
            KubernetesSessionWorkspaceClaimV2::retained_from_value(replace(
                &wire,
                &format!("/{field}"),
                value
            ))
            .err(),
            Some(KubernetesSessionRuntimeError::WorkspaceClaimInvalid),
            "{field}"
        );
    }
    for (pointer, value) in [
        ("/spec/accessModes", json!(["ReadWriteOnce"])),
        (
            "/spec/accessModes",
            json!(["ReadWriteOncePod", "ReadWriteOnce"]),
        ),
        ("/status/accessModes", json!(["ReadWriteOnce"])),
        ("/status/phase", json!("Pending")),
        ("/metadata/deletionTimestamp", json!("2026-11-12T08:00:00Z")),
        ("/metadata/namespace", json!("tenant-b")),
        ("/metadata/name", json!("ai-guard-ws-other")),
        ("/metadata/uid", json!(Uuid::nil().to_string())),
        ("/metadata/finalizers", json!(["example.com/hold"])),
        ("/spec/volumeName", json!("")),
        ("/spec/volumeMode", json!("Block")),
        ("/spec/storageClassName", json!("")),
        (
            "/spec/dataSource",
            json!({"kind":"VolumeSnapshot","name":"seeded"}),
        ),
        ("/status/capacity/storage", json!("lots")),
        ("/kind", json!("PersistentVolume")),
    ] {
        assert_eq!(
            parse_workspace_claim(
                &authority,
                &name,
                replace(&valid, pointer, value),
                OBSERVED_AT
            )
            .err(),
            Some(KubernetesSessionRuntimeError::WorkspaceClaimInvalid),
            "{pointer}"
        );
    }
    let mut missing = valid.clone();
    assert!(
        missing["spec"]
            .as_object_mut()
            .unwrap()
            .remove("volumeName")
            .is_some()
    );
    assert_eq!(
        parse_workspace_claim(&authority, &name, missing, OBSERVED_AT).err(),
        Some(KubernetesSessionRuntimeError::WorkspaceClaimInvalid)
    );
    assert_eq!(
        parse_workspace_claim(&authority, &name, valid, "2026-11-12T08:00:00Z").err(),
        Some(KubernetesSessionRuntimeError::WorkspaceClaimInvalid)
    );
}

struct NameBehavior {
    pod: Option<Value>,
    pvc: Option<Value>,
    requests: Vec<String>,
}
struct NamePeer {
    endpoint: String,
    state: Arc<Mutex<NameBehavior>>,
    task: JoinHandle<()>,
}
impl Drop for NamePeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl NamePeer {
    async fn start(material: &TlsMaterial) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("https://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(NameBehavior {
            pod: None,
            pvc: None,
            requests: Vec::new(),
        }));
        let app = Router::new()
            .fallback(name_response)
            .with_state(state.clone());
        let identity = ServerIdentity::from_files(&material.certificate, &material.key).unwrap();
        let task = tokio::spawn(async move {
            serve(listener, app, identity, std::future::pending::<()>())
                .await
                .unwrap();
        });
        Self {
            endpoint,
            state,
            task,
        }
    }
    fn set(&self, pod: Option<Value>, pvc: Option<Value>) {
        let mut state = self.state.lock().unwrap();
        state.pod = pod;
        state.pvc = pvc;
    }
    fn requests(&self) -> Vec<String> {
        self.state.lock().unwrap().requests.clone()
    }
}
async fn name_response(
    State(state): State<Arc<Mutex<NameBehavior>>>,
    request: Request<Body>,
) -> Response {
    let authorized = request
        .headers()
        .get("authorization")
        .is_some_and(|value| value.as_bytes() == format!("Bearer {TOKEN}").as_bytes());
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let mut state = state.lock().unwrap();
    state.requests.push(format!("{method} {path}"));
    let body = if !authorized || method != "GET" || request.uri().query().is_some() {
        None
    } else if path == "/api/v1/namespaces/kube-system" {
        Some(json!({"apiVersion":"v1","kind":"Namespace",
            "metadata":{"name":"kube-system","uid":"11111111-1111-4111-8111-111111111111"},
            "status":{"phase":"Active"}}))
    } else if path == "/api/v1/namespaces/tenant-a" {
        Some(json!({"apiVersion":"v1","kind":"Namespace",
            "metadata":{"name":"tenant-a","uid":"22222222-2222-4222-8222-222222222222"},
            "status":{"phase":"Active"}}))
    } else if path.starts_with("/api/v1/namespaces/tenant-a/pods/") {
        state.pod.clone()
    } else if path.starts_with("/api/v1/namespaces/tenant-a/persistentvolumeclaims/") {
        state.pvc.clone()
    } else {
        None
    };
    match body {
        Some(body) => Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap(),
        None => Response::builder()
            .status(404)
            .header("content-type", "application/json")
            .body(Body::from(r#"{"kind":"Status","code":404}"#))
            .unwrap(),
    }
}

#[tokio::test]
async fn name_inspection_reports_presence_without_ownership() {
    let material = TlsMaterial::create().await;
    let peer = NamePeer::start(&material).await;
    let ca = PrivateFile::open(&material.ca, 64 * 1024).unwrap();
    let trust = ca.digest().to_owned();
    let token = material.ca.parent().unwrap().join("session-names-token");
    std::fs::write(&token, TOKEN).unwrap();
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
    let rendered = rendered_case_with(fixture_at(&peer.endpoint, &trust));
    let target = rendered.request.target();
    let runtime = KubernetesRuntime::connect(
        rendered.intent.authority().clone(),
        KubernetesCredentials::bearer(ca, PrivateFile::open(&token, 8192).unwrap()),
    )
    .unwrap();
    let claim_path = format!(
        "GET /api/v1/namespaces/tenant-a/persistentvolumeclaims/{}",
        rendered.claim.name()
    );
    let pod_path = format!("GET /api/v1/namespaces/tenant-a/pods/{}", target.pod_name());
    let anchors = [
        "GET /api/v1/namespaces/kube-system".to_owned(),
        "GET /api/v1/namespaces/tenant-a".to_owned(),
    ];
    peer.set(
        Some(admitted(&rendered.request)),
        Some(bound_pvc("tenant-a", rendered.claim.name(), CLAIM_UID)),
    );
    let present = runtime.inspect_session_names(target).await.unwrap();
    assert_eq!(
        present.pod(),
        &KubernetesSessionObjectPresenceV2::Present {
            uid: POD_UID.into(),
            resource_version: "42".into(),
            deletion_requested: false,
        }
    );
    assert_eq!(
        present.workspace_claim(),
        &KubernetesSessionObjectPresenceV2::Present {
            uid: CLAIM_UID.into(),
            resource_version: "901".into(),
            deletion_requested: false,
        }
    );
    assert_eq!(present.workspace_claim_uid_matches_retained(), Some(true));
    // Descriptive only: the complete key set carries no ownership claim.
    let wire = serde_json::to_value(&present).unwrap();
    let mut top: Vec<&str> = wire
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    top.sort_unstable();
    assert_eq!(
        top,
        [
            "authority_fingerprint",
            "namespace",
            "observed_at",
            "pod",
            "pod_name",
            "version",
            "workspace_claim",
            "workspace_claim_name",
            "workspace_claim_uid_matches_retained",
        ]
    );
    let mut keys = BTreeSet::new();
    collect_keys(&wire, &mut keys);
    assert!(!keys.iter().any(|key| key.contains("own")), "{keys:?}");
    let expected = [
        anchors[0].clone(),
        anchors[1].clone(),
        pod_path.clone(),
        claim_path.clone(),
        anchors[0].clone(),
        anchors[1].clone(),
    ];
    assert_eq!(peer.requests(), expected);
    // A 404 is an observed absence at the name, never terminal or ownership;
    // another claim UID at the attempt name is reported, not adopted.
    peer.set(
        None,
        Some(bound_pvc("tenant-a", rendered.claim.name(), OTHER_UID)),
    );
    let absent = runtime.inspect_session_names(target).await.unwrap();
    assert_eq!(
        absent.pod(),
        &KubernetesSessionObjectPresenceV2::AbsentObserved404
    );
    assert_eq!(absent.workspace_claim_uid_matches_retained(), Some(false));
    // A target rendered for another authority is refused before any request.
    let before = peer.requests().len();
    let foreign = rendered_case();
    assert_eq!(
        runtime
            .inspect_session_names(foreign.request.target())
            .await
            .unwrap_err(),
        KubernetesError::InvalidAuthority
    );
    let requests = peer.requests();
    assert_eq!(requests.len(), before);
    assert_eq!(requests.len(), 12);
    assert!(requests.iter().all(|line| line.starts_with("GET ")));
}
