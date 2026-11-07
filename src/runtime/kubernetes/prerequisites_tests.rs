//! Real loopback TLS requests with declared API fixtures; no cluster effects.
use super::{
    BeforeKubernetesPreflightGuard, KubernetesAuthorityV1, KubernetesCredentials, KubernetesError,
    KubernetesNamespaceStateV1, KubernetesPermissionDecisionV1, KubernetesPermissionReviewStatusV1,
    KubernetesPreflightPlanV1, KubernetesPreflightResourcesV1, KubernetesPreflightTargetV1,
    KubernetesResult, KubernetesRuntime, admission::transport_tests::TlsMaterial,
};
use crate::{
    control_fs::PrivateFile,
    tls::{ServerIdentity, serve},
};
use axum::{Router, body::Body, extract::State, http::Request, response::Response};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;

const CLUSTER_UID: &str = "11111111-1111-4111-8111-111111111111";
const NAMESPACE_UID: &str = "22222222-2222-4222-8222-222222222222";
const CANARY: &str = "private-authorization-reason-must-not-be-reported";
const TOKEN: &str = "public-kubernetes-prerequisite-fixture-token-000000000000";

#[derive(Clone)]
enum ReviewMode {
    Standard,
    Denied,
    NoOpinion,
    EvaluationError,
    Contradictory,
    ChangedScope,
    Duplicate,
    Deep,
    Oversized,
    Http(u16),
    Redirect(String),
    ChangedNamespace,
    TerminatingAfterReview,
}
struct Behavior {
    namespace_status: Value,
    cluster_status: Value,
    deleting: bool,
    namespace_uid: String,
    mode: ReviewMode,
    requests: Vec<(String, String, Option<Value>)>,
}

