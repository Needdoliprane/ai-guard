//! Effect mechanics over a real reservation, journal and rendered request,
//! through the real HTTPS client and one owned raw TLS loopback peer. The
//! peer's Pod bodies and statuses are declared fixtures: they never qualify a
//! Kubernetes API server, admission, scheduling gates or deletion.
use super::*;
use crate::{
    control_fs::PrivateFile,
    runtime::kubernetes::{
        KubernetesCredentials,
        session::tests::fixture_at,
        session_instance::SessionContainerStateV2,
        session_pod::tests::{
            POD_UID, Rendered, admitted, rendered_case, rendered_case_with, replace,
        },
        session_test_peer::{PeerReply, PeerRequest, SessionTestPeer},
    },
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};

const TOKEN: &str = "public-session-effect-loopback-fixture-token-000000000000";
const CHANGED_NAMESPACE_UID: &str = "44444444-4444-4444-8444-444444444444";
const CREATE: &str = "/api/v1/namespaces/tenant-a/pods?fieldValidation=Strict";
const ANCHORS: [&str; 2] = [
    "GET /api/v1/namespaces/kube-system",
    "GET /api/v1/namespaces/tenant-a",
];

struct Case {
    peer: SessionTestPeer,
    rendered: Rendered,
    runtime: KubernetesRuntime,
    namespace_uid: Arc<Mutex<String>>,
}
impl Case {
    /// Real reservation and journal for the peer's endpoint and trust.
    async fn new() -> Self {
        let peer = SessionTestPeer::start().await;
        let ca = PrivateFile::open(peer.ca(), 64 * 1024).unwrap();
        let trust = ca.digest().to_owned();
        let token = peer.ca().parent().unwrap().join("session-effect-token");
        std::fs::write(&token, TOKEN).unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        let rendered = rendered_case_with(fixture_at(peer.endpoint(), &trust));
        assert_eq!(rendered.intent.authority().namespace(), "tenant-a");
        let runtime = KubernetesRuntime::connect(
            rendered.intent.authority().clone(),
            KubernetesCredentials::bearer(ca, PrivateFile::open(&token, 8192).unwrap()),
        )
        .unwrap();
        let namespace_uid = Arc::new(Mutex::new(
            rendered.intent.authority().namespace_uid.clone(),
        ));
        Self {
            peer,
            rendered,
            runtime,
            namespace_uid,
        }
    }
    fn fingerprint(&self) -> String {
        self.rendered.intent.authority().fingerprint()
    }
    fn pod_path(&self) -> String {
        format!(
            "/api/v1/namespaces/tenant-a/pods/{}",
            self.rendered.request.target().pod_name()
        )
    }
    fn guard(&self, kind: SessionEffectKindV2, digest: &str) -> LoopbackEffectGuardV2 {
        LoopbackEffectGuardV2::new(kind, digest, self.peer.listener()).unwrap()
    }
    fn create_guard(&self) -> LoopbackEffectGuardV2 {
        self.guard(
            SessionEffectKindV2::CreateHeld,
            self.rendered.request.target().request_sha256(),
        )
    }
    /// Active anchors (tenant UID from `namespace_uid`), the Pod at its name
    /// (404 when `None`), and `effect` for every other request.
    fn respond(
        &self,
        pod: Option<Value>,
        mut effect: impl FnMut(&PeerRequest) -> PeerReply + Send + 'static,
    ) {
        let cluster_uid = self
            .rendered
            .intent
            .authority()
            .cluster_namespace_uid
            .clone();
        let namespace_uid = self.namespace_uid.clone();
        let pod_path = self.pod_path();
        self.peer.respond(move |request: &PeerRequest| {
            if request.method() != "GET" {
                return effect(request);
            }
            match request.target() {
                "/api/v1/namespaces/kube-system" => namespace_reply("kube-system", &cluster_uid),
                "/api/v1/namespaces/tenant-a" => {
                    namespace_reply("tenant-a", &namespace_uid.lock().unwrap())
                }
                target if target == pod_path => pod
                    .as_ref()
                    .map_or_else(not_found, |pod| PeerReply::json(200, pod)),
                _ => effect(request),
            }
        });
    }
    fn set_namespace_uid(&self, uid: &str) {
        *self.namespace_uid.lock().unwrap() = uid.into();
    }
    fn requests_since(&self, start: usize) -> Vec<PeerRequest> {
        self.peer.requests()[start..].to_vec()
    }
    fn effect_count(&self) -> usize {
        self.peer
            .requests()
            .iter()
            .filter(|request| request.method() != "GET")
            .count()
    }
    async fn create(
        &self,
        guard: LoopbackEffectGuardV2,
        cancelled: bool,
    ) -> Result<SessionCreateOutcomeV2, KubernetesSessionRuntimeError> {
        self.runtime
            .create_session_held(&self.rendered.request, guard, &AtomicBool::new(cancelled))
            .await
    }
    /// A real CreatedHeld instance from a validated 201 of this peer.
    async fn created(&self) -> SessionInstanceV2 {
        self.respond(None, created_201);
        match self.create(self.create_guard(), false).await.unwrap() {
            SessionCreateOutcomeV2::CreatedHeld(instance) => *instance,
            other => panic!("a validated 201 must retain the instance: {other:?}"),
        }
    }
    async fn observe(&self, instance: &SessionInstanceV2, pod: &Value) -> SessionObservedV2 {
        self.respond(Some(pod.clone()), |_: &PeerRequest| not_found());
        self.runtime
            .observe_session_instance(instance)
            .await
            .unwrap()
    }
}

