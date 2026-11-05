//! Pure profile/response contracts. No permission, HTTP, cluster or model call.
use super::*;
use crate::runtime::kubernetes::{KubernetesAuthorityV1, KubernetesError, KubernetesPodHandle};
use serde_json::json;
use uuid::Uuid;

const OBSERVED_AT: &str = "2026-11-05T08:00:00.000Z";
const POD_UID: &str = "11111111-1111-4111-8111-111111111119";

fn intent() -> KubernetesCreateIntentV1 {
    let authority = KubernetesAuthorityV1::new(
        Uuid::from_u128(1),
        "https://127.0.0.1:6443",
        "11111111-1111-4111-8111-111111111111",
        "tenant-a",
        "22222222-2222-4222-8222-222222222222",
        &"a".repeat(64),
    )
    .unwrap();
    serde_json::from_value(json!({
        "version":1,
        "create_id":Uuid::from_u128(8).to_string(),
        "session_id":Uuid::from_u128(7).to_string(),
        "control_namespace_uuid":Uuid::from_u128(3).to_string(),
        "broker_id":Uuid::from_u128(4).to_string(),
        "incarnation_id":Uuid::from_u128(5).to_string(),
        "authority":authority,
        "profile":"kubernetes_held_pod_v1",
        "image":format!("local/worker@sha256:{}","b".repeat(64)),
        "resources":{"cpu_millis":2000,"memory_mib":1024,"workspace_mib":128,
            "temporary_mib":64,"deadline_seconds":60}
    }))
    .unwrap()
}

// Independent fixture of the entire closed request, not a call to preflight
// rendering or to the new renderer. Value serialization fixes canonical keys.
fn golden_pod() -> Value {
    serde_json::from_str(r#"{
        "apiVersion":"v1", "kind":"Pod",
        "metadata":{
            "name":"ai-guard-held-00000000000000000000000000000008",
            "namespace":"tenant-a",
            "labels":{
                "app.kubernetes.io/managed-by":"ai-guard",
                "ai-guard/session":"00000000-0000-0000-0000-000000000007",
                "ai-guard/operation":"00000000-0000-0000-0000-000000000008",
                "ai-guard/purpose":"held-create"
            }
        },
        "spec":{
            "automountServiceAccountToken":false,"serviceAccountName":"default",
            "enableServiceLinks":false,"hostNetwork":false,"hostPID":false,"hostIPC":false,
            "shareProcessNamespace":false,"restartPolicy":"Never","activeDeadlineSeconds":60,
            "terminationGracePeriodSeconds":30,"dnsPolicy":"None",
            "dnsConfig":{"nameservers":["127.0.0.1"]},"schedulerName":"default-scheduler",
            "schedulingGates":[{"name":"ai-guard.io/held-create-v1"}],
            "securityContext":{"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,
                "fsGroup":1000,"fsGroupChangePolicy":"OnRootMismatch",
                "seccompProfile":{"type":"RuntimeDefault"}},
            "containers":[{
                "name":"worker","image":"local/worker@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "imagePullPolicy":"Never","command":["/usr/bin/id"],"workingDir":"/workspace",
                "stdin":false,"tty":false,"terminationMessagePath":"/dev/termination-log",
                "terminationMessagePolicy":"File",
                "securityContext":{"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,
                    "privileged":false,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,
                    "capabilities":{"drop":["ALL"]},"seccompProfile":{"type":"RuntimeDefault"}},
                "resources":{
                    "requests":{"cpu":"2","memory":"1Gi","ephemeral-storage":"1Mi"},
                    "limits":{"cpu":"2","memory":"1Gi","ephemeral-storage":"1Mi"}
                },
                "volumeMounts":[{"name":"workspace","mountPath":"/workspace"},
                    {"name":"temporary","mountPath":"/tmp"}]
            }],
            "volumes":[{"name":"workspace","emptyDir":{"medium":"Memory","sizeLimit":"128Mi"}},
                {"name":"temporary","emptyDir":{"medium":"Memory","sizeLimit":"64Mi"}}]
        }
    }"#).unwrap()
}

fn admitted_pod() -> Value {
    let mut pod = golden_pod();
    pod["metadata"]["uid"] = json!(POD_UID);
    pod["metadata"]["resourceVersion"] = json!("17");
    pod
}

fn replace(pod: &Value, pointer: &str, value: Value) -> Value {
    let mut changed = pod.clone();
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    changed
        .pointer_mut(parent)
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(key.into(), value);
    changed
}

