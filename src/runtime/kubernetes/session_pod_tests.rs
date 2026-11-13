//! Pure renderer and admitted-response contracts over real reservations,
//! journals and claims. No HTTP, cluster or model call. The admitted bodies are
//! declared fixtures (EXT-14): they never qualify a real API server encoding.
use super::*;
use crate::{
    audit::{
        kubernetes_admission_state_tests::Fixture,
        kubernetes_session::ConfirmedSessionReservationV2,
    },
    provider::{BOOTSTRAP_ENV, BOOTSTRAP_PROGRAM},
    runtime::kubernetes::{
        KubernetesAuthorityV1,
        session::tests::{fixture, prepared_binding},
        session_instance::{
            KubernetesSessionRuntimeError, parse_workspace_claim,
            tests::{CLAIM_UID, OBSERVED_AT, bound_claim, bound_pvc},
            validate_admitted_session_pod,
        },
        session_profile::KubernetesSessionInvocationV2,
    },
};
use std::collections::BTreeSet;

pub(crate) const POD_UID: &str = "33333333-3333-4333-8333-333333333333";
/// Core/v1 pointer bools rendered by the V2 profile: an explicit false is
/// kept by the API encoder (declared external fact EXT-14).
const POINTER_BOOLS: [&str; 7] = [
    "automountServiceAccountToken",
    "enableServiceLinks",
    "shareProcessNamespace",
    "runAsNonRoot",
    "privileged",
    "allowPrivilegeEscalation",
    "readOnlyRootFilesystem",
];
/// Non-pointer bools whose omitted false the shared admission validator
/// restores (preflight.rs); every other omitted false would be a mismatch.
const RESTORED: [(&str, &str); 5] = [
    ("/spec", "hostNetwork"),
    ("/spec", "hostPID"),
    ("/spec", "hostIPC"),
    ("/spec/containers/0", "stdin"),
    ("/spec/containers/0", "tty"),
];

pub(crate) struct Rendered {
    pub(crate) _fixture: Fixture,
    pub(crate) intent: KubernetesSessionReservationIntentV2,
    pub(crate) binding: SessionBootstrapBindingV2,
    pub(crate) claim: KubernetesSessionWorkspaceClaimV2,
    pub(crate) request: KubernetesSessionPodRequestV2,
}
/// Real reservation, real confirmed journal, claim named for its attempt.
pub(crate) fn rendered_case_with(
    (fixture, intent, confirmed): (
        Fixture,
        KubernetesSessionReservationIntentV2,
        ConfirmedSessionReservationV2,
    ),
) -> Rendered {
    let binding = prepared_binding(&fixture, &intent, &confirmed);
    let claim = bound_claim(&intent, CLAIM_UID);
    let request = KubernetesSessionPodRequestV2::render(&intent, &binding, &claim).unwrap();
    Rendered {
        _fixture: fixture,
        intent,
        binding,
        claim,
        request,
    }
}
pub(crate) fn rendered_case() -> Rendered {
    rendered_case_with(fixture())
}
/// Exact echo of the request with the server identity facts of a 201.
pub(crate) fn admitted(request: &KubernetesSessionPodRequestV2) -> Value {
    let mut pod: Value = serde_json::from_slice(request.bytes()).unwrap();
    pod["metadata"]["uid"] = json!(POD_UID);
    pod["metadata"]["resourceVersion"] = json!("42");
    pod
}
pub(crate) fn replace(value: &Value, pointer: &str, replacement: Value) -> Value {
    let mut changed = value.clone();
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    changed
        .pointer_mut(parent)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(key.into(), replacement);
    changed
}
pub(crate) fn collect_keys(value: &Value, keys: &mut BTreeSet<String>) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                keys.insert(key.clone());
                collect_keys(value, keys);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_keys(value, keys);
            }
        }
        _ => {}
    }
}
fn collect_false(value: &Value, path: &str, output: &mut Vec<String>) {
    match value {
        Value::Bool(false) => output.push(path.to_owned()),
        Value::Object(fields) => {
            for (key, value) in fields {
                collect_false(value, &format!("{path}/{key}"), output);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                collect_false(value, &format!("{path}/{index}"), output);
            }
        }
        _ => {}
    }
}
fn pod_of(request: &KubernetesSessionPodRequestV2) -> Value {
    serde_json::from_slice(request.bytes()).unwrap()
}
/// Documented API server additions on CREATE (declared fixture, EXT-14).
fn with_server_defaults(mut pod: Value) -> Value {
    pod["metadata"]["creationTimestamp"] = json!("2026-11-12T08:00:00Z");
    pod["metadata"]["generation"] = json!(1);
    pod["metadata"]["managedFields"] = json!([{"manager":"agentsandbox","operation":"Update"}]);
    let spec = pod["spec"].as_object_mut().unwrap();
    spec.insert("serviceAccount".into(), json!("default"));
    spec.insert("preemptionPolicy".into(), json!("PreemptLowerPriority"));
    spec.insert("priority".into(), json!(0));
    spec.insert(
        "tolerations".into(),
        json!([
            {"key":"node.kubernetes.io/not-ready","operator":"Exists","effect":"NoExecute","tolerationSeconds":300},
            {"key":"node.kubernetes.io/unreachable","operator":"Exists","effect":"NoExecute","tolerationSeconds":300}
        ]),
    );
    pod["status"] = json!({"phase":"Pending","qosClass":"Guaranteed","conditions":[{
        "type":"PodScheduled","status":"False","reason":"SchedulingGated",
        "message":"Scheduling is blocked due to non-empty scheduling gates",
        "lastProbeTime":null,"lastTransitionTime":"2026-11-12T08:00:00Z"}]});
    pod
}
/// Non-pointer omitempty encoding of the four false values the renderer emits.
fn without_false_non_pointer_bools(mut pod: Value) -> Value {
    let spec = pod["spec"].as_object_mut().unwrap();
    for key in ["hostNetwork", "hostPID", "hostIPC"] {
        assert_eq!(spec.remove(key), Some(json!(false)));
    }
    let container = spec.get_mut("containers").unwrap()[0]
        .as_object_mut()
        .unwrap();
    assert_eq!(container.remove("tty"), Some(json!(false)));
    pod
}

