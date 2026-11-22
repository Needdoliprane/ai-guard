//! Workload admission of effectful V2 sessions (G11, R7).
//!
//! `admit_session_workload` is the opening of the formerly closed admission:
//! it reads, through the retained authority, every tenant prerequisite that
//! the session effects rely on and refuses at the first deviation, each with
//! its own closed reason. Its result is an observation of objects, never a
//! proof that the CNI enforces the NetworkPolicy, that the provisioner
//! enforces `ReadWriteOncePod` or bytes, or that no other subject can attach
//! or exec in the namespace: those stay qualification facts (plan steps 4a
//! and 9, R7 exclusivity). The separate reads are not an atomic snapshot.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "admission without a production caller until the broker driver (K4) lands"
    )
)]
use super::{
    KubernetesAuthorityV1, KubernetesError, KubernetesResult, KubernetesRuntime,
    prerequisites::{KubernetesPermissionActionV1, KubernetesPermissionDecisionV1, parse_review},
    session_claim::KubernetesSessionStorageRequestV2,
    valid_dns_label, valid_uid,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;

const ENFORCE: &str = "pod-security.kubernetes.io/enforce";
const ENFORCE_VERSION: &str = "pod-security.kubernetes.io/enforce-version";

/// Explicit activation of effectful Kubernetes sessions for one tenant
/// authority (R7). Server configuration only; absence keeps admission closed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionActivationV2 {
    pub version: u32,
    pub activated: bool,
    /// The one tenant authority this activation opens. Two tenants never
    /// share an authority id, an endpoint and namespace, or a namespace UID
    /// (R10, checked at configuration load by `refuse_shared_authorities`).
    pub authority: KubernetesAuthorityV1,
    /// Pinned Pod Security version of the tenant namespace, for example
    /// `v1.36`; `latest` is refused because it moves with the server.
    pub pod_security_version: String,
    /// Name of the single default-deny NetworkPolicy of the namespace.
    pub network_policy_name: String,
    pub storage: KubernetesSessionStorageRequestV2,
}
impl KubernetesSessionActivationV2 {
    pub fn validate(&self) -> KubernetesResult<()> {
        self.authority.validate()?;
        let version = self.pod_security_version.strip_prefix("v1.");
        if self.version == 1
            && version.is_some_and(|minor| {
                !minor.is_empty() && minor.len() <= 3 && minor.bytes().all(|b| b.is_ascii_digit())
            })
            && valid_dns_label(&self.network_policy_name)
        {
            self.storage.validate()
        } else {
            Err(KubernetesError::InvalidPreflightPlan)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KubernetesSessionAdmissionError {
    Kubernetes(KubernetesError),
    NotActivated,
    PodSecurityNotRestricted,
    NetworkPolicyNotDefaultDeny,
    ServiceAccountTokenAutomount,
    StorageClassNotCsi,
    StorageClassNotImmediate,
    PermissionMissing(KubernetesPermissionActionV1),
    PermissionNotDenied(KubernetesPermissionActionV1),
}
impl From<KubernetesError> for KubernetesSessionAdmissionError {
    fn from(error: KubernetesError) -> Self {
        Self::Kubernetes(error)
    }
}
impl fmt::Display for KubernetesSessionAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kubernetes(error) => write!(f, "Kubernetes session admission: {error}"),
            Self::NotActivated => f.write_str("Kubernetes sessions are not activated for this tenant authority"),
            Self::PodSecurityNotRestricted => f.write_str("the tenant namespace does not enforce the pinned Pod Security restricted level"),
            Self::NetworkPolicyNotDefaultDeny => f.write_str("the tenant namespace does not hold exactly the configured default-deny ingress and egress NetworkPolicy"),
            Self::ServiceAccountTokenAutomount => f.write_str("the default ServiceAccount of the tenant namespace may automount a token"),
            Self::StorageClassNotCsi => f.write_str("the configured StorageClass is not served by a registered CSI driver"),
            Self::StorageClassNotImmediate => f.write_str("the configured StorageClass does not bind Immediately"),
            Self::PermissionMissing(action) => write!(f, "the broker identity lacks a session permission: {action:?}"),
            Self::PermissionNotDenied(action) => write!(f, "the broker identity holds a forbidden permission or its review is unknown: {action:?}"),
        }
    }
}
impl std::error::Error for KubernetesSessionAdmissionError {}

/// Descriptive anchors of one admission. The NetworkPolicy UID and
/// resourceVersion are re-read and compared before every gate release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct KubernetesSessionAdmissionV2 {
    pub(crate) version: u32,
    pub(crate) authority_fingerprint: String,
    pub(crate) namespace_uid: String,
    pub(crate) network_policy_uid: String,
    pub(crate) network_policy_resource_version: String,
    pub(crate) storage_class_name: String,
    pub(crate) csi_driver: String,
}