fn namespace_reply(name: &str, uid: &str) -> PeerReply {
    PeerReply::json(
        200,
        &json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":name,"uid":uid},
            "status":{"phase":"Active"}}),
    )
}
fn not_found() -> PeerReply {
    PeerReply::json(404, &json!({"kind":"Status","code":404}))
}
fn status(code: u16) -> PeerReply {
    PeerReply::json(code, &json!({"kind":"Status","code":code}))
}
/// Echo of the received CREATE body with the identity facts of a 201.
fn echo_created(body: &[u8]) -> Value {
    let mut pod: Value = serde_json::from_slice(body).unwrap();
    pod["metadata"]["uid"] = json!(POD_UID);
    pod["metadata"]["resourceVersion"] = json!("42");
    pod
}
fn created_201(request: &PeerRequest) -> PeerReply {
    if request.method() == "POST" && request.target() == CREATE {
        PeerReply::json(201, &echo_created(request.body()))
    } else {
        not_found()
    }
}
fn lines(requests: &[PeerRequest]) -> Vec<String> {
    requests.iter().map(PeerRequest::line).collect()
}
fn without_gate(pod: &Value, resource_version: &str) -> Value {
    let mut pod = pod.clone();
    assert!(
        pod["spec"]
            .as_object_mut()
            .unwrap()
            .remove("schedulingGates")
            .is_some()
    );
    pod["metadata"]["resourceVersion"] = json!(resource_version);
    pod
}
fn running(pod: &Value) -> Value {
    let mut pod = without_gate(pod, "46");
    pod["spec"]["nodeName"] = json!("node-a");
    pod["status"] = json!({"phase":"Running","containerStatuses":[{"name":"worker",
        "restartCount":0,"containerID":"containerd://0123456789abcdef",
        "imageID":"sha256:fedcba9876543210",
        "state":{"running":{"startedAt":"2026-11-12T08:01:00Z"}}}]});
    pod
}
/// Canonical release JSON Patch for `resource_version`, written out by hand.
fn release_body(resource_version: &str) -> String {
    format!(
        "[{{\"op\":\"test\",\"path\":\"/metadata/uid\",\"value\":\"{POD_UID}\"}},\
         {{\"op\":\"test\",\"path\":\"/metadata/resourceVersion\",\"value\":\"{resource_version}\"}},\
         {{\"op\":\"test\",\"path\":\"/spec/schedulingGates\",\"value\":[{{\"name\":\"ai-guard.io/session-v2\"}}]}},\
         {{\"op\":\"remove\",\"path\":\"/spec/schedulingGates\"}}]"
    )
}
/// Canonical DeleteOptions, written out by hand.
fn delete_body(resource_version: Option<&str>) -> String {
    let preconditions = match resource_version {
        Some(version) => format!("{{\"resourceVersion\":\"{version}\",\"uid\":\"{POD_UID}\"}}"),
        None => format!("{{\"uid\":\"{POD_UID}\"}}"),
    };
    format!(
        "{{\"apiVersion\":\"v1\",\"kind\":\"DeleteOptions\",\"preconditions\":{preconditions},\
         \"propagationPolicy\":\"Background\"}}"
    )
}