#[test]
fn rendered_request_is_deterministic_and_bound_to_the_confirmed_intent() {
    let rendered = rendered_case();
    let Rendered {
        intent,
        binding,
        claim,
        request,
        ..
    } = &rendered;
    let again = KubernetesSessionPodRequestV2::render(intent, binding, claim).unwrap();
    assert_eq!(&again, request);
    assert_eq!(again.bytes(), request.bytes());
    assert!(request.bytes().len() <= 32 * 1024);
    let ids = intent.ids().unwrap();
    let fingerprint = intent.authority().fingerprint();
    // Commitments recomputed from the documented domains, not by the renderer.
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:kubernetes-session-pod-request:v2\0");
    hash.update(fingerprint.as_bytes());
    hash.update(request.bytes());
    let request_sha256 = hex::encode(hash.finalize());
    assert_eq!(request.target().request_sha256(), request_sha256);
    let pod = pod_of(request);
    let simple = ids.attempt_id.simple().to_string();
    assert_eq!(
        serde_json::to_value(request.target()).unwrap(),
        json!({
            "version":2,"renderer":"kubernetes_session_pod_v2r1",
            "reservation_id":intent.reservation_id(),
            "reservation_input_hash":binding.reservation_input_hash,
            "session_id":ids.session_id.to_string(),"attempt_id":ids.attempt_id.to_string(),
            "authority_fingerprint":fingerprint,
            "preparation_sha256":binding.preparation_sha256,
            "pod_name":format!("ai-guard-session-{simple}"),
            "workspace_claim_name":format!("ai-guard-ws-{simple}"),
            "workspace_claim_uid":CLAIM_UID,"request_sha256":request_sha256,
            "requested_configuration_sha256":
                KubernetesPodHandle::configuration_digest(&pod["spec"]).unwrap()
        })
    );
    // The preparation commitment is the confirmed journal's own binding.
    assert_eq!(
        request.target().preparation_sha256(),
        binding.preparation_sha256
    );
    assert_eq!(
        pod["metadata"],
        json!({"name":format!("ai-guard-session-{simple}"),"namespace":"tenant-a",
            "labels":{"app.kubernetes.io/managed-by":"ai-guard",
                "ai-guard/session":ids.session_id.to_string(),
                "ai-guard/attempt":ids.attempt_id.to_string(),
                "ai-guard/purpose":"session-v2"}})
    );
    // Another real reservation renders other bytes, names and commitments,
    // and its preparation binding cannot be substituted for this intent.
    let other = rendered_case();
    assert_ne!(other.request.bytes(), request.bytes());
    assert_ne!(
        other.request.target().pod_name(),
        request.target().pod_name()
    );
    assert_ne!(
        other.request.target().request_sha256(),
        request.target().request_sha256()
    );
    assert_eq!(
        KubernetesSessionPodRequestV2::render(intent, &other.binding, claim),
        Err(KubernetesError::InvalidPreflightPlan)
    );
}