#[derive(Clone)]
struct PeerState {
    inner: Arc<Mutex<Behavior>>,
    posts: Arc<AtomicUsize>,
}
struct Peer {
    runtime: KubernetesRuntime,
    state: PeerState,
    task: JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Peer {
    async fn new(material: &TlsMaterial, namespace_status: Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("https://{}", listener.local_addr().unwrap());
        let ca = PrivateFile::open(&material.ca, 64 * 1024).unwrap();
        let authority = KubernetesAuthorityV1::new(
            Uuid::from_u128(1),
            &endpoint,
            CLUSTER_UID,
            "tenant-a",
            NAMESPACE_UID,
            ca.digest(),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let token = material.ca.parent().unwrap().join("prerequisites-token");
        std::fs::write(&token, TOKEN).unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        let credentials =
            KubernetesCredentials::bearer(ca, PrivateFile::open(&token, 8192).unwrap());
        let runtime = KubernetesRuntime::connect(authority, credentials).unwrap();
        let state = PeerState {
            inner: Arc::new(Mutex::new(Behavior {
                namespace_status,
                cluster_status: json!({"phase":"Active"}),
                deleting: false,
                namespace_uid: NAMESPACE_UID.into(),
                mode: ReviewMode::Standard,
                requests: Vec::new(),
            })),
            posts: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new().fallback(respond).with_state(state.clone());
        let identity = ServerIdentity::from_pem(
            &std::fs::read(&material.certificate).unwrap(),
            &std::fs::read(&material.key).unwrap(),
        )
        .unwrap();
        let task = tokio::spawn(async move {
            serve(listener, app, identity, std::future::pending::<()>())
                .await
                .unwrap();
        });
        Self {
            runtime,
            state,
            task,
        }
    }
}
async fn respond(State(state): State<PeerState>, request: Request<Body>) -> Response {
    assert_eq!(
        request
            .headers()
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap(),
        format!("Bearer {TOKEN}")
    );
    assert!(request.headers().get("impersonate-user").is_none());
    let path = request.uri().path().to_owned();
    let query = request.uri().query().map(str::to_owned);
    let method = request.method().as_str().to_owned();
    let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: Option<Value> = (!bytes.is_empty()).then(|| serde_json::from_slice(&bytes).unwrap());
    let mut behavior = state.inner.lock().unwrap();
    behavior
        .requests
        .push((method.clone(), path.clone(), body.clone()));
    let (status, value) = match (method.as_str(), path.as_str()) {
        ("GET", "/api/v1/namespaces/kube-system") => (
            200,
            json!({
                "apiVersion":"v1", "kind":"Namespace", "metadata":{"name":"kube-system","uid":CLUSTER_UID},
                "status":behavior.cluster_status
            }),
        ),
        ("GET", "/api/v1/namespaces/tenant-a") => (200, {
            let mut value = json!({
                "apiVersion":"v1", "kind":"Namespace", "metadata":{"name":"tenant-a","uid":behavior.namespace_uid},
                "status":behavior.namespace_status
            });
            if behavior.deleting {
                value["metadata"]["deletionTimestamp"] = json!("2026-11-06T08:00:00Z");
            }
            value
        }),
        ("GET", "/version") => (200, json!({"gitVersion":"v1.36.1"})),
        ("POST", "/api/v1/namespaces/tenant-a/pods") => {
            assert_eq!(query.as_deref(), Some("dryRun=All&fieldValidation=Strict"));
            state.posts.fetch_add(1, Ordering::SeqCst);
            (201, body.unwrap())
        }
        ("POST", "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews") => {
            assert!(query.is_none());
            assert!(bytes.len() <= 4096);
            let body = body.unwrap();
            assert_eq!(body["spec"]["resourceAttributes"]["namespace"], "tenant-a");
            let mode = behavior.mode.clone();
            if let ReviewMode::Http(status) = mode {
                return response(status, json!({"private":CANARY}));
            }
            if let ReviewMode::Redirect(location) = mode {
                return Response::builder()
                    .status(302)
                    .header("location", location)
                    .body(Body::empty())
                    .unwrap();
            }
            if let ReviewMode::Oversized = mode {
                return raw(201, vec![b'x'; 32 * 1024 + 1]);
            }
            let attributes = &body["spec"]["resourceAttributes"];
            let allowed = attributes["resource"] == "pods"
                && (attributes["verb"] == "get" || attributes["verb"] == "create")
                && attributes.get("subresource").is_none();
            let mut value = json!({"apiVersion":"authorization.k8s.io/v1", "kind":"SelfSubjectAccessReview",
                "spec":body["spec"], "status":{"allowed":allowed,"denied":!allowed,"reason":CANARY}});
            // Exercise the actual API omitempty response shape.
            value["spec"]["resourceAttributes"]
                .as_object_mut()
                .unwrap()
                .remove("group");
            if let ReviewMode::Duplicate = mode {
                // A permissive last-wins decoder would recover the exact valid
                // neighbouring response; only the duplicated member is invalid.
                let encoded = serde_json::to_string(&value).unwrap();
                let original = format!("\"allowed\":{allowed}");
                assert_eq!(encoded.matches(&original).count(), 1);
                let duplicate = format!("\"allowed\":{},\"allowed\":{allowed}", !allowed);
                return raw(201, encoded.replacen(&original, &duplicate, 1).into_bytes());
            }
            if let ReviewMode::Deep = mode {
                // Application parsing ignores this field; only the bounded JSON
                // decoder should refuse an otherwise valid response's depth.
                let mut nested = json!(0);
                for _ in 0..20 {
                    nested = json!([nested]);
                }
                value["ignored"] = nested;
                return response(201, value);
            }
            match mode {
                ReviewMode::Denied => value["status"] = json!({"allowed":false,"denied":true}),
                ReviewMode::NoOpinion => value["status"] = json!({"allowed":false}),
                ReviewMode::EvaluationError => {
                    value["status"] = json!({"allowed":true,"evaluationError":CANARY})
                }
                ReviewMode::Contradictory => {
                    value["status"] = json!({"allowed":true,"denied":true})
                }
                ReviewMode::ChangedScope => {
                    value["spec"]["resourceAttributes"]["namespace"] = json!("tenant-b")
                }
                ReviewMode::ChangedNamespace => behavior.namespace_uid = CLUSTER_UID.into(),
                ReviewMode::TerminatingAfterReview => {
                    behavior.namespace_status = json!({"phase":"Terminating"})
                }
                _ => {}
            }
            (201, value)
        }
        _ => panic!("request outside the namespace precondition fixture"),
    };
    response(status, value)
}
fn response(status: u16, value: Value) -> Response {
    raw(status, serde_json::to_vec(&value).unwrap())
}
fn raw(status: u16, bytes: Vec<u8>) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(bytes))
        .unwrap()
}
struct Guard;
impl BeforeKubernetesPreflightGuard for Guard {
    fn check_before_dry_run(
        self: Box<Self>,
        _target: &KubernetesPreflightTargetV1,
    ) -> KubernetesResult<()> {
        Ok(())
    }
}

struct CountedGuard(Arc<AtomicUsize>);
impl BeforeKubernetesPreflightGuard for CountedGuard {
    fn check_before_dry_run(
        self: Box<Self>,
        _target: &KubernetesPreflightTargetV1,
    ) -> KubernetesResult<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn permission_preflight_plan(runtime: &KubernetesRuntime) -> KubernetesPreflightPlanV1 {
    KubernetesPreflightPlanV1::new(
        runtime.binding(),
        Uuid::from_u128(2),
        Uuid::from_u128(3),
        &format!("fixture.invalid/worker@sha256:{}", "a".repeat(64)),
        KubernetesPreflightResourcesV1 {
            cpu_millis: 1000,
            memory_mib: 1024,
            workspace_mib: 128,
            temporary_mib: 64,
            deadline_seconds: 30,
        },
    )
    .unwrap()
}

#[tokio::test]
async fn preflight_unconfirmed_create_permission_never_consumes_guard_or_posts_pod() {
    let material = TlsMaterial::create().await;
    let mut observations = Vec::new();
    for (name, mode) in [
        ("denied", ReviewMode::Denied),
        ("no opinion", ReviewMode::NoOpinion),
        ("evaluation error", ReviewMode::EvaluationError),
    ] {
        let peer = Peer::new(&material, json!({"phase":"Active"})).await;
        peer.state.inner.lock().unwrap().mode = mode;
        let guard_calls = Arc::new(AtomicUsize::new(0));
        let result = peer
            .runtime
            .preflight(
                &permission_preflight_plan(&peer.runtime),
                Box::new(CountedGuard(Arc::clone(&guard_calls))),
            )
            .await;
        let requests = &peer.state.inner.lock().unwrap().requests;
        let reviews = requests
            .iter()
            .filter(|(method, path, _)| {
                method == "POST" && path == "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews"
            })
            .count();
        observations.push((
            name,
            result.is_err(),
            reviews,
            guard_calls.load(Ordering::SeqCst),
            peer.state.posts.load(Ordering::SeqCst),
        ));
    }
    assert_eq!(
        observations,
        vec![
            ("denied", true, 1, 0, 0),
            ("no opinion", true, 1, 0, 0),
            ("evaluation error", true, 1, 0, 0),
        ],
        "only an observed Allowed create/pods review may reach the guard and Pod POST"
    );
}

#[tokio::test]
async fn preflight_allowed_create_permission_is_exact_and_precedes_the_guarded_dry_run() {
    let material = TlsMaterial::create().await;
    let peer = Peer::new(&material, json!({"phase":"Active"})).await;
    let guard_calls = Arc::new(AtomicUsize::new(0));
    let observation = peer
        .runtime
        .preflight(
            &permission_preflight_plan(&peer.runtime),
            Box::new(CountedGuard(Arc::clone(&guard_calls))),
        )
        .await
        .unwrap();
    assert_eq!(
        observation.target.authority_fingerprint,
        peer.runtime.binding().fingerprint()
    );
    assert_eq!(guard_calls.load(Ordering::SeqCst), 1);
    assert_eq!(peer.state.posts.load(Ordering::SeqCst), 1);
    let requests = &peer.state.inner.lock().unwrap().requests;
    let reviews: Vec<_> = requests
        .iter()
        .enumerate()
        .filter(|(_, (method, path, _))| {
            method == "POST" && path == "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews"
        })
        .collect();
    assert_eq!(
        reviews.len(),
        1,
        "preflight asks only for its create/pods operation"
    );
    assert_eq!(
        reviews[0].1.2,
        Some(json!({
            "apiVersion":"authorization.k8s.io/v1",
            "kind":"SelfSubjectAccessReview",
            "spec":{"resourceAttributes":{
                "namespace":"tenant-a", "group":"", "version":"v1",
                "resource":"pods", "verb":"create"
            }}
        }))
    );
    let pod_post = requests
        .iter()
        .position(|(method, path, _)| {
            method == "POST" && path == "/api/v1/namespaces/tenant-a/pods"
        })
        .unwrap();
    assert!(reviews[0].0 < pod_post);
    assert!(peer.runtime.binding().require_workload_admission().is_err());
}

#[tokio::test]
async fn preflight_rechecks_namespace_uid_after_create_permission_review_before_pod_post() {
    let material = TlsMaterial::create().await;
    let peer = Peer::new(&material, json!({"phase":"Active"})).await;
    peer.state.inner.lock().unwrap().mode = ReviewMode::ChangedNamespace;
    let guard_calls = Arc::new(AtomicUsize::new(0));
    let result = peer
        .runtime
        .preflight(
            &permission_preflight_plan(&peer.runtime),
            Box::new(CountedGuard(Arc::clone(&guard_calls))),
        )
        .await;
    assert_eq!(result.err(), Some(KubernetesError::AuthorityChanged));
    assert_eq!(guard_calls.load(Ordering::SeqCst), 0);
    assert_eq!(peer.state.posts.load(Ordering::SeqCst), 0);
    let requests = &peer.state.inner.lock().unwrap().requests;
    assert_eq!(
        requests
            .iter()
            .filter(|(method, path, _)| {
                method == "POST" && path == "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews"
            })
            .count(),
        1
    );
}

#[tokio::test]
async fn namespace_phase_must_be_active_before_a_preflight_request() {
    let material = TlsMaterial::create().await;
    for status in [
        json!({"phase":"Terminating"}),
        json!({}),
        json!({"phase":"Unrecognised"}),
    ] {
        let peer = Peer::new(&material, status.clone()).await;
        let plan = KubernetesPreflightPlanV1::new(
            peer.runtime.binding(),
            Uuid::from_u128(2),
            Uuid::from_u128(3),
            &format!("fixture.invalid/worker@sha256:{}", "a".repeat(64)),
            KubernetesPreflightResourcesV1 {
                cpu_millis: 1000,
                memory_mib: 1024,
                workspace_mib: 128,
                temporary_mib: 64,
                deadline_seconds: 30,
            },
        )
        .unwrap();
        assert!(
            peer.runtime
                .preflight(&plan, Box::new(Guard))
                .await
                .is_err(),
            "a namespace without an observed Active phase must refuse preflight: {status}"
        );
        assert_eq!(
            peer.state.posts.load(Ordering::SeqCst),
            0,
            "a namespace precondition failure must not reach the Pod POST"
        );
    }
}

#[tokio::test]
async fn opt_in_reviews_exact_actions_and_keeps_legacy_doctor_get_only() {
    let material = TlsMaterial::create().await;
    let peer = Peer::new(&material, json!({"phase":"Active"})).await;
    peer.runtime.doctor().await.unwrap();
    assert_eq!(peer.state.inner.lock().unwrap().requests.len(), 3);
    assert!(
        peer.state
            .inner
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|(method, _, body)| method == "GET" && body.is_none())
    );
    let (diagnostics, prerequisites) = peer.runtime.doctor_with_prerequisites().await.unwrap();
    assert_eq!(diagnostics.namespace_uid, NAMESPACE_UID);
    assert_eq!(prerequisites.version, 1);
    assert_eq!(
        prerequisites.namespace.state,
        KubernetesNamespaceStateV1::Active
    );
    assert_eq!(
        prerequisites.permission_review_status,
        KubernetesPermissionReviewStatusV1::Complete
    );
    assert_eq!(prerequisites.access_reviews.len(), 12);
    assert_eq!(
        prerequisites.access_reviews[0].decision,
        KubernetesPermissionDecisionV1::Allowed
    );
    assert_eq!(
        prerequisites.access_reviews[2].decision,
        KubernetesPermissionDecisionV1::Denied
    );
    assert!(prerequisites.completed_at >= prerequisites.started_at);
    let encoded = serde_json::to_string(&prerequisites).unwrap();
    assert!(!encoded.contains(CANARY));
    assert!(!encoded.contains(TOKEN));
    assert!(peer.runtime.binding().require_workload_admission().is_err());
    let state = peer.state.inner.lock().unwrap();
    assert_eq!(state.requests.len(), 20);
    let posts: Vec<_> = state
        .requests
        .iter()
        .filter(|(method, _, _)| method == "POST")
        .collect();
    assert_eq!(posts.len(), 12);
    for (_, path, body) in posts {
        assert_eq!(
            path,
            "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews"
        );
        let spec = &body.as_ref().unwrap()["spec"];
        assert_eq!(spec.as_object().unwrap().len(), 1);
        assert_eq!(spec["resourceAttributes"]["namespace"], "tenant-a");
        assert_eq!(spec["resourceAttributes"]["version"], "v1");
    }
}

#[tokio::test]
async fn inactive_unknown_or_deleting_anchors_explicitly_skip_reviews() {
    let material = TlsMaterial::create().await;
    for (status, deleting, cluster_status, expected) in [
        (
            json!({"phase":"Terminating"}),
            false,
            json!({"phase":"Active"}),
            KubernetesNamespaceStateV1::Terminating,
        ),
        (
            json!({}),
            false,
            json!({"phase":"Active"}),
            KubernetesNamespaceStateV1::Unknown,
        ),
        (
            Value::Null,
            false,
            json!({"phase":"Active"}),
            KubernetesNamespaceStateV1::Unknown,
        ),
        (
            json!({"phase":"Active"}),
            true,
            json!({"phase":"Active"}),
            KubernetesNamespaceStateV1::Terminating,
        ),
        (
            json!({"phase":"Active"}),
            false,
            json!({}),
            KubernetesNamespaceStateV1::Active,
        ),
    ] {
        let peer = Peer::new(&material, status).await;
        {
            let mut behavior = peer.state.inner.lock().unwrap();
            behavior.deleting = deleting;
            behavior.cluster_status = cluster_status;
        }
        let (_, prerequisites) = peer.runtime.doctor_with_prerequisites().await.unwrap();
        assert_eq!(prerequisites.namespace.state, expected);
        assert_eq!(
            prerequisites.permission_review_status,
            KubernetesPermissionReviewStatusV1::SkippedNamespaceNotActive
        );
        assert!(prerequisites.access_reviews.is_empty());
        assert_eq!(peer.state.inner.lock().unwrap().requests.len(), 3);
        assert_eq!(
            peer.runtime.verify_active_authority().await.err(),
            Some(KubernetesError::NamespaceNotActive)
        );
        assert!(
            peer.state
                .inner
                .lock()
                .unwrap()
                .requests
                .iter()
                .all(|(method, _, _)| method == "GET")
        );
    }
}

#[tokio::test]
async fn ssar_status_unknown_and_protocol_failures_never_grant_activation() {
    let material = TlsMaterial::create().await;
    for mode in [ReviewMode::NoOpinion, ReviewMode::EvaluationError] {
        let peer = Peer::new(&material, json!({"phase":"Active"})).await;
        let evaluation_error = matches!(mode, ReviewMode::EvaluationError);
        peer.state.inner.lock().unwrap().mode = mode;
        let (_, prerequisites) = peer.runtime.doctor_with_prerequisites().await.unwrap();
        assert!(
            prerequisites
                .access_reviews
                .iter()
                .all(
                    |review| review.decision == KubernetesPermissionDecisionV1::Unknown
                        && review.evaluation_error_present == evaluation_error
                )
        );
        assert!(
            !serde_json::to_string(&prerequisites)
                .unwrap()
                .contains(CANARY)
        );
    }
    for (mode, expected) in [
        (ReviewMode::Contradictory, KubernetesError::InvalidResponse),
        (ReviewMode::ChangedScope, KubernetesError::InvalidResponse),
        (ReviewMode::Duplicate, KubernetesError::InvalidResponse),
        (ReviewMode::Deep, KubernetesError::InvalidResponse),
        (ReviewMode::Oversized, KubernetesError::ResponseTooLarge),
        (ReviewMode::Http(403), KubernetesError::ApiRejected(403)),
        (
            ReviewMode::ChangedNamespace,
            KubernetesError::AuthorityChanged,
        ),
        (
            ReviewMode::TerminatingAfterReview,
            KubernetesError::NamespaceNotActive,
        ),
    ] {
        let peer = Peer::new(&material, json!({"phase":"Active"})).await;
        peer.state.inner.lock().unwrap().mode = mode;
        let error = peer.runtime.doctor_with_prerequisites().await.unwrap_err();
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains(CANARY));
        assert!(!format!("{error:?} {error}").contains(TOKEN));
        assert!(peer.runtime.binding().require_workload_admission().is_err());
    }
    let trap = Peer::new(&material, json!({"phase":"Active"})).await;
    let peer = Peer::new(&material, json!({"phase":"Active"})).await;
    peer.state.inner.lock().unwrap().mode =
        ReviewMode::Redirect(trap.runtime.binding().endpoint.clone());
    assert_eq!(
        peer.runtime.doctor_with_prerequisites().await.unwrap_err(),
        KubernetesError::ApiRejected(302)
    );
    assert!(trap.state.inner.lock().unwrap().requests.is_empty());
    assert_eq!(
        peer.state.inner.lock().unwrap().requests.len(),
        4,
        "redirect was not retried"
    );
}

