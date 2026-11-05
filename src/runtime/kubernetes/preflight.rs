//! Server-side admission inspection of a closed, non-executed Pod profile.
//! This is deliberately not a start permit or a producer of owned Pod handles.
use super::{
    KubernetesAdmissionGapV1, KubernetesAuthorityV1, KubernetesError, KubernetesPodHandle,
    KubernetesResult, KubernetesRuntime, valid_immutable_image,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Resource declarations sent to admission, not observed runtime enforcement.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesPreflightResourcesV1 {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    pub workspace_mib: u32,
    pub temporary_mib: u32,
    pub deadline_seconds: u32,
}
impl KubernetesPreflightResourcesV1 {
    pub(super) fn validate(&self) -> KubernetesResult<()> {
        if !(1..=200_000).contains(&self.cpu_millis)
            || !(1..=102_400).contains(&self.memory_mib)
            || self.workspace_mib == 0
            || self.temporary_mib == 0
            || u64::from(self.workspace_mib) + u64::from(self.temporary_mib)
                >= u64::from(self.memory_mib)
            || !(1..=86_400).contains(&self.deadline_seconds)
        {
            return Err(KubernetesError::InvalidPreflightPlan);
        }
        Ok(())
    }
}

/// Private operator input. There is no tenant, credential, argv, environment,
/// free Pod manifest, image pull secret or kubeconfig field. The fixed /usr/bin/id
/// profile tests admission only; a business command must use a future private
/// execution channel, never the API object's command or environment fields.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesPreflightPlanV1 {
    version: u32,
    authority_fingerprint: String,
    session_id: Uuid,
    operation_id: Uuid,
    image: String,
    resources: KubernetesPreflightResourcesV1,
}
impl KubernetesPreflightPlanV1 {
    pub fn new(
        authority: &KubernetesAuthorityV1,
        session_id: Uuid,
        operation_id: Uuid,
        image: &str,
        resources: KubernetesPreflightResourcesV1,
    ) -> KubernetesResult<Self> {
        let plan = Self {
            version: 1,
            authority_fingerprint: authority.fingerprint(),
            session_id,
            operation_id,
            image: image.into(),
            resources,
        };
        plan.validate(authority)?;
        Ok(plan)
    }

    fn validate(&self, authority: &KubernetesAuthorityV1) -> KubernetesResult<()> {
        authority.validate()?;
        self.resources.validate()?;
        let image_valid = valid_immutable_image(&self.image);
        if self.version != 1
            || self.authority_fingerprint != authority.fingerprint()
            || self.session_id.is_nil()
            || self.operation_id.is_nil()
            || !image_valid
        {
            return Err(KubernetesError::InvalidPreflightPlan);
        }
        Ok(())
    }

    fn pod_name(&self) -> String {
        format!("ai-guard-preflight-{}", self.operation_id.simple())
    }