#[test]
fn rendered_spec_satisfies_the_closed_v2_baseline_without_secret_argv_or_host_access() {
    let rendered = rendered_case();
    let pod = pod_of(&rendered.request);
    let mut keys = BTreeSet::new();
    collect_keys(&pod, &mut keys);
    for forbidden in [
        "hostPath",
        "nodeName",
        "env",
        "envFrom",
        "secret",
        "projected",
        "serviceAccountToken",
        "initContainers",
        "ephemeralContainers",
        "args",
        "annotations",
        "hostUsers",
        "ownerReferences",
        "finalizers",
        "readOnly",
    ] {
        assert!(
            !keys.contains(forbidden),
            "forbidden key rendered: {forbidden}"
        );
    }
    let text = String::from_utf8(rendered.request.bytes().to_vec()).unwrap();
    let provider = rendered.intent.profile().retained_provider().unwrap();
    let credential = provider.credential_binding_id.to_string();
    for private in [
        "public session fixture",
        credential.as_str(),
        BOOTSTRAP_ENV,
        "--prompt",
        "--client",
    ] {
        assert!(
            !text.contains(private),
            "private material rendered: {private}"
        );
    }
    let spec = &pod["spec"];
    assert_eq!(
        spec["schedulingGates"],
        json!([{"name":"ai-guard.io/session-v2"}])
    );
    for (key, value) in [
        ("automountServiceAccountToken", json!(false)),
        ("enableServiceLinks", json!(false)),
        ("hostNetwork", json!(false)),
        ("hostPID", json!(false)),
        ("hostIPC", json!(false)),
        ("shareProcessNamespace", json!(false)),
        ("restartPolicy", json!("Never")),
        ("dnsPolicy", json!("None")),
        ("serviceAccountName", json!("default")),
    ] {
        assert_eq!(spec[key], value, "{key}");
    }
    let containers = spec["containers"].as_array().unwrap();
    assert_eq!(containers.len(), 1);
    let worker = &containers[0];
    assert_eq!(worker["name"], json!("worker"));
    assert_eq!(
        worker["image"],
        json!(rendered.intent.profile().retained_image())
    );
    assert_eq!(worker["imagePullPolicy"], json!("Never"));
    assert_eq!(worker["stdin"], json!(true));
    assert_eq!(worker["stdinOnce"], json!(true));
    assert_eq!(worker["tty"], json!(false));
    assert_eq!(
        worker["securityContext"],
        json!({"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,"privileged":false,
            "allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,
            "capabilities":{"drop":["ALL"]},"seccompProfile":{"type":"RuntimeDefault"}})
    );
    assert_eq!(
        spec["volumes"],
        json!([
            {"name":"workspace","persistentVolumeClaim":{"claimName":rendered.claim.name()}},
            {"name":"temporary","emptyDir":{"medium":"Memory","sizeLimit":"8Mi"}},
            {"name":"home","emptyDir":{"medium":"Memory","sizeLimit":"8Mi"}}
        ])
    );
    assert_eq!(
        worker["volumeMounts"],
        json!([{"name":"workspace","mountPath":"/workspace"},
            {"name":"temporary","mountPath":"/tmp"},{"name":"home","mountPath":"/home/agent"}])
    );
}