#[test]
fn review_parser_rejects_scope_divergence_and_ambiguous_status() {
    use super::prerequisites::{KubernetesPermissionActionV1, parse_review};
    let action = KubernetesPermissionActionV1::CreatePods;
    let valid = json!({"apiVersion":"authorization.k8s.io/v1","kind":"SelfSubjectAccessReview",
        "spec":{"resourceAttributes":action.attributes("tenant-a")},"status":{"allowed":true}});
    assert_eq!(
        parse_review(action, "tenant-a", valid.clone())
            .unwrap()
            .decision,
        KubernetesPermissionDecisionV1::Allowed
    );
    for (pointer, value) in [
        ("/spec/resourceAttributes/namespace", json!("tenant-b")),
        ("/spec/resourceAttributes/resource", json!("secrets")),
        ("/spec/resourceAttributes/group", json!("apps")),
        ("/spec/resourceAttributes/version", json!("v2")),
        ("/spec/resourceAttributes/verb", json!("delete")),
        ("/status/allowed", Value::Null),
        ("/status", json!({"allowed":true,"denied":true})),
        ("/status", json!({"allowed":true,"evaluationError":false})),
    ] {
        let mut altered = valid.clone();
        *altered.pointer_mut(pointer).unwrap() = value;
        assert!(
            parse_review(action, "tenant-a", altered).is_err(),
            "divergence at {pointer}"
        );
    }
    for (key, value) in [
        ("name", json!("named-pod")),
        ("labelSelector", json!({})),
        ("user", json!("other-user")),
    ] {
        let mut altered = valid.clone();
        altered["spec"]["resourceAttributes"][key] = value;
        assert!(parse_review(action, "tenant-a", altered).is_err());
    }
}