    fn pod(&self, authority: &KubernetesAuthorityV1) -> KubernetesResult<Value> {
        self.validate(authority)?;
        let cpu = if self.resources.cpu_millis.is_multiple_of(1000) {
            (self.resources.cpu_millis / 1000).to_string()
        } else {
            format!("{}m", self.resources.cpu_millis)
        };
        let memory = mib_quantity(self.resources.memory_mib);
        let workspace = mib_quantity(self.resources.workspace_mib);
        let temporary = mib_quantity(self.resources.temporary_mib);
        let limits = json!({"cpu":cpu,"memory":memory,"ephemeral-storage":"1Mi"});
        Ok(json!({
            "apiVersion":"v1",
            "kind":"Pod",
            "metadata":{
                "name":self.pod_name(),
                "namespace":authority.namespace(),
                "labels":{
                    "app.kubernetes.io/managed-by":"ai-guard",
                    "ai-guard/session":self.session_id.to_string(),
                    "ai-guard/operation":self.operation_id.to_string(),
                    "ai-guard/purpose":"admission-preflight"
                }
            },
            "spec":{
                "automountServiceAccountToken":false,
                "serviceAccountName":"default",
                "enableServiceLinks":false,
                "hostNetwork":false,
                "hostPID":false,
                "hostIPC":false,
                "shareProcessNamespace":false,
                "restartPolicy":"Never",
                "activeDeadlineSeconds":self.resources.deadline_seconds,
                "terminationGracePeriodSeconds":30,
                "dnsPolicy":"None",
                "dnsConfig":{"nameservers":["127.0.0.1"]},
                "schedulerName":"default-scheduler",
                "securityContext":{
                    "runAsNonRoot":true,
                    "runAsUser":1000,
                    "runAsGroup":1000,
                    "fsGroup":1000,
                    "fsGroupChangePolicy":"OnRootMismatch",
                    "seccompProfile":{"type":"RuntimeDefault"}
                },
                "containers":[{
                    "name":"worker",
                    "image":self.image,
                    "imagePullPolicy":"Never",
                    "command":["/usr/bin/id"],
                    "workingDir":"/workspace",
                    "stdin":false,
                    "tty":false,
                    "terminationMessagePath":"/dev/termination-log",
                    "terminationMessagePolicy":"File",
                    "securityContext":{
                        "runAsNonRoot":true,
                        "runAsUser":1000,
                        "runAsGroup":1000,
                        "privileged":false,
                        "allowPrivilegeEscalation":false,
                        "readOnlyRootFilesystem":true,
                        "capabilities":{"drop":["ALL"]},
                        "seccompProfile":{"type":"RuntimeDefault"}
                    },
                    "resources":{"requests":limits,"limits":limits},
                    "volumeMounts":[
                        {"name":"workspace","mountPath":"/workspace"},
                        {"name":"temporary","mountPath":"/tmp"}
                    ]
                }],
                "volumes":[
                    {"name":"workspace","emptyDir":{"medium":"Memory","sizeLimit":workspace}},
                    {"name":"temporary","emptyDir":{"medium":"Memory","sizeLimit":temporary}}
                ]
            }
        }))
    }

    /// This commitment must be bound to authenticated tenant/control authority
    /// and durably authorized before the request is sent. It grants no start.
    pub fn target(
        &self,
        authority: &KubernetesAuthorityV1,
    ) -> KubernetesResult<KubernetesPreflightTargetV1> {
        let pod = self.pod(authority)?;
        let bytes = serde_json::to_vec(&pod).map_err(|_| KubernetesError::InvalidPreflightPlan)?;
        let mut hash = Sha256::new();
        hash.update(b"ai-guard:kubernetes-preflight-request:v1\0");
        hash.update(authority.fingerprint().as_bytes());
        hash.update(&bytes);
        Ok(KubernetesPreflightTargetV1 {
            version: 1,
            authority_fingerprint: authority.fingerprint(),
            session_id: self.session_id,
            operation_id: self.operation_id,
            pod_name: self.pod_name(),
            request_sha256: hex::encode(hash.finalize()),
            configuration_sha256: KubernetesPodHandle::configuration_digest(&pod["spec"])
                .map_err(|_| KubernetesError::InvalidPreflightPlan)?,
        })
    }
}

fn mib_quantity(mib: u32) -> String {
    if mib.is_multiple_of(1024) {
        format!("{}Gi", mib / 1024)
    } else {
        format!("{mib}Mi")
    }
}

/// Audit-safe request commitment. This data never establishes tenant identity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesPreflightTargetV1 {
    pub version: u32,
    pub authority_fingerprint: String,
    pub session_id: Uuid,
    pub operation_id: Uuid,
    pub pod_name: String,
    pub request_sha256: String,
    pub configuration_sha256: String,
}

/// Last synchronous trusted-control check after authority GETs, permit
/// acquisition and credential revalidation, immediately before the POST.
/// The real control implementation must authenticate, bind this exact target
/// and establish durable audit. A successful dry-run guard never admits work.
pub trait BeforeKubernetesPreflightGuard: Send {
    fn check_before_dry_run(
        self: Box<Self>,
        target: &KubernetesPreflightTargetV1,
    ) -> KubernetesResult<()>;
}

/// A server-side dry-run response validated against the closed requested
/// profile. It has no UID/handle, start acceptance, quota, usage or CNI proof.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesPreflightObservationV1 {
    pub version: u32,
    pub target: KubernetesPreflightTargetV1,
    pub admitted_configuration_sha256: String,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub admission_gaps: Vec<KubernetesAdmissionGapV1>,
}