type Admission<T> = Result<T, KubernetesSessionAdmissionError>;

impl KubernetesRuntime {
    /// Read-only. Refuses before any request when not activated.
    pub(crate) async fn admit_session_workload(
        &self,
        activation: &KubernetesSessionActivationV2,
    ) -> Admission<KubernetesSessionAdmissionV2> {
        activation.validate()?;
        if !activation.activated {
            return Err(KubernetesSessionAdmissionError::NotActivated);
        }
        // The activation opens exactly the authority this runtime is bound to.
        if activation.authority.fingerprint() != self.binding.fingerprint() {
            return Err(KubernetesSessionAdmissionError::NotActivated);
        }
        let namespace_identity = self.verify_active_authority().await?;
        let namespace = self.binding.namespace().to_owned();
        let labels: Value = self
            .transport
            .get_unique(&format!("/api/v1/namespaces/{namespace}"))
            .await?;
        if labels.pointer("/metadata/uid").and_then(Value::as_str)
            != Some(namespace_identity.metadata.uid.as_str())
        {
            return Err(KubernetesError::AuthorityChanged.into());
        }
        let label = |key: &str| {
            labels
                .pointer("/metadata/labels")
                .and_then(|labels| labels.get(key))
                .and_then(Value::as_str)
        };
        if label(ENFORCE) != Some("restricted")
            || label(ENFORCE_VERSION) != Some(activation.pod_security_version.as_str())
        {
            return Err(KubernetesSessionAdmissionError::PodSecurityNotRestricted);
        }
        let policies: Value = self
            .transport
            .get_unique(&format!(
                "/apis/networking.k8s.io/v1/namespaces/{namespace}/networkpolicies"
            ))
            .await?;
        let (network_policy_uid, network_policy_resource_version) =
            default_deny(&policies, &namespace, &activation.network_policy_name)?;
        let account: Value = self
            .transport
            .get_unique(&format!(
                "/api/v1/namespaces/{namespace}/serviceaccounts/default"
            ))
            .await?;
        if account.pointer("/automountServiceAccountToken") != Some(&json!(false)) {
            return Err(KubernetesSessionAdmissionError::ServiceAccountTokenAutomount);
        }
        let class_name = &activation.storage.storage_class_name;
        let class: Value = self
            .transport
            .get_unique(&format!(
                "/apis/storage.k8s.io/v1/storageclasses/{class_name}"
            ))
            .await?;
        if class.pointer("/metadata/name").and_then(Value::as_str) != Some(class_name.as_str()) {
            return Err(KubernetesError::InvalidResponse.into());
        }
        if class.get("volumeBindingMode").and_then(Value::as_str) != Some("Immediate") {
            return Err(KubernetesSessionAdmissionError::StorageClassNotImmediate);
        }
        let provisioner = class
            .get("provisioner")
            .and_then(Value::as_str)
            .filter(|name| valid_subdomain(name))
            .ok_or(KubernetesSessionAdmissionError::StorageClassNotCsi)?
            .to_owned();
        let driver = match self
            .transport
            .get_unique::<Value>(&format!("/apis/storage.k8s.io/v1/csidrivers/{provisioner}"))
            .await
        {
            Ok(driver) => driver,
            Err(KubernetesError::ApiRejected(404)) => {
                return Err(KubernetesSessionAdmissionError::StorageClassNotCsi);
            }
            Err(error) => return Err(error.into()),
        };
        if driver.pointer("/metadata/name").and_then(Value::as_str) != Some(provisioner.as_str()) {
            return Err(KubernetesSessionAdmissionError::StorageClassNotCsi);
        }
        self.require_session_permissions(&namespace).await?;
        // Separate reads do not form an atomic snapshot or exclude races.
        let after = self.verify_active_authority().await?;
        if after.metadata.uid != namespace_identity.metadata.uid {
            return Err(KubernetesError::AuthorityChanged.into());
        }
        Ok(KubernetesSessionAdmissionV2 {
            version: 1,
            authority_fingerprint: self.binding.fingerprint(),
            namespace_uid: namespace_identity.metadata.uid,
            network_policy_uid,
            network_policy_resource_version,
            storage_class_name: class_name.clone(),
            csi_driver: provisioner,
        })
    }