#[test]
fn held_profile_matches_an_independent_golden_and_binds_exact_request_bytes() {
    let intent = intent();
    let prepared = prepare(&intent).expect("closed held profile must render");
    let expected = golden_pod();
    let bytes = serde_json::to_vec(&expected).unwrap();
    assert_eq!(prepared.pod_bytes(), bytes);
    assert!(bytes.len() <= 32 * 1024);
    // Independent JSON::PP/Digest::SHA calculation from this literal fixture;
    // no production hash helper supplies the expected commitments.
    assert_eq!(
        serde_json::to_value(prepared.target()).unwrap(),
        json!({"version":1,"create_id":intent.create_id,"session_id":intent.session_id,
            "authority_fingerprint":"6587b1d5ac873356ad23c7c8c9ff9c2dc7f167b292c5abf95dca08b8b2d49a61",
            "pod_name":"ai-guard-held-00000000000000000000000000000008",
            "request_sha256":"fd91e5469fecf619ca05cd484d54d1ae642c661430c323c9e3955cf5a89139f5",
            "configuration_sha256":"d82eddf9473009bcb6e019d3af5ff1daac8c0a94dbb2c83e7be24bdb5ea22866"})
    );
    assert_eq!(
        intent.authority.require_workload_admission(),
        Err(KubernetesError::WorkloadAdmissionUnavailable)
    );
    for (pointer, value) in [
        ("/image", json!("local/worker:latest")),
        ("/profile", json!("unknown")),
        ("/resources/cpu_millis", json!(0)),
        ("/resources/workspace_mib", json!(1024)),
    ] {
        let value = replace(&serde_json::to_value(&intent).unwrap(), pointer, value);
        let invalid: KubernetesCreateIntentV1 = serde_json::from_value(value).unwrap();
        assert!(
            prepare(&invalid).is_err(),
            "invalid held profile: {pointer}"
        );
    }
}

#[test]
fn held_response_projects_only_closed_facts_and_preserves_the_admitted_spec_digest() {
    let intent = intent();
    let mut pod = admitted_pod();
    pod["metadata"]["creationTimestamp"] = json!("2026-11-05T07:59:59Z");
    pod["metadata"]["managedFields"] = json!([{"manager":"server-only"}]);
    let spec = pod["spec"].as_object_mut().unwrap();
    for field in ["hostNetwork", "hostPID", "hostIPC"] {
        spec.remove(field);
    }
    spec.insert("serviceAccount".into(), json!("default"));
    spec.insert("preemptionPolicy".into(), json!("PreemptLowerPriority"));
    spec.insert("priority".into(), json!(0));
    spec.insert("tolerations".into(), json!([
        {"key":"node.kubernetes.io/not-ready","operator":"Exists","effect":"NoExecute","tolerationSeconds":300},
        {"key":"node.kubernetes.io/unreachable","operator":"Exists","effect":"NoExecute","tolerationSeconds":300}
    ]));
    let container = spec.get_mut("containers").unwrap()[0]
        .as_object_mut()
        .unwrap();
    container.remove("stdin");
    container.remove("tty");
    pod["status"] = json!({"phase":"Pending","containerStatuses":[],"initContainerStatuses":[],
        "ephemeralContainerStatuses":[],"qosClass":"Guaranteed",
        "conditions":[{"type":"PodScheduled","status":"False","reason":"SchedulingGated"}]});
    let outcome = parse_created_held(&intent, &serde_json::to_vec(&pod).unwrap(), OBSERVED_AT)
        .expect("valid held response must project safely");
    let KubernetesCreateOutcomeV1::CreatedHeld {
        observation,
        admitted_spec,
        admitted_configuration_sha256,
        observed_at,
    } = outcome
    else {
        panic!("valid held response cannot be unknown");
    };
    assert_eq!(admitted_spec, pod["spec"]);
    assert_eq!(
        admitted_configuration_sha256,
        KubernetesPodHandle::configuration_digest(&pod["spec"]).unwrap()
    );
    assert_ne!(
        admitted_configuration_sha256,
        KubernetesPodHandle::configuration_digest(&golden_pod()["spec"]).unwrap()
    );
    assert_eq!(observed_at, OBSERVED_AT);
    assert_eq!(
        serde_json::to_value(&observation).unwrap(),
        json!({"version":1,"api_version":"v1","kind":"Pod",
            "metadata":{"name":pod["metadata"]["name"],"namespace":"tenant-a","uid":POD_UID,
                "resource_version":"17","labels":golden_pod()["metadata"]["labels"],
                "annotation_count":0,"owner_reference_count":0,"finalizer_count":0,
                "deletion_requested":false,"unexpected_fields_present":false},
            "status":{"phase":"Pending","container_status_count":0,"init_container_status_count":0,
                "ephemeral_container_status_count":0,"start_time_present":false,
                "placement_fields_present":false,"unexpected_fields_present":false,
                "pod_scheduled":{"status":"False","reason":"SchedulingGated"}}})
    );
    assert!(
        !serde_json::to_string(&observation)
            .unwrap()
            .contains("server-only")
    );
    validate_created_held(
        &intent,
        &observation,
        &admitted_spec,
        &admitted_configuration_sha256,
    )
    .expect("control-side pure validation must accept the same safe facts");
    let absent = parse_created_held(
        &intent,
        &serde_json::to_vec(&admitted_pod()).unwrap(),
        OBSERVED_AT,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(absent).unwrap()["observation"]["status"],
        Value::Null
    );
}