#[tokio::test]
async fn create_posts_exact_retained_bytes_once_and_only_a_validated_201_retains_an_instance() {
    let case = Case::new().await;
    let request = &case.rendered.request;
    let post = format!("POST {CREATE}");
    let bearer = format!("Bearer {TOKEN}");
    // A validated 201: anchors, exactly one POST of the retained bytes,
    // anchors again, and an instance carrying the server-assigned UID.
    case.respond(None, created_201);
    let SessionCreateOutcomeV2::CreatedHeld(instance) =
        case.create(case.create_guard(), false).await.unwrap()
    else {
        panic!("a validated 201 must retain the instance");
    };
    assert_eq!(instance.pod_uid(), POD_UID);
    assert_eq!(instance.created_resource_version(), "42");
    assert_eq!(instance.target(), request.target());
    let seen = case.requests_since(0);
    assert_eq!(
        lines(&seen),
        [
            ANCHORS[0],
            ANCHORS[1],
            post.as_str(),
            ANCHORS[0],
            ANCHORS[1]
        ]
    );
    assert_eq!(seen[2].body(), request.bytes());
    assert_eq!(seen[2].header("content-type"), Some("application/json"));
    assert_eq!(seen[2].header("authorization"), Some(bearer.as_str()));
    // Any other status is unknown ownership, even with a well-formed Pod
    // body, and is never followed by a second POST.
    for code in [200_u16, 202, 409, 500] {
        let start = case.peer.requests().len();
        case.respond(None, move |request: &PeerRequest| {
            PeerReply::json(code, &echo_created(request.body()))
        });
        let outcome = case.create(case.create_guard(), false).await.unwrap();
        assert!(
            matches!(
                outcome,
                SessionCreateOutcomeV2::OwnershipUnknown {
                    http_status: Some(seen_status),
                    cause: KubernetesSessionRuntimeError::Kubernetes(KubernetesError::ApiRejected(api_status)),
                } if seen_status == code && api_status == code
            ),
            "{code}: {outcome:?}"
        );
        assert_eq!(
            lines(&case.requests_since(start)),
            [ANCHORS[0], ANCHORS[1], post.as_str()],
            "{code}"
        );
    }
    // The full request was received, then the answer was lost.
    let start = case.peer.requests().len();
    case.respond(None, |_: &PeerRequest| PeerReply::CloseWithoutResponse);
    let outcome = case.create(case.create_guard(), false).await.unwrap();
    assert!(
        matches!(
            outcome,
            SessionCreateOutcomeV2::OwnershipUnknown {
                http_status: None,
                cause: KubernetesSessionRuntimeError::Kubernetes(
                    KubernetesError::TransportUnavailable
                ),
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(
        lines(&case.requests_since(start)),
        [ANCHORS[0], ANCHORS[1], post.as_str()]
    );
    // A 201 whose object drifted from the admitted profile, or is not
    // unique JSON, is not an instance.
    for (reply, expected) in [
        (
            PeerReply::Json {
                status: 201,
                body: serde_json::to_vec(&replace(
                    &admitted(request),
                    "/spec/hostNetwork",
                    json!(true),
                ))
                .unwrap(),
            },
            KubernetesSessionRuntimeError::ProfileChanged,
        ),
        (
            PeerReply::Json {
                status: 201,
                body: br#"{"apiVersion":"v1","apiVersion":"v1"}"#.to_vec(),
            },
            KubernetesSessionRuntimeError::Kubernetes(KubernetesError::InvalidResponse),
        ),
    ] {
        let start = case.peer.requests().len();
        let mut reply = Some(reply);
        case.respond(None, move |_: &PeerRequest| {
            reply.take().unwrap_or_else(not_found)
        });
        let outcome = case.create(case.create_guard(), false).await.unwrap();
        assert!(
            matches!(
                outcome,
                SessionCreateOutcomeV2::OwnershipUnknown { http_status: Some(201), cause }
                    if cause == expected
            ),
            "{expected:?}: {outcome:?}"
        );
        assert_eq!(
            lines(&case.requests_since(start)),
            [ANCHORS[0], ANCHORS[1], post.as_str()]
        );
    }
    // A valid 201 followed by a replaced namespace is detected afterwards
    // and stays unknown: it never becomes a retained instance.
    let start = case.peer.requests().len();
    let namespace_uid = case.namespace_uid.clone();
    case.respond(None, move |request: &PeerRequest| {
        *namespace_uid.lock().unwrap() = CHANGED_NAMESPACE_UID.into();
        created_201(request)
    });
    let outcome = case.create(case.create_guard(), false).await.unwrap();
    assert!(
        matches!(
            outcome,
            SessionCreateOutcomeV2::OwnershipUnknown {
                http_status: Some(201),
                cause: KubernetesSessionRuntimeError::Kubernetes(KubernetesError::AuthorityChanged),
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(
        lines(&case.requests_since(start)),
        [
            ANCHORS[0],
            ANCHORS[1],
            post.as_str(),
            ANCHORS[0],
            ANCHORS[1]
        ]
    );
    // One POST per call, always the exact retained bytes: 1 + 4 + 1 + 2 + 1.
    let posts: Vec<PeerRequest> = case
        .peer
        .requests()
        .into_iter()
        .filter(|request| request.method() == "POST")
        .collect();
    assert_eq!(posts.len(), 9);
    assert!(
        posts
            .iter()
            .all(|seen_post| seen_post.body() == request.bytes()
                && seen_post.target() == CREATE
                && seen_post.header("authorization") == Some(bearer.as_str()))
    );
}

#[tokio::test]
async fn release_patch_carries_uid_rv_and_gate_tests_from_the_exact_observation() {
    let case = Case::new().await;
    let instance = case.created().await;
    let patch_target = format!("{}?fieldValidation=Strict", case.pod_path());
    let patch = format!("PATCH {patch_target}");
    // The Held observation has a newer resourceVersion than the CREATE.
    let held = replace(
        &admitted(&case.rendered.request),
        "/metadata/resourceVersion",
        json!("44"),
    );
    let observed = case.observe(&instance, &held).await;
    assert_eq!(observed.state(), &SessionObservationStateV2::Held);
    assert_eq!(observed.resource_version(), "44");
    let body = release_body("44");
    let digest = effect_request_sha256(
        SessionEffectKindV2::ReleaseGate,
        &case.fingerprint(),
        "PATCH",
        &patch_target,
        body.as_bytes(),
    );
    let released = without_gate(&held, "45");
    let cases = [
        (
            PeerReply::json(200, &released),
            SessionReleaseOutcomeV2::Released {
                resource_version: "45".into(),
            },
            true,
        ),
        (
            status(409),
            SessionReleaseOutcomeV2::RefusedNotApplied { status: 409 },
            true,
        ),
        (
            status(422),
            SessionReleaseOutcomeV2::RefusedNotApplied { status: 422 },
            true,
        ),
        // A 200 whose body is not the Pod, or that still shows the gate, is
        // not a validated release.
        (
            PeerReply::json(200, &without_gate(&held, "45")["metadata"]),
            SessionReleaseOutcomeV2::Uncertain {
                http_status: Some(200),
                cause: KubernetesSessionRuntimeError::Kubernetes(KubernetesError::InvalidResponse),
            },
            false,
        ),
        (
            PeerReply::json(
                200,
                &replace(&held, "/metadata/resourceVersion", json!("45")),
            ),
            SessionReleaseOutcomeV2::Uncertain {
                http_status: Some(200),
                cause: KubernetesSessionRuntimeError::Kubernetes(KubernetesError::InvalidResponse),
            },
            false,
        ),
        (
            status(500),
            SessionReleaseOutcomeV2::Uncertain {
                http_status: Some(500),
                cause: KubernetesSessionRuntimeError::Kubernetes(KubernetesError::ApiRejected(500)),
            },
            false,
        ),
        (
            PeerReply::CloseWithoutResponse,
            SessionReleaseOutcomeV2::Uncertain {
                http_status: None,
                cause: KubernetesSessionRuntimeError::Kubernetes(
                    KubernetesError::TransportUnavailable,
                ),
            },
            false,
        ),
    ];
    for (reply, expected, rechecked) in cases {
        let start = case.peer.requests().len();
        let mut reply = Some(reply);
        case.respond(Some(held.clone()), move |_: &PeerRequest| {
            reply.take().unwrap_or_else(not_found)
        });
        let outcome = case
            .runtime
            .release_session_gate(
                &instance,
                &observed,
                case.guard(SessionEffectKindV2::ReleaseGate, &digest),
                &AtomicBool::new(false),
            )
            .await
            .unwrap();
        assert_eq!(outcome, expected);
        let seen = case.requests_since(start);
        let mut expected_lines = vec![ANCHORS[0], ANCHORS[1], patch.as_str()];
        if rechecked {
            expected_lines.extend(ANCHORS);
        }
        assert_eq!(lines(&seen), expected_lines, "{expected:?}");
        assert_eq!(seen[2].body(), body.as_bytes());
        assert_eq!(
            seen[2].header("content-type"),
            Some("application/json-patch+json")
        );
        let ops: Value = serde_json::from_slice(seen[2].body()).unwrap();
        assert_eq!(
            ops,
            json!([
                {"op":"test","path":"/metadata/uid","value":POD_UID},
                {"op":"test","path":"/metadata/resourceVersion","value":"44"},
                {"op":"test","path":"/spec/schedulingGates","value":[{"name":"ai-guard.io/session-v2"}]},
                {"op":"remove","path":"/spec/schedulingGates"}
            ])
        );
    }
    // A guard bound to the CREATE resourceVersion instead of the observed one
    // refuses the PATCH this code builds: the patch uses the observation.
    let start = case.peer.requests().len();
    let stale = effect_request_sha256(
        SessionEffectKindV2::ReleaseGate,
        &case.fingerprint(),
        "PATCH",
        &patch_target,
        release_body(instance.created_resource_version()).as_bytes(),
    );
    assert_eq!(
        case.runtime
            .release_session_gate(
                &instance,
                &observed,
                case.guard(SessionEffectKindV2::ReleaseGate, &stale),
                &AtomicBool::new(false),
            )
            .await
            .err(),
        Some(KubernetesSessionRuntimeError::EffectRefusedBeforeSend)
    );
    assert_eq!(lines(&case.requests_since(start)), ANCHORS);
    // Only a Held observation can be released; nothing is sent otherwise.
    for (pod, expected) in [
        (
            without_gate(&held, "47"),
            KubernetesSessionRuntimeError::EffectRefusedBeforeSend,
        ),
        (
            replace(
                &held,
                "/metadata/deletionTimestamp",
                json!("2026-11-12T08:05:00Z"),
            ),
            KubernetesSessionRuntimeError::InstanceDeleting,
        ),
    ] {
        let other = case.observe(&instance, &pod).await;
        assert_ne!(other.state(), &SessionObservationStateV2::Held);
        let start = case.peer.requests().len();
        assert_eq!(
            case.runtime
                .release_session_gate(
                    &instance,
                    &other,
                    case.guard(SessionEffectKindV2::ReleaseGate, &digest),
                    &AtomicBool::new(false),
                )
                .await
                .err(),
            Some(expected)
        );
        assert!(case.requests_since(start).is_empty());
    }
    // Exactly one PATCH per release call that reached the transport.
    let patches = case
        .peer
        .requests()
        .iter()
        .filter(|request| request.method() == "PATCH")
        .count();
    assert_eq!(patches, 7);
}

#[tokio::test]
async fn delete_requires_uid_precondition_and_never_reports_terminal() {
    let case = Case::new().await;
    let instance = case.created().await;
    let pod_path = case.pod_path();
    let delete = format!("DELETE {pod_path}");
    let held = replace(
        &admitted(&case.rendered.request),
        "/metadata/resourceVersion",
        json!("44"),
    );
    let deleting = replace(
        &held,
        "/metadata/deletionTimestamp",
        json!("2026-11-12T08:05:00Z"),
    );
    let digest_for = |body: &str| {
        effect_request_sha256(
            SessionEffectKindV2::DeleteExact,
            &case.fingerprint(),
            "DELETE",
            &pod_path,
            body.as_bytes(),
        )
    };
    // From a Held observation: UID and resourceVersion preconditions.
    let observed_held = case.observe(&instance, &held).await;
    assert_eq!(observed_held.state(), &SessionObservationStateV2::Held);
    let held_body = delete_body(Some("44"));
    let start = case.peer.requests().len();
    let reply_deleting = deleting.clone();
    case.respond(Some(held.clone()), move |_: &PeerRequest| {
        PeerReply::json(200, &reply_deleting)
    });
    assert_eq!(
        case.runtime
            .delete_session_exact(
                &instance,
                &observed_held,
                case.guard(SessionEffectKindV2::DeleteExact, &digest_for(&held_body)),
                &AtomicBool::new(false),
            )
            .await
            .unwrap(),
        SessionDeleteOutcomeV2::DeletionRequested
    );
    let seen = case.requests_since(start);
    assert_eq!(
        lines(&seen),
        [
            ANCHORS[0],
            ANCHORS[1],
            delete.as_str(),
            ANCHORS[0],
            ANCHORS[1]
        ]
    );
    assert_eq!(seen[2].body(), held_body.as_bytes());
    assert_eq!(seen[2].header("content-type"), Some("application/json"));
    // An accepted deletion is not terminal: the same instance is observed
    // again, as Deleting.
    assert!(matches!(
        case.observe(&instance, &deleting).await.state(),
        SessionObservationStateV2::Deleting {
            container: SessionContainerStateV2::Absent
        }
    ));
    // From a non-Held observation the resourceVersion is not bound; the
    // UID precondition always is.
    let running_pod = running(&held);
    let observed_running = case.observe(&instance, &running_pod).await;
    assert!(matches!(
        observed_running.state(),
        SessionObservationStateV2::Scheduled { .. }
    ));
    let uid_body = delete_body(None);
    let cases = [
        (status(202), SessionDeleteOutcomeV2::DeletionRequested, true),
        (status(404), SessionDeleteOutcomeV2::NotFound, true),
        (
            status(409),
            SessionDeleteOutcomeV2::PreconditionFailed,
            true,
        ),
        (
            status(500),
            SessionDeleteOutcomeV2::Uncertain {
                http_status: Some(500),
                cause: KubernetesSessionRuntimeError::Kubernetes(KubernetesError::ApiRejected(500)),
            },
            false,
        ),
        (
            PeerReply::CloseWithoutResponse,
            SessionDeleteOutcomeV2::Uncertain {
                http_status: None,
                cause: KubernetesSessionRuntimeError::Kubernetes(
                    KubernetesError::TransportUnavailable,
                ),
            },
            false,
        ),
    ];
    for (reply, expected, rechecked) in cases {
        let start = case.peer.requests().len();
        let mut reply = Some(reply);
        case.respond(Some(running_pod.clone()), move |_: &PeerRequest| {
            reply.take().unwrap_or_else(not_found)
        });
        let outcome = case
            .runtime
            .delete_session_exact(
                &instance,
                &observed_running,
                case.guard(SessionEffectKindV2::DeleteExact, &digest_for(&uid_body)),
                &AtomicBool::new(false),
            )
            .await
            .unwrap();
        assert_eq!(outcome, expected);
        let seen = case.requests_since(start);
        let mut expected_lines = vec![ANCHORS[0], ANCHORS[1], delete.as_str()];
        if rechecked {
            expected_lines.extend(ANCHORS);
        }
        assert_eq!(lines(&seen), expected_lines, "{expected:?}");
        assert_eq!(seen[2].body(), uid_body.as_bytes());
    }
    // A 404 from DELETE is an answer, not a terminal: a later GET of the
    // name stays unknown (ApiRejected(404)) for the same instance.
    case.respond(None, |_: &PeerRequest| not_found());
    assert_eq!(
        case.runtime.observe_session_instance(&instance).await.err(),
        Some(KubernetesSessionRuntimeError::Kubernetes(
            KubernetesError::ApiRejected(404)
        ))
    );
    let deletes: Vec<PeerRequest> = case
        .peer
        .requests()
        .into_iter()
        .filter(|request| request.method() == "DELETE")
        .collect();
    assert_eq!(deletes.len(), 6);
    for request in &deletes {
        let body = std::str::from_utf8(request.body()).unwrap();
        assert!(!body.contains("gracePeriodSeconds"), "{body}");
        assert!(body.contains("\"propagationPolicy\":\"Background\""));
        assert!(body.contains(&format!("\"uid\":\"{POD_UID}\"")));
    }
}

#[tokio::test]
async fn refused_guard_cancellation_or_changed_anchor_sends_zero_effect_requests() {
    let case = Case::new().await;
    let instance = case.created().await;
    assert_eq!(case.effect_count(), 1);
    let request = &case.rendered.request;
    let authority = case.rendered.intent.authority();
    let held = admitted(request);
    let observed = case.observe(&instance, &held).await;
    let patch_target = format!("{}?fieldValidation=Strict", case.pod_path());
    let release_digest = effect_request_sha256(
        SessionEffectKindV2::ReleaseGate,
        &case.fingerprint(),
        "PATCH",
        &patch_target,
        release_body("42").as_bytes(),
    );
    let delete_digest = effect_request_sha256(
        SessionEffectKindV2::DeleteExact,
        &case.fingerprint(),
        "DELETE",
        &case.pod_path(),
        delete_body(Some("42")).as_bytes(),
    );
    // Any effect that slipped through would be answered and counted.
    case.respond(Some(held.clone()), |_: &PeerRequest| status(200));
    let refused = Some(KubernetesSessionRuntimeError::EffectRefusedBeforeSend);
    let foreign_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let foreign = |kind: SessionEffectKindV2, digest: &str| {
        LoopbackEffectGuardV2::new(kind, digest, &foreign_listener).unwrap()
    };

    // The guard itself: exact listener, kind, digest and no cancellation.
    let exact = "e".repeat(64);
    let not_cancelled = AtomicBool::new(false);
    assert_eq!(
        case.guard(SessionEffectKindV2::Attach, &exact)
            .check_before_send(
                SessionEffectKindV2::Attach,
                &exact,
                authority,
                &not_cancelled
            ),
        Ok(())
    );
    let elsewhere = KubernetesAuthorityV1::new(
        authority.authority_id(),
        "https://127.0.0.1:1",
        &authority.cluster_namespace_uid,
        authority.namespace(),
        &authority.namespace_uid,
        &authority.trust_sha256,
    )
    .unwrap();
    for (guard, kind, digest, authority, cancelled) in [
        (
            foreign(SessionEffectKindV2::Attach, &exact),
            SessionEffectKindV2::Attach,
            exact.as_str(),
            authority,
            false,
        ),
        (
            case.guard(SessionEffectKindV2::Attach, &exact),
            SessionEffectKindV2::Attach,
            exact.as_str(),
            &elsewhere,
            false,
        ),
        (
            case.guard(SessionEffectKindV2::Attach, &exact),
            SessionEffectKindV2::DeleteExact,
            exact.as_str(),
            authority,
            false,
        ),
        (
            case.guard(SessionEffectKindV2::Attach, &exact),
            SessionEffectKindV2::Attach,
            release_digest.as_str(),
            authority,
            false,
        ),
        (
            case.guard(SessionEffectKindV2::Attach, &exact),
            SessionEffectKindV2::Attach,
            exact.as_str(),
            authority,
            true,
        ),
    ] {
        assert_eq!(
            guard
                .check_before_send(kind, digest, authority, &AtomicBool::new(cancelled))
                .err(),
            refused
        );
    }

    // CREATE: another listener, another kind or digest are refused by the
    // guard after the anchors; a cancellation is refused before any request.
    for guard in [
        foreign(
            SessionEffectKindV2::CreateHeld,
            request.target().request_sha256(),
        ),
        case.guard(
            SessionEffectKindV2::ReleaseGate,
            request.target().request_sha256(),
        ),
        case.guard(SessionEffectKindV2::CreateHeld, &exact),
    ] {
        let start = case.peer.requests().len();
        assert_eq!(case.create(guard, false).await.err(), refused);
        assert_eq!(lines(&case.requests_since(start)), ANCHORS);
    }
    let start = case.peer.requests().len();
    assert_eq!(case.create(case.create_guard(), true).await.err(), refused);
    assert!(case.requests_since(start).is_empty());

    // A cancellation raised while the anchors are read, after the entry
    // check, is refused by the transport itself before any effect request.
    // The guard used here ignores the flag (control just below), so only the
    // transport check can refuse it: without that check the CREATE would be
    // sent and answered.
    let ignoring = || {
        LoopbackEffectGuardV2::ignoring_cancellation(
            SessionEffectKindV2::CreateHeld,
            request.target().request_sha256(),
            case.peer.listener(),
        )
        .unwrap()
    };
    assert_eq!(
        ignoring().check_before_send(
            SessionEffectKindV2::CreateHeld,
            request.target().request_sha256(),
            authority,
            &AtomicBool::new(true),
        ),
        Ok(())
    );
    let raised = Arc::new(AtomicBool::new(false));
    let raise = raised.clone();
    let cluster_uid = authority.cluster_namespace_uid.clone();
    let namespace_uid = authority.namespace_uid.clone();
    case.peer.respond(
        move |incoming: &PeerRequest| match (incoming.method(), incoming.target()) {
            ("GET", "/api/v1/namespaces/kube-system") => {
                raise.store(true, Ordering::Release);
                namespace_reply("kube-system", &cluster_uid)
            }
            ("GET", "/api/v1/namespaces/tenant-a") => namespace_reply("tenant-a", &namespace_uid),
            _ => status(200),
        },
    );
    let start = case.peer.requests().len();
    assert_eq!(
        case.runtime
            .create_session_held(request, ignoring(), &raised)
            .await
            .err(),
        refused
    );
    assert!(raised.load(Ordering::Acquire));
    assert_eq!(lines(&case.requests_since(start)), ANCHORS);
    case.respond(Some(held.clone()), |_: &PeerRequest| status(200));

    // Release and delete: the same refusals for the exact instance.
    let cancelled = AtomicBool::new(true);
    for (guard, flag) in [
        (
            foreign(SessionEffectKindV2::ReleaseGate, &release_digest),
            &not_cancelled,
        ),
        (
            case.guard(SessionEffectKindV2::ReleaseGate, &release_digest),
            &cancelled,
        ),
    ] {
        assert_eq!(
            case.runtime
                .release_session_gate(&instance, &observed, guard, flag)
                .await
                .err(),
            refused
        );
    }
    for (guard, flag) in [
        (
            foreign(SessionEffectKindV2::DeleteExact, &delete_digest),
            &not_cancelled,
        ),
        (
            case.guard(SessionEffectKindV2::DeleteExact, &delete_digest),
            &cancelled,
        ),
    ] {
        assert_eq!(
            case.runtime
                .delete_session_exact(&instance, &observed, guard, flag)
                .await
                .err(),
            refused
        );
    }

    // A replaced namespace refuses every effect before its request, even
    // with an exact guard.
    case.set_namespace_uid(CHANGED_NAMESPACE_UID);
    let changed = Some(KubernetesSessionRuntimeError::Kubernetes(
        KubernetesError::AuthorityChanged,
    ));
    let start = case.peer.requests().len();
    assert_eq!(case.create(case.create_guard(), false).await.err(), changed);
    assert_eq!(
        case.runtime
            .release_session_gate(
                &instance,
                &observed,
                case.guard(SessionEffectKindV2::ReleaseGate, &release_digest),
                &not_cancelled,
            )
            .await
            .err(),
        changed
    );
    assert_eq!(
        case.runtime
            .delete_session_exact(
                &instance,
                &observed,
                case.guard(SessionEffectKindV2::DeleteExact, &delete_digest),
                &not_cancelled,
            )
            .await
            .err(),
        changed
    );
    let anchors_three_times: Vec<&str> = ANCHORS.iter().copied().cycle().take(6).collect();
    assert_eq!(lines(&case.requests_since(start)), anchors_three_times);

    // A request rendered for another authority is refused before any request.
    let other = rendered_case();
    let start = case.peer.requests().len();
    assert_eq!(
        case.runtime
            .create_session_held(
                &other.request,
                case.guard(
                    SessionEffectKindV2::CreateHeld,
                    other.request.target().request_sha256(),
                ),
                &not_cancelled,
            )
            .await
            .err(),
        Some(KubernetesSessionRuntimeError::Kubernetes(
            KubernetesError::InvalidAuthority
        ))
    );
    assert!(case.requests_since(start).is_empty());
    // Only the initial validated CREATE ever reached the peer, and every
    // accepted connection carried a parsed, recorded request: no unparsable
    // effect request went uncounted.
    assert_eq!(case.effect_count(), 1);
    assert_eq!(case.peer.accepted(), case.peer.requests().len());
}

#[test]
fn effect_request_digest_binds_method_path_and_body() {
    let fingerprint = "a".repeat(64);
    let path = "/api/v1/namespaces/tenant-a/pods/ai-guard-session-x?fieldValidation=Strict";
    let body: &[u8] = br#"[{"op":"remove","path":"/spec/schedulingGates"}]"#;
    // Recomputed here from the documented domain and separators.
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:kubernetes-session-effect-request:v2\0release_gate\0");
    hash.update(fingerprint.as_bytes());
    hash.update(b"\0PATCH ");
    hash.update(path.as_bytes());
    hash.update(b"\0");
    hash.update(body);
    let expected = hex::encode(hash.finalize());
    let release = SessionEffectKindV2::ReleaseGate;
    assert_eq!(
        effect_request_sha256(release, &fingerprint, "PATCH", path, body),
        expected
    );
    let (path_head, path_tail) = path.split_at(path.len() - 1);
    let mut other_body = body.to_vec();
    other_body[2] ^= 1;
    let other_fingerprint = format!("b{}", &fingerprint[1..]);
    let moved_body = [path_tail.as_bytes(), body].concat();
    let variants = [
        expected.clone(),
        effect_request_sha256(release, &fingerprint, "PATCX", path, body),
        effect_request_sha256(
            release,
            &fingerprint,
            "PATCH",
            &format!("{path_head}X"),
            body,
        ),
        effect_request_sha256(release, &fingerprint, "PATCH", path, &other_body),
        effect_request_sha256(release, &other_fingerprint, "PATCH", path, body),
        // One byte moved across the path/body separator.
        effect_request_sha256(release, &fingerprint, "PATCH", path_head, &moved_body),
        effect_request_sha256(
            SessionEffectKindV2::CreateHeld,
            &fingerprint,
            "PATCH",
            path,
            body,
        ),
        effect_request_sha256(
            SessionEffectKindV2::Attach,
            &fingerprint,
            "PATCH",
            path,
            body,
        ),
        effect_request_sha256(
            SessionEffectKindV2::DeleteExact,
            &fingerprint,
            "PATCH",
            path,
            body,
        ),
    ];
    assert!(variants.iter().all(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }));
    let distinct: BTreeSet<&String> = variants.iter().collect();
    assert_eq!(distinct.len(), variants.len());
}

// Pre-S5 source regressions: real retained HTTPS transport, declared loopback
// server replies, no production effect guard or cluster qualification.
#[tokio::test]
async fn create_201_response_witness_is_affine_and_bound_to_the_original_target() {
    for original_target in [true, false] {
        let case = Case::new().await;
        case.respond(None, created_201);
        let reply = case
            .runtime
            .transport
            .send_session_effect(
                &case.runtime.binding,
                SessionHttpEffectV2::CreateHeld {
                    request: &case.rendered.request,
                },
                case.create_guard(),
                &AtomicBool::new(false),
            )
            .await
            .unwrap();
        let response = match reply {
            SessionHttpResponseV2::CreatedResponse(response) => response,
            other => panic!("actual bounded POST201 response missing: {other:?}"),
        };
        if original_target {
            let instance =
                validate_admitted_session_pod(&case.rendered.request, *response).unwrap();
            assert_eq!(instance.target(), case.rendered.request.target());
            assert_eq!(instance.pod_uid(), POD_UID);
        } else {
            let other = rendered_case();
            assert_ne!(other.request.target(), case.rendered.request.target());
            assert_eq!(
                validate_admitted_session_pod(&other.request, *response).err(),
                Some(KubernetesSessionRuntimeError::Kubernetes(
                    KubernetesError::InvalidResponse
                )),
            );
        }
        assert_eq!(case.effect_count(), 1);
        assert_eq!(case.peer.requests()[0].line(), format!("POST {CREATE}"));
        assert_eq!(
            case.peer.requests()[0].body(),
            case.rendered.request.bytes()
        );
    }
}

#[tokio::test]
async fn non_201_or_non_create_http_reply_never_produces_a_create_response_witness() {
    let case = Case::new().await;
    let body = admitted(&case.rendered.request);
    case.respond(None, move |_: &PeerRequest| PeerReply::json(200, &body));
    let reply = case
        .runtime
        .transport
        .send_session_effect(
            &case.runtime.binding,
            SessionHttpEffectV2::CreateHeld {
                request: &case.rendered.request,
            },
            case.create_guard(),
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
    assert!(matches!(
        reply,
        SessionHttpResponseV2::Complete { status: 200, .. }
    ));

    let body = admitted(&case.rendered.request);
    case.respond(None, move |_: &PeerRequest| PeerReply::json(201, &body));
    let deletion = delete_body(Some("42"));
    let digest = effect_request_sha256(
        SessionEffectKindV2::DeleteExact,
        &case.fingerprint(),
        "DELETE",
        &case.pod_path(),
        deletion.as_bytes(),
    );
    let reply = case
        .runtime
        .transport
        .send_session_effect(
            &case.runtime.binding,
            SessionHttpEffectV2::DeleteExact {
                name: case.rendered.request.target().pod_name(),
                body: deletion.into_bytes(),
            },
            case.guard(SessionEffectKindV2::DeleteExact, &digest),
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
    assert!(matches!(
        reply,
        SessionHttpResponseV2::Complete { status: 201, .. }
    ));
    assert_eq!(case.effect_count(), 2);
}

fn terminated(pod: &Value) -> Value {
    let mut pod = running(pod);
    pod["status"]["phase"] = json!("Succeeded");
    pod["status"]["containerStatuses"][0]["state"] = json!({"terminated":{"exitCode":0,
        "reason":"Completed","startedAt":"2026-11-12T08:01:00Z","finishedAt":"2026-11-12T08:02:00Z"}});
    pod
}

#[tokio::test]
async fn finalizer_release_needs_an_observed_terminal_and_tests_uid_and_finalizers() {
    let case = Case::new().await;
    let instance = case.created().await;
    let admitted_pod = admitted(&case.rendered.request);
    let patch_target = format!("{}?fieldValidation=Strict", case.pod_path());
    // Written out by hand: canonical JSON Patch.
    let body = format!(
        "[{{\"op\":\"test\",\"path\":\"/metadata/uid\",\"value\":\"{POD_UID}\"}},\
         {{\"op\":\"test\",\"path\":\"/metadata/finalizers\",\"value\":[\"ai-guard.io/session-terminal\"]}},\
         {{\"op\":\"remove\",\"path\":\"/metadata/finalizers\"}}]"
    );
    let digest = effect_request_sha256(
        SessionEffectKindV2::ReleaseFinalizer,
        &case.fingerprint(),
        "PATCH",
        &patch_target,
        body.as_bytes(),
    );
    // Held, released-unscheduled and running observations refuse before
    // any request: the terminal is not observed.
    for pod in [
        admitted_pod.clone(),
        without_gate(&admitted_pod, "45"),
        running(&admitted_pod),
    ] {
        let observed = case.observe(&instance, &pod).await;
        let before = case.effect_count();
        assert_eq!(
            case.runtime
                .release_session_finalizer(
                    &instance,
                    &observed,
                    case.guard(SessionEffectKindV2::ReleaseFinalizer, &digest),
                    &AtomicBool::new(false),
                )
                .await
                .err(),
            Some(KubernetesSessionRuntimeError::EffectRefusedBeforeSend)
        );
        assert_eq!(case.effect_count(), before);
    }
    let done = terminated(&admitted_pod);
    let mut released = done.clone();
    released["metadata"]["resourceVersion"] = json!("50");
    let still_held = replace(
        &done,
        "/metadata/finalizers",
        json!(["ai-guard.io/session-terminal"]),
    );
    for (reply, expected) in [
        (
            PeerReply::json(200, &released),
            SessionFinalizerOutcomeV2::Released,
        ),
        (
            status(409),
            SessionFinalizerOutcomeV2::RefusedNotApplied { status: 409 },
        ),
        (
            status(422),
            SessionFinalizerOutcomeV2::RefusedNotApplied { status: 422 },
        ),
        (status(404), SessionFinalizerOutcomeV2::NotFound),
        (
            PeerReply::json(200, &still_held),
            SessionFinalizerOutcomeV2::Uncertain {
                http_status: Some(200),
                cause: KubernetesSessionRuntimeError::Kubernetes(KubernetesError::InvalidResponse),
            },
        ),
    ] {
        let observed = case.observe(&instance, &done).await;
        let mut reply = Some(reply);
        case.respond(Some(done.clone()), move |_: &PeerRequest| {
            reply.take().unwrap_or_else(not_found)
        });
        let start = case.peer.requests().len();
        let outcome = case
            .runtime
            .release_session_finalizer(
                &instance,
                &observed,
                case.guard(SessionEffectKindV2::ReleaseFinalizer, &digest),
                &AtomicBool::new(false),
            )
            .await
            .unwrap();
        assert_eq!(outcome, expected);
        let seen = case.requests_since(start);
        let patch = format!("PATCH {patch_target}");
        assert_eq!(lines(&seen)[..3], [ANCHORS[0], ANCHORS[1], patch.as_str()]);
        assert_eq!(seen[2].body(), body.as_bytes());
        assert_eq!(
            seen[2].header("content-type"),
            Some("application/json-patch+json")
        );
    }
}

#[tokio::test]
async fn finalizer_release_is_allowed_for_a_deleted_pod_that_was_never_released() {
    let case = Case::new().await;
    let instance = case.created().await;
    let deleting_held = replace(
        &admitted(&case.rendered.request),
        "/metadata/deletionTimestamp",
        json!("2026-11-12T08:05:00Z"),
    );
    let observed = case.observe(&instance, &deleting_held).await;
    assert!(observed.gated());
    let patch_target = format!("{}?fieldValidation=Strict", case.pod_path());
    let body = canonical(&json!([
        {"op":"test","path":"/metadata/uid","value":POD_UID},
        {"op":"test","path":"/metadata/finalizers","value":["ai-guard.io/session-terminal"]},
        {"op":"remove","path":"/metadata/finalizers"}
    ]))
    .unwrap();
    let digest = effect_request_sha256(
        SessionEffectKindV2::ReleaseFinalizer,
        &case.fingerprint(),
        "PATCH",
        &patch_target,
        &body,
    );
    let mut released = deleting_held.clone();
    released["metadata"]["resourceVersion"] = json!("51");
    case.respond(Some(deleting_held.clone()), move |_: &PeerRequest| {
        PeerReply::json(200, &released)
    });
    assert_eq!(
        case.runtime
            .release_session_finalizer(
                &instance,
                &observed,
                case.guard(SessionEffectKindV2::ReleaseFinalizer, &digest),
                &AtomicBool::new(false),
            )
            .await
            .unwrap(),
        SessionFinalizerOutcomeV2::Released
    );
    // A deleting Pod that was released and bound to a node, with no
    // container status yet, may still be starting: refused.
    let mut deleting_bound = without_gate(&admitted(&case.rendered.request), "52");
    deleting_bound["spec"]["nodeName"] = json!("node-a");
    deleting_bound["metadata"]["deletionTimestamp"] = json!("2026-11-12T08:05:00Z");
    let observed = case.observe(&instance, &deleting_bound).await;
    assert!(!observed.gated());
    let before = case.effect_count();
    assert_eq!(
        case.runtime
            .release_session_finalizer(
                &instance,
                &observed,
                case.guard(SessionEffectKindV2::ReleaseFinalizer, &digest),
                &AtomicBool::new(false),
            )
            .await
            .err(),
        Some(KubernetesSessionRuntimeError::EffectRefusedBeforeSend)
    );
    assert_eq!(case.effect_count(), before);
}