    /// Self reviews of this broker identity only: they show neither what
    /// other subjects may do nor a future authorization.
    async fn require_session_permissions(&self, namespace: &str) -> Admission<()> {
        for (actions, expected) in [
            (
                &KubernetesPermissionActionV1::SESSION_REQUIRED[..],
                KubernetesPermissionDecisionV1::Allowed,
            ),
            (
                &KubernetesPermissionActionV1::SESSION_FORBIDDEN[..],
                KubernetesPermissionDecisionV1::Denied,
            ),
        ] {
            for &action in actions {
                let response = self
                    .transport
                    .self_subject_access_review(namespace, action)
                    .await?;
                let review = parse_review(action, namespace, response)?;
                if review.decision != expected {
                    return Err(if expected == KubernetesPermissionDecisionV1::Allowed {
                        KubernetesSessionAdmissionError::PermissionMissing(action)
                    } else {
                        KubernetesSessionAdmissionError::PermissionNotDenied(action)
                    });
                }
            }
        }
        Ok(())
    }
}

/// R10: refuses, at configuration load, two tenants whose activations share
/// an authority id, an endpoint and namespace, or a namespace UID. The
/// cluster itself would let them; ai-guard never does.
pub fn refuse_shared_authorities<'a>(
    activations: impl IntoIterator<Item = (&'a str, &'a KubernetesSessionActivationV2)>,
) -> anyhow::Result<()> {
    let mut ids = std::collections::BTreeMap::new();
    let mut namespaces = std::collections::BTreeMap::new();
    let mut uids = std::collections::BTreeMap::new();
    for (tenant, activation) in activations {
        activation.validate().map_err(|_| {
            anyhow::anyhow!("invalid Kubernetes session activation for tenant {tenant}")
        })?;
        let authority = &activation.authority;
        for (index, key) in [
            (&mut ids, authority.authority_id().to_string()),
            (
                &mut namespaces,
                format!("{}\0{}", authority.endpoint(), authority.namespace()),
            ),
            (&mut uids, authority.namespace_uid().to_owned()),
        ] {
            if let Some(other) = index.insert(key, tenant.to_owned()) {
                anyhow::bail!(
                    "tenants {other} and {tenant} share a Kubernetes authority or namespace; each tenant needs its own (R10)"
                );
            }
        }
    }
    Ok(())
}

/// Exactly one policy, named as configured, selecting every Pod and denying
/// all ingress and egress. Any other policy could only add allowed traffic.
fn default_deny(policies: &Value, namespace: &str, name: &str) -> Admission<(String, String)> {
    let refused = KubernetesSessionAdmissionError::NetworkPolicyNotDefaultDeny;
    if policies.get("kind").and_then(Value::as_str) != Some("NetworkPolicyList") {
        return Err(KubernetesError::InvalidResponse.into());
    }
    let items = policies.get("items").and_then(Value::as_array).ok_or(
        KubernetesSessionAdmissionError::Kubernetes(KubernetesError::InvalidResponse),
    )?;
    let [policy] = items.as_slice() else {
        return Err(refused);
    };
    let text = |pointer: &str| policy.pointer(pointer).and_then(Value::as_str);
    let empty_rules = |key: &str| {
        policy
            .pointer(&format!("/spec/{key}"))
            .is_none_or(|rules| rules.as_array().is_some_and(Vec::is_empty))
    };
    let mut types: Vec<&str> = policy
        .pointer("/spec/policyTypes")
        .and_then(Value::as_array)
        .map(|types| types.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    types.sort_unstable();
    let spec_keys_closed = policy
        .get("spec")
        .and_then(Value::as_object)
        .is_some_and(|spec| {
            spec.keys().all(|key| {
                ["podSelector", "policyTypes", "ingress", "egress"].contains(&key.as_str())
            })
        });
    if text("/metadata/name") != Some(name)
        || text("/metadata/namespace") != Some(namespace)
        || policy
            .pointer("/metadata/deletionTimestamp")
            .is_some_and(|value| !value.is_null())
        || policy.pointer("/spec/podSelector") != Some(&json!({}))
        || types != ["Egress", "Ingress"]
        || !empty_rules("ingress")
        || !empty_rules("egress")
        || !spec_keys_closed
    {
        return Err(refused);
    }
    let uid = text("/metadata/uid").filter(|uid| valid_uid(uid)).ok_or(
        KubernetesSessionAdmissionError::Kubernetes(KubernetesError::InvalidResponse),
    )?;
    let resource_version = text("/metadata/resourceVersion")
        .filter(|version| !version.is_empty() && version.len() <= 128)
        .ok_or(KubernetesSessionAdmissionError::Kubernetes(
            KubernetesError::InvalidResponse,
        ))?;
    Ok((uid.into(), resource_version.into()))
}

fn valid_subdomain(value: &str) -> bool {
    !value.is_empty() && value.len() <= 253 && value.split('.').all(valid_dns_label)
}

#[cfg(test)]
#[path = "session_admission_tests.rs"]
mod tests;
