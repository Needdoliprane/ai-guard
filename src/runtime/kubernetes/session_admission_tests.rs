//! Session admission through the real HTTPS client against one owned TLS
//! loopback peer. Every object below is a declared fixture: a passing
//! admission here qualifies no API server, CNI, CSI driver or RBAC setup.
use super::*;
use crate::{
    control_fs::PrivateFile,
    runtime::kubernetes::{
        KubernetesAuthorityV1, KubernetesCredentials,
        session::tests::fixture_at,
        session_pod::tests::rendered_case_with,
        session_test_peer::{PeerReply, PeerRequest, SessionTestPeer},
    },
};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};

const POLICY_UID: &str = "abababab-abab-4bab-8bab-abababababab";

fn activation_for(authority: &KubernetesAuthorityV1) -> KubernetesSessionActivationV2 {
    KubernetesSessionActivationV2 {
        version: 1,
        activated: true,
        authority: authority.clone(),
        pod_security_version: "v1.36".into(),
        network_policy_name: "ai-guard-default-deny".into(),
        storage: KubernetesSessionStorageRequestV2 {
            version: 1,
            storage_class_name: "ai-guard-rwop".into(),
            capacity_mib: 1024,
        },
    }
}

/// Fixture objects served at fixed paths; a test replaces one of them.
#[derive(Clone)]
struct Objects {
    namespace_labels: Value,
    policies: Value,
    account: Value,
    class: Value,
    driver: Option<Value>,
    /// Review decisions by (resource, subresource, verb); default allowed
    /// for the required set, denied for the forbidden set.
    overrides: BTreeMap<String, bool>,
}
impl Objects {
    fn good() -> Self {
        Self {
            namespace_labels: json!({"kubernetes.io/metadata.name":"tenant-a",
                "pod-security.kubernetes.io/enforce":"restricted",
                "pod-security.kubernetes.io/enforce-version":"v1.36"}),
            policies: json!({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicyList",
                "items":[{"metadata":{"name":"ai-guard-default-deny","namespace":"tenant-a",
                    "uid":POLICY_UID,"resourceVersion":"12"},
                    "spec":{"podSelector":{},"policyTypes":["Ingress","Egress"]}}]}),
            account: json!({"apiVersion":"v1","kind":"ServiceAccount",
                "metadata":{"name":"default","namespace":"tenant-a"},
                "automountServiceAccountToken":false}),
            class: json!({"apiVersion":"storage.k8s.io/v1","kind":"StorageClass",
                "metadata":{"name":"ai-guard-rwop"},"provisioner":"hostpath.csi.k8s.io",
                "reclaimPolicy":"Delete","volumeBindingMode":"Immediate"}),
            driver: Some(json!({"apiVersion":"storage.k8s.io/v1","kind":"CSIDriver",
                "metadata":{"name":"hostpath.csi.k8s.io"},"spec":{"fsGroupPolicy":"File"}})),
            overrides: BTreeMap::new(),
        }
    }
}
fn review_key(attributes: &Value) -> String {
    format!(
        "{}/{}:{}",
        attributes["resource"].as_str().unwrap_or_default(),
        attributes["subresource"].as_str().unwrap_or_default(),
        attributes["verb"].as_str().unwrap_or_default()
    )
}
fn default_allowed(attributes: &Value) -> bool {
    KubernetesPermissionActionV1::SESSION_REQUIRED
        .iter()
        .any(|action| review_key(&action.attributes("tenant-a")) == review_key(attributes))
}

