use super::{PreparedHeldCreate, json::canonical_bytes};
use crate::{
    control::kubernetes_admission::{KubernetesCreateIntentV1, KubernetesCreateTargetV1},
    runtime::kubernetes::{KubernetesError, KubernetesPodHandle, KubernetesResult},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(super) fn prepare(intent: &KubernetesCreateIntentV1) -> KubernetesResult<PreparedHeldCreate> {
    intent
        .validate()
        .map_err(|_| KubernetesError::InvalidPreflightPlan)?;
    canonical_bytes(
        &serde_json::to_value(intent).map_err(|_| KubernetesError::InvalidPreflightPlan)?,
        16 * 1024,
    )?;
    let pod = pod(intent)?;
    let pod_bytes = canonical_bytes(&pod, 32 * 1024)?;
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:kubernetes-create-request:v1\0");
    hash.update(intent.authority.fingerprint().as_bytes());
    hash.update(&pod_bytes);
    Ok(PreparedHeldCreate {
        target: KubernetesCreateTargetV1 {
            version: 1,
            create_id: intent.create_id.clone(),
            session_id: intent.session_id.clone(),
            authority_fingerprint: intent.authority.fingerprint(),
            pod_name: pod_name(&intent.create_id)?,
            request_sha256: hex::encode(hash.finalize()),
            configuration_sha256: KubernetesPodHandle::configuration_digest(&pod["spec"])?,
        },
        pod_bytes,
    })
}

pub(super) fn pod(intent: &KubernetesCreateIntentV1) -> KubernetesResult<Value> {
    // Called only after the intention has been validated by prepare, or by
    // response validation through prepare. This renderer performs no effects.
    let resources = &intent.resources;
    resources.validate()?;
    let cpu = cpu_quantity(resources.cpu_millis);
    let limits = json!({"cpu":cpu,"memory":mib(resources.memory_mib),"ephemeral-storage":"1Mi"});
    let security = json!({"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,
        "privileged":false,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,
        "capabilities":{"drop":["ALL"]},"seccompProfile":{"type":"RuntimeDefault"}});
    let worker = json!({
        "name":"worker","image":intent.image,"imagePullPolicy":"Never",
        "command":["/usr/bin/id"],"workingDir":"/workspace","stdin":false,"tty":false,
        "terminationMessagePath":"/dev/termination-log","terminationMessagePolicy":"File",
        "securityContext":security,"resources":{"requests":limits,"limits":limits},
        "volumeMounts":[{"name":"workspace","mountPath":"/workspace"},
            {"name":"temporary","mountPath":"/tmp"}]
    });
    let pod_security = json!({"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,
        "fsGroup":1000,"fsGroupChangePolicy":"OnRootMismatch",
        "seccompProfile":{"type":"RuntimeDefault"}});
    let volumes = json!([
        {"name":"workspace","emptyDir":{"medium":"Memory","sizeLimit":mib(resources.workspace_mib)}},
        {"name":"temporary","emptyDir":{"medium":"Memory","sizeLimit":mib(resources.temporary_mib)}}
    ]);
    let spec = json!({
        "automountServiceAccountToken":false,"serviceAccountName":"default","enableServiceLinks":false,
        "hostNetwork":false,"hostPID":false,"hostIPC":false,"shareProcessNamespace":false,
        "restartPolicy":"Never","activeDeadlineSeconds":resources.deadline_seconds,
        "terminationGracePeriodSeconds":30,"dnsPolicy":"None","dnsConfig":{"nameservers":["127.0.0.1"]},
        "schedulerName":"default-scheduler","schedulingGates":[{"name":"ai-guard.io/held-create-v1"}],
        "securityContext":pod_security,"containers":[worker],"volumes":volumes
    });
    Ok(json!({"apiVersion":"v1","kind":"Pod","metadata":{
        "name":pod_name(&intent.create_id)?,"namespace":intent.authority.namespace(),
        "labels":{"app.kubernetes.io/managed-by":"ai-guard",
            "ai-guard/session":intent.session_id,"ai-guard/operation":intent.create_id,
            "ai-guard/purpose":"held-create"}},"spec":spec}))
}

fn pod_name(create_id: &str) -> KubernetesResult<String> {
    let id = Uuid::parse_str(create_id).map_err(|_| KubernetesError::InvalidPreflightPlan)?;
    if id.is_nil() || id.to_string() != create_id {
        return Err(KubernetesError::InvalidPreflightPlan);
    }
    Ok(format!("ai-guard-held-{}", id.simple()))
}
/// V1 canonical CPU form: whole cores as an integer, otherwise millicores.
/// Shared unchanged with the V2 session renderer; no runtime enforcement.
pub(in crate::runtime::kubernetes) fn cpu_quantity(millis: u32) -> String {
    if millis.is_multiple_of(1000) {
        (millis / 1000).to_string()
    } else {
        format!("{millis}m")
    }
}
/// V1 canonical memory form: `Gi` for whole multiples of 1024 MiB, else `Mi`.
pub(in crate::runtime::kubernetes) fn mib(value: u32) -> String {
    if value.is_multiple_of(1024) {
        format!("{}Gi", value / 1024)
    } else {
        format!("{value}Mi")
    }
}