#[test]
fn held_response_rejects_mutation_execution_duplicate_keys_and_unbounded_input() {
    let intent = intent();
    let valid = admitted_pod();
    let safe = parse_created_held(&intent, &serde_json::to_vec(&valid).unwrap(), OBSERVED_AT)
        .expect("negative cases require a working positive response path");
    for (pointer, value) in [
        ("/metadata/uid", json!(Uuid::nil().to_string())),
        ("/metadata/resourceVersion", json!("")),
        ("/metadata/namespace", json!("tenant-b")),
        (
            "/metadata/annotations",
            json!({"unapproved":"synthetic-marker"}),
        ),
        ("/metadata/ownerReferences", json!([{}])),
        ("/metadata/finalizers", json!(["unapproved"])),
        ("/metadata/unexpected", json!(true)),
        ("/spec/schedulingGates", json!([])),
        ("/spec/nodeName", json!("node-1")),
        ("/spec/containers/0/args", json!(["unapproved"])),
        (
            "/spec/containers/0/env",
            json!([{"name":"UNAPPROVED","value":"synthetic-marker"}]),
        ),
        ("/spec/initContainers", json!([{}])),
        ("/status", json!({"phase":"Running"})),
        ("/status", json!({"containerStatuses":[{"name":"worker"}]})),
        ("/status", json!({"containerStatuses":null})),
        ("/status", json!({"startTime":"2026-11-05T08:00:00Z"})),
        ("/status", json!({"hostIP":"192.0.2.1"})),
        (
            "/status",
            json!({"conditions":[{"type":"PodScheduled","status":"True","reason":"Scheduled"}]}),
        ),
        ("/status", json!({"unexpected":"synthetic-marker"})),
    ] {
        let changed = replace(&valid, pointer, value);
        assert!(
            parse_created_held(&intent, &serde_json::to_vec(&changed).unwrap(), OBSERVED_AT)
                .is_err(),
            "unsafe held response accepted: {pointer}"
        );
    }
    let bytes = serde_json::to_string(&valid).unwrap();
    let duplicate_root = format!("{{\"kind\":\"Pod\",{}", &bytes[1..]);
    let duplicate_spec = bytes.replacen(
        "\"restartPolicy\":\"Never\"",
        "\"restartPolicy\":\"Never\",\"restartPolicy\":\"Never\"",
        1,
    );
    assert_ne!(duplicate_spec, bytes);
    for duplicate in [duplicate_root, duplicate_spec] {
        assert!(parse_created_held(&intent, duplicate.as_bytes(), OBSERVED_AT).is_err());
    }
    let mut too_large = bytes.into_bytes();
    too_large.resize(1024 * 1024 + 1, b' ');
    assert!(parse_created_held(&intent, &too_large, OBSERVED_AT).is_err());
    let mut nested = Value::Null;
    for _ in 0..65 {
        nested = Value::Array(vec![nested]);
    }
    let too_deep = replace(&valid, "/metadata/managedFields", nested);
    assert!(
        parse_created_held(
            &intent,
            &serde_json::to_vec(&too_deep).unwrap(),
            OBSERVED_AT
        )
        .is_err()
    );
    for date in [
        "2026-11-05T08:00:00Z",
        "2026-11-05T08:00:00.000+00:00",
        "2026-11-05T08:00:00.000000Z",
    ] {
        assert!(parse_created_held(&intent, &serde_json::to_vec(&valid).unwrap(), date).is_err());
    }
    // Central validation receives descriptive DTOs, not a raw Pod. It must
    // independently reject forged flags/counts instead of trusting extraction.
    let KubernetesCreateOutcomeV1::CreatedHeld {
        observation,
        admitted_spec,
        admitted_configuration_sha256,
        ..
    } = safe
    else {
        panic!("positive response must produce a held candidate");
    };
    let wire = serde_json::to_value(&observation).unwrap();
    for (pointer, value) in [
        ("/version", json!(2)),
        ("/metadata/annotation_count", json!(1)),
        ("/metadata/deletion_requested", json!(true)),
        ("/metadata/unexpected_fields_present", json!(true)),
        (
            "/status",
            json!({"phase":"Running","container_status_count":null,
            "init_container_status_count":null,"ephemeral_container_status_count":null,
            "start_time_present":false,"placement_fields_present":false,
            "unexpected_fields_present":false,"pod_scheduled":null}),
        ),
    ] {
        let forged: KubernetesCreateObservationV1 =
            serde_json::from_value(replace(&wire, pointer, value)).unwrap();
        assert!(
            validate_created_held(
                &intent,
                &forged,
                &admitted_spec,
                &admitted_configuration_sha256
            )
            .is_err(),
            "forged projection accepted: {pointer}"
        );
    }
}