struct Case {
    peer: SessionTestPeer,
    runtime: KubernetesRuntime,
    authority: KubernetesAuthorityV1,
    objects: Arc<Mutex<Objects>>,
    _rendered: crate::runtime::kubernetes::session_pod::tests::Rendered,
}
impl Case {
    async fn new() -> Self {
        let peer = SessionTestPeer::start().await;
        let ca = PrivateFile::open(peer.ca(), 64 * 1024).unwrap();
        let trust = ca.digest().to_owned();
        let token = peer.ca().parent().unwrap().join("session-admission-token");
        std::fs::write(
            &token,
            "public-session-admission-loopback-fixture-token-0000000",
        )
        .unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        let rendered = rendered_case_with(fixture_at(peer.endpoint(), &trust));
        let authority = rendered.intent.authority().clone();
        let runtime = KubernetesRuntime::connect(
            authority.clone(),
            KubernetesCredentials::bearer(ca, PrivateFile::open(&token, 8192).unwrap()),
        )
        .unwrap();
        let objects = Arc::new(Mutex::new(Objects::good()));
        let served = objects.clone();
        let (cluster_uid, namespace_uid) = (
            authority.cluster_namespace_uid.clone(),
            authority.namespace_uid.clone(),
        );
        peer.respond(move |request: &PeerRequest| {
            let objects = served.lock().unwrap().clone();
            match (request.method(), request.target()) {
                ("GET", "/api/v1/namespaces/kube-system") => PeerReply::json(
                    200,
                    &json!({"apiVersion":"v1","kind":"Namespace",
                        "metadata":{"name":"kube-system","uid":cluster_uid},
                        "status":{"phase":"Active"}}),
                ),
                ("GET", "/api/v1/namespaces/tenant-a") => PeerReply::json(
                    200,
                    &json!({"apiVersion":"v1","kind":"Namespace",
                        "metadata":{"name":"tenant-a","uid":namespace_uid,
                            "labels":objects.namespace_labels},
                        "status":{"phase":"Active"}}),
                ),
                ("GET", "/apis/networking.k8s.io/v1/namespaces/tenant-a/networkpolicies") => {
                    PeerReply::json(200, &objects.policies)
                }
                ("GET", "/api/v1/namespaces/tenant-a/serviceaccounts/default") => {
                    PeerReply::json(200, &objects.account)
                }
                ("GET", "/apis/storage.k8s.io/v1/storageclasses/ai-guard-rwop") => {
                    PeerReply::json(200, &objects.class)
                }
                ("GET", "/apis/storage.k8s.io/v1/csidrivers/hostpath.csi.k8s.io") => {
                    match objects.driver {
                        Some(driver) => PeerReply::json(200, &driver),
                        None => PeerReply::json(404, &json!({"kind":"Status","code":404})),
                    }
                }
                ("POST", "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews") => {
                    let body: Value = serde_json::from_slice(request.body()).unwrap();
                    let attributes = &body["spec"]["resourceAttributes"];
                    let allowed = objects
                        .overrides
                        .get(&review_key(attributes))
                        .copied()
                        .unwrap_or_else(|| default_allowed(attributes));
                    PeerReply::json(
                        201,
                        &json!({"apiVersion":"authorization.k8s.io/v1",
                            "kind":"SelfSubjectAccessReview","spec":body["spec"],
                            "status":{"allowed":allowed,"denied":!allowed}}),
                    )
                }
                _ => PeerReply::json(404, &json!({"kind":"Status","code":404})),
            }
        });
        Self {
            peer,
            runtime,
            authority,
            objects,
            _rendered: rendered,
        }
    }
    fn change(&self, change: impl FnOnce(&mut Objects)) {
        change(&mut self.objects.lock().unwrap());
    }
    fn activation(&self) -> KubernetesSessionActivationV2 {
        activation_for(&self.authority)
    }
    async fn admit(&self) -> Admission<KubernetesSessionAdmissionV2> {
        self.runtime
            .admit_session_workload(&self.activation())
            .await
    }
}

#[tokio::test]
async fn admission_returns_the_observed_anchors_of_an_exact_tenant_setup() {
    let case = Case::new().await;
    let admission = case.admit().await.unwrap();
    assert_eq!(
        admission.authority_fingerprint,
        case.authority.fingerprint()
    );
    assert_eq!(admission.namespace_uid, case.authority.namespace_uid);
    assert_eq!(admission.network_policy_uid, POLICY_UID);
    assert_eq!(admission.network_policy_resource_version, "12");
    assert_eq!(admission.storage_class_name, "ai-guard-rwop");
    assert_eq!(admission.csi_driver, "hostpath.csi.k8s.io");
    // Only reads and self reviews: no mutation of the cluster.
    assert!(
        case.peer
            .requests()
            .iter()
            .all(|request| request.method() == "GET"
                || request.target() == "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews")
    );
    let reviews = case
        .peer
        .requests()
        .iter()
        .filter(|request| request.method() == "POST")
        .count();
    assert_eq!(reviews, 19);
}

#[tokio::test]
async fn a_disabled_activation_refuses_before_any_request() {
    let case = Case::new().await;
    let mut disabled = case.activation();
    disabled.activated = false;
    assert_eq!(
        case.runtime
            .admit_session_workload(&disabled)
            .await
            .unwrap_err(),
        KubernetesSessionAdmissionError::NotActivated
    );
    let mut floating = case.activation();
    floating.pod_security_version = "latest".into();
    assert_eq!(
        case.runtime
            .admit_session_workload(&floating)
            .await
            .unwrap_err(),
        KubernetesSessionAdmissionError::Kubernetes(KubernetesError::InvalidPreflightPlan)
    );
    assert!(case.peer.requests().is_empty());
}