impl KubernetesRuntime {
    pub async fn preflight(
        &self,
        plan: &KubernetesPreflightPlanV1,
        guard: Box<dyn BeforeKubernetesPreflightGuard>,
    ) -> KubernetesResult<KubernetesPreflightObservationV1> {
        let requested = plan.pod(&self.binding)?;
        let target = plan.target(&self.binding)?;
        self.verify_authority().await?;
        let admitted: Value = self
            .transport
            .dry_run_pod(&self.binding.namespace, &requested, &target, guard)
            .await?;
        // Pod CREATE has no namespace-UID precondition. The dry-run has no
        // persisted workload effect, but do not report success against a
        // namespace that changed between the preflight authority checks.
        self.verify_authority().await?;
        validate_admitted(&requested, &admitted)?;
        Ok(KubernetesPreflightObservationV1 {
            version: 1,
            target,
            admitted_configuration_sha256: KubernetesPodHandle::configuration_digest(
                &admitted["spec"],
            )?,
            observed_at: chrono::Utc::now(),
            admission_gaps: super::admission_gaps(),
        })
    }
}

pub(super) fn validate_admitted(requested: &Value, admitted: &Value) -> KubernetesResult<()> {
    if admitted.get("apiVersion") != requested.get("apiVersion")
        || admitted.get("kind") != requested.get("kind")
        || admitted.pointer("/metadata/name") != requested.pointer("/metadata/name")
        || admitted.pointer("/metadata/namespace") != requested.pointer("/metadata/namespace")
        || admitted.pointer("/metadata/labels") != requested.pointer("/metadata/labels")
        || admitted
            .pointer("/metadata/annotations")
            .is_some_and(|value| value != &json!({}))
        || admitted
            .pointer("/metadata/deletionTimestamp")
            .is_some_and(|value| !value.is_null())
        || admitted
            .pointer("/metadata/ownerReferences")
            .is_some_and(|value| value != &json!([]))
        || admitted
            .pointer("/metadata/finalizers")
            .is_some_and(|value| value != &json!([]))
    {
        return Err(KubernetesError::PreflightProfileChanged);
    }
    // Unknown admission additions are rejected. This intentionally requires a
    // new reviewed profile when a cluster needs additional scheduling defaults.
    let mut actual = admitted
        .get("spec")
        .cloned()
        .ok_or(KubernetesError::InvalidResponse)?;
    let fields = actual
        .as_object_mut()
        .ok_or(KubernetesError::InvalidResponse)?;
    // Non-pointer bool fields are omitted by the Kubernetes JSON encoder.
    // Their absence preserves false; true or another type still fails below.
    for field in ["hostNetwork", "hostPID", "hostIPC"] {
        fields.entry(field).or_insert(json!(false));
    }
    for (field, default) in [
        ("serviceAccount", json!("default")),
        ("preemptionPolicy", json!("PreemptLowerPriority")),
        ("priority", json!(0)),
    ] {
        if let Some(value) = fields.remove(field)
            && value != default
        {
            return Err(KubernetesError::PreflightProfileChanged);
        }
    }
    // Built-in DefaultTolerationSeconds adds these two scheduling tolerations.
    if let Some(tolerations) = fields.remove("tolerations") {
        let tolerations = tolerations
            .as_array()
            .ok_or(KubernetesError::PreflightProfileChanged)?;
        let mut keys = std::collections::BTreeSet::new();
        for toleration in tolerations {
            let key = toleration
                .get("key")
                .and_then(Value::as_str)
                .ok_or(KubernetesError::PreflightProfileChanged)?;
            if !matches!(
                key,
                "node.kubernetes.io/not-ready" | "node.kubernetes.io/unreachable"
            ) || !keys.insert(key)
                || toleration
                    != &json!({"key":key,"operator":"Exists","effect":"NoExecute","tolerationSeconds":300})
            {
                return Err(KubernetesError::PreflightProfileChanged);
            }
        }
    }
    if let Some(containers) = actual.get_mut("containers").and_then(Value::as_array_mut) {
        for container in containers {
            if let Some(fields) = container.as_object_mut() {
                for field in ["stdin", "tty"] {
                    fields.entry(field).or_insert(json!(false));
                }
            }
        }
    }
    if &actual
        != requested
            .get("spec")
            .ok_or(KubernetesError::InvalidPreflightPlan)?
    {
        return Err(KubernetesError::PreflightProfileChanged);
    }
    Ok(())
}