#[test]
fn pod_command_is_the_existing_bootstrap_waiting_mode_and_never_a_business_invocation() {
    let rendered = rendered_case();
    let pod = pod_of(&rendered.request);
    let worker = &pod["spec"]["containers"][0];
    let command: Vec<String> = serde_json::from_value(worker["command"].clone()).unwrap();
    assert_eq!(command, [BOOTSTRAP_PROGRAM, "kubernetes-session-wait-v2"]);
    assert_eq!(command[1], SESSION_WAIT_MODE_ARG);
    for absent in ["args", "env", "envFrom"] {
        assert!(worker.get(absent).is_none(), "{absent}");
    }
    let profile = rendered.intent.profile();
    let provider = profile.retained_provider().unwrap();
    // The waiting-mode argv is refused by the business-invocation validator,
    // while the committed business argv of the same profile is accepted.
    assert!(provider.validate_argv(&command).is_err());
    let business: Vec<String> = [
        BOOTSTRAP_PROGRAM,
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
        "public session fixture",
    ]
    .map(str::to_owned)
    .into();
    assert!(provider.validate_argv(&business).is_ok());
    assert!(
        profile
            .verify_invocation(
                KubernetesSessionInvocationV2::new(BOOTSTRAP_PROGRAM, &business[1..]).unwrap()
            )
            .is_ok()
    );
}

#[test]
fn rendered_spec_has_no_false_non_pointer_bool_the_validator_cannot_restore() {
    let rendered = rendered_case();
    let pod = pod_of(&rendered.request);
    let mut falses = Vec::new();
    collect_false(&pod, "", &mut falses);
    let mut non_pointer = Vec::new();
    for path in &falses {
        let (parent, key) = path.rsplit_once('/').unwrap();
        if !POINTER_BOOLS.contains(&key) {
            assert!(
                RESTORED.contains(&(parent, key)),
                "unrestorable omitted false: {path}"
            );
            non_pointer.push(path.as_str());
        }
    }
    non_pointer.sort_unstable();
    assert_eq!(
        non_pointer,
        [
            "/spec/containers/0/tty",
            "/spec/hostIPC",
            "/spec/hostNetwork",
            "/spec/hostPID"
        ]
    );
    let mut keys = BTreeSet::new();
    collect_keys(&pod, &mut keys);
    assert!(!keys.contains("readOnly"));
    assert_eq!(
        pod.pointer("/spec/containers/0/stdinOnce"),
        Some(&json!(true))
    );
}

#[test]
fn rendered_quantities_reuse_the_v1_canonical_forms() {
    assert_eq!(cpu_quantity(1000), "1");
    assert_eq!(cpu_quantity(1500), "1500m");
    assert_eq!(mib(1024), "1Gi");
    assert_eq!(mib(768), "768Mi");
    let rendered = rendered_case();
    let pod = pod_of(&rendered.request);
    let resources = rendered.intent.resources();
    let expected = json!({"cpu":"500m","memory":"128Mi","ephemeral-storage":"16Mi"});
    assert_eq!(
        pod["spec"]["containers"][0]["resources"],
        json!({"requests":expected,"limits":expected})
    );
    assert_eq!(expected["cpu"], json!(cpu_quantity(resources.cpu_millis)));
    assert_eq!(expected["memory"], json!(mib(resources.memory_mib)));
    assert_eq!(
        pod["spec"]["volumes"][1]["emptyDir"]["sizeLimit"],
        json!(mib(resources.temporary_mib))
    );
    assert_eq!(
        pod["spec"]["volumes"][2]["emptyDir"]["sizeLimit"],
        json!(mib(resources.home_mib))
    );
    assert_eq!(
        pod["spec"]["activeDeadlineSeconds"],
        json!(resources.deadline_seconds)
    );
}