#[tokio::test]
async fn each_deviation_is_refused_with_its_own_reason() {
    type Change = Box<dyn FnOnce(&mut Objects)>;
    let deny_rule = json!([{"to":[{"ipBlock":{"cidr":"0.0.0.0/0"}}]}]);
    let cases: Vec<(Change, KubernetesSessionAdmissionError)> = vec![
        (
            Box::new(|o| {
                o.namespace_labels["pod-security.kubernetes.io/enforce"] = json!("baseline")
            }),
            KubernetesSessionAdmissionError::PodSecurityNotRestricted,
        ),
        (
            Box::new(|o| {
                o.namespace_labels["pod-security.kubernetes.io/enforce-version"] = json!("latest")
            }),
            KubernetesSessionAdmissionError::PodSecurityNotRestricted,
        ),
        (
            Box::new(|o| o.policies["items"] = json!([])),
            KubernetesSessionAdmissionError::NetworkPolicyNotDefaultDeny,
        ),
        (
            Box::new(move |o| o.policies["items"][0]["spec"]["egress"] = deny_rule),
            KubernetesSessionAdmissionError::NetworkPolicyNotDefaultDeny,
        ),
        (
            Box::new(|o| o.policies["items"][0]["spec"]["policyTypes"] = json!(["Ingress"])),
            KubernetesSessionAdmissionError::NetworkPolicyNotDefaultDeny,
        ),
        (
            Box::new(|o| {
                o.policies["items"][0]["spec"]["podSelector"] =
                    json!({"matchLabels":{"ai-guard/purpose":"session-v2"}})
            }),
            KubernetesSessionAdmissionError::NetworkPolicyNotDefaultDeny,
        ),
        (
            Box::new(|o| {
                let second = o.policies["items"][0].clone();
                o.policies["items"].as_array_mut().unwrap().push(second);
            }),
            KubernetesSessionAdmissionError::NetworkPolicyNotDefaultDeny,
        ),
        (
            Box::new(|o| {
                o.account
                    .as_object_mut()
                    .unwrap()
                    .remove("automountServiceAccountToken");
            }),
            KubernetesSessionAdmissionError::ServiceAccountTokenAutomount,
        ),
        (
            Box::new(|o| o.class["volumeBindingMode"] = json!("WaitForFirstConsumer")),
            KubernetesSessionAdmissionError::StorageClassNotImmediate,
        ),
        (
            Box::new(|o| o.driver = None),
            KubernetesSessionAdmissionError::StorageClassNotCsi,
        ),
        (
            Box::new(|o| {
                o.overrides
                    .insert("persistentvolumeclaims/:create".into(), false);
            }),
            KubernetesSessionAdmissionError::PermissionMissing(
                KubernetesPermissionActionV1::CreatePersistentVolumeClaims,
            ),
        ),
        (
            Box::new(|o| {
                o.overrides.insert("pods/exec:create".into(), true);
            }),
            KubernetesSessionAdmissionError::PermissionNotDenied(
                KubernetesPermissionActionV1::CreatePodExec,
            ),
        ),
        (
            Box::new(|o| {
                o.overrides
                    .insert("pods/ephemeralcontainers:patch".into(), true);
            }),
            KubernetesSessionAdmissionError::PermissionNotDenied(
                KubernetesPermissionActionV1::PatchPodEphemeralContainers,
            ),
        ),
    ];
    for (change, expected) in cases {
        let case = Case::new().await;
        case.change(change);
        assert_eq!(case.admit().await.unwrap_err(), expected);
        assert!(
            case.peer
                .requests()
                .iter()
                .all(|request| request.method() == "GET"
                    || request.target()
                        == "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews"),
            "{expected:?}: read-only"
        );
    }
}

#[tokio::test]
async fn an_activation_of_another_authority_opens_nothing() {
    let case = Case::new().await;
    let mut other = case.authority.clone();
    other.namespace_uid = "cdcdcdcd-cdcd-4dcd-8dcd-cdcdcdcdcdcd".into();
    assert_eq!(
        case.runtime
            .admit_session_workload(&activation_for(&other))
            .await
            .unwrap_err(),
        KubernetesSessionAdmissionError::NotActivated
    );
    assert!(case.peer.requests().is_empty());
}

#[test]
fn tenants_never_share_an_authority_namespace_or_namespace_uid() {
    let rendered = crate::runtime::kubernetes::session_pod::tests::rendered_case();
    let a = rendered.intent.authority().clone();
    let with = |change: &dyn Fn(&mut KubernetesAuthorityV1)| {
        let mut authority = a.clone();
        change(&mut authority);
        activation_for(&authority)
    };
    let own = with(&|authority| {
        authority.authority_id = uuid::Uuid::new_v4();
        authority.namespace = "tenant-b".into();
        authority.namespace_uid = "cdcdcdcd-cdcd-4dcd-8dcd-cdcdcdcdcdcd".into();
    });
    let first = activation_for(&a);
    refuse_shared_authorities([("tenant-a", &first.authority), ("tenant-b", &own.authority)])
        .unwrap();
    for shared in [
        // The same authority id.
        with(&|authority| {
            authority.namespace = "tenant-b".into();
            authority.namespace_uid = "cdcdcdcd-cdcd-4dcd-8dcd-cdcdcdcdcdcd".into();
        }),
        // The same endpoint and namespace under another id.
        with(&|authority| {
            authority.authority_id = uuid::Uuid::new_v4();
            authority.namespace_uid = "cdcdcdcd-cdcd-4dcd-8dcd-cdcdcdcdcdcd".into();
        }),
        // The same namespace UID behind another name.
        with(&|authority| {
            authority.authority_id = uuid::Uuid::new_v4();
            authority.namespace = "tenant-b".into();
        }),
    ] {
        let error = refuse_shared_authorities([
            ("tenant-a", &first.authority),
            ("tenant-b", &shared.authority),
        ])
        .unwrap_err()
        .to_string();
        assert!(error.contains("R10"), "{error}");
    }
}