#[test]
fn admitted_response_accepts_documented_defaults_and_omitempty_encoding_and_refuses_every_profile_mutation()
 {
    let rendered = rendered_case();
    let request = &rendered.request;
    let validate =
        |pod: &Value| validate_admitted_session_pod(request, &serde_json::to_vec(pod).unwrap());
    let bare = validate(&admitted(request)).unwrap();
    assert_eq!(bare.pod_uid(), POD_UID);
    let echo = with_server_defaults(admitted(request));
    let instance = validate(&echo).unwrap();
    assert_eq!(instance.pod_uid(), POD_UID);
    assert_eq!(instance.created_resource_version(), "42");
    assert_eq!(instance.target(), request.target());
    // The API encoder omits false non-pointer bools: still the exact profile.
    let compact = validate(&without_false_non_pointer_bools(echo.clone())).unwrap();
    assert_eq!(compact.pod_uid(), POD_UID);
    let mut containers = echo["spec"]["containers"].clone();
    containers
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"sidecar","image":format!("local/sidecar@sha256:{}","d".repeat(64))}));
    let mut volumes = echo["spec"]["volumes"].clone();
    volumes
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"host","hostPath":{"path":"/"}}));
    for (pointer, value) in [
        ("/spec/nodeName", json!("node-a")),
        ("/spec/schedulingGates", json!([])),
        ("/spec/containers", containers),
        (
            "/spec/containers/0/image",
            json!(format!("local/other@sha256:{}", "c".repeat(64))),
        ),
        (
            "/spec/containers/0/env",
            json!([{"name":"INJECTED","value":"synthetic-marker"}]),
        ),
        ("/spec/volumes", volumes),
        ("/spec/automountServiceAccountToken", json!(true)),
        (
            "/metadata/annotations",
            json!({"unapproved":"synthetic-marker"}),
        ),
        (
            "/metadata/ownerReferences",
            json!([{"apiVersion":"v1","kind":"Node","name":"node-a","uid":POD_UID}]),
        ),
        ("/status/phase", json!("Running")),
        (
            "/status/containerStatuses",
            json!([{"name":"worker","restartCount":0}]),
        ),
        ("/spec/containers/0/stdin", json!(false)),
        ("/metadata/finalizers", json!(["unapproved"])),
        ("/metadata/deletionTimestamp", json!("2026-11-12T08:00:01Z")),
        ("/metadata/generation", json!(2)),
        ("/metadata/uid", json!(Uuid::nil().to_string())),
        ("/metadata/name", json!("ai-guard-session-other")),
        ("/status/startTime", json!("2026-11-12T08:00:01Z")),
    ] {
        assert!(
            validate(&replace(&echo, pointer, value)).is_err(),
            "profile mutation accepted: {pointer}"
        );
    }
    assert_eq!(
        validate(&replace(
            &echo,
            "/spec/automountServiceAccountToken",
            json!(true)
        ))
        .err(),
        Some(KubernetesSessionRuntimeError::ProfileChanged)
    );
    let bytes = serde_json::to_string(&echo).unwrap();
    let duplicate = bytes.replacen("\"kind\":\"Pod\"", "\"kind\":\"Pod\",\"kind\":\"Pod\"", 1);
    assert_ne!(duplicate, bytes);
    assert!(validate_admitted_session_pod(request, duplicate.as_bytes()).is_err());
    let mut large = bytes.into_bytes();
    large.resize(1024 * 1024 + 1, b' ');
    assert_eq!(
        validate_admitted_session_pod(request, &large).err(),
        Some(KubernetesSessionRuntimeError::Kubernetes(
            KubernetesError::ResponseTooLarge
        ))
    );
}

#[test]
fn render_refuses_a_workspace_claim_not_named_for_this_attempt() {
    let rendered = rendered_case();
    let Rendered {
        intent,
        binding,
        claim,
        ..
    } = &rendered;
    let authority = intent.authority();
    // A real projection named for another attempt, same authority.
    let elsewhere_name = session_workspace_claim_name(Uuid::new_v4());
    let other_attempt = parse_workspace_claim(
        authority,
        &elsewhere_name,
        bound_pvc(authority.namespace(), &elsewhere_name, CLAIM_UID),
        OBSERVED_AT,
    )
    .unwrap();
    assert_eq!(
        KubernetesSessionPodRequestV2::render(intent, binding, &other_attempt),
        Err(KubernetesError::InvalidPreflightPlan)
    );
    // The right name, observed through another authority (endpoint, trust).
    let elsewhere = KubernetesAuthorityV1::new(
        Uuid::new_v4(),
        "https://127.0.0.1:7443",
        &authority.cluster_namespace_uid,
        authority.namespace(),
        &authority.namespace_uid,
        &"b".repeat(64),
    )
    .unwrap();
    let foreign = parse_workspace_claim(
        &elsewhere,
        claim.name(),
        bound_pvc(authority.namespace(), claim.name(), CLAIM_UID),
        OBSERVED_AT,
    )
    .unwrap();
    assert_eq!(foreign.name(), claim.name());
    assert_eq!(foreign.uid(), claim.uid());
    assert_eq!(
        KubernetesSessionPodRequestV2::render(intent, binding, &foreign),
        Err(KubernetesError::InvalidPreflightPlan)
    );
    // Control: the claim named for this attempt and authority renders.
    assert!(KubernetesSessionPodRequestV2::render(intent, binding, claim).is_ok());
}
