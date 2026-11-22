//! Opt-in observations of namespace readiness and the current identity's access.
//! SSAR answers cover only the explicit queried actions, not actor exclusivity,
//! admission, CNI enforcement, named-resource grants or future authorization.
use super::{
    KubernetesDiagnosticsV1, KubernetesError, KubernetesResult, KubernetesRuntime, Namespace,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

const DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(30);

// Kept separate from the historical GET-only Namespace decoder so that opting
// out retains its parsing contract as well as the report's serialized shape.
#[derive(Deserialize)]
struct NamespaceReadiness {
    #[serde(flatten)]
    identity: Namespace,
    #[serde(default)]
    status: Option<NamespaceStatus>,
}
#[derive(Deserialize)]
struct NamespaceStatus {
    #[serde(default)]
    phase: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesNamespaceStateV1 {
    Active,
    Terminating,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesNamespacePrerequisiteV1 {
    pub name: String,
    pub uid: String,
    pub state: KubernetesNamespaceStateV1,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesPermissionActionV1 {
    GetPods,
    CreatePods,
    PatchPods,
    UpdatePods,
    DeletePods,
    CreatePodBinding,
    CreatePodExec,
    CreatePodAttach,
    GetPodLogs,
    GetSecrets,
    ListSecrets,
    CreateServiceAccountToken,
    /// G11 session admission (R5, D4): the broker creates and deletes the
    /// per-attempt claim and reads the tenant's NetworkPolicies and default
    /// ServiceAccount. Not part of the legacy twelve doctor reviews.
    GetPersistentVolumeClaims,
    CreatePersistentVolumeClaims,
    DeletePersistentVolumeClaims,
    ListNetworkPolicies,
    GetServiceAccounts,
    /// Must stay denied: an ephemeral container or a port forward would
    /// bypass the closed Pod request and the attach-only channel.
    PatchPodEphemeralContainers,
    CreatePodPortForward,
}
impl KubernetesPermissionActionV1 {
    /// Reviews required by `admit_session_workload`: allowed for the session
    /// lifecycle, then denied for every bypass of it.
    pub(super) const SESSION_REQUIRED: [Self; 10] = [
        Self::GetPods,
        Self::CreatePods,
        Self::PatchPods,
        Self::DeletePods,
        Self::CreatePodAttach,
        Self::GetPersistentVolumeClaims,
        Self::CreatePersistentVolumeClaims,
        Self::DeletePersistentVolumeClaims,
        Self::ListNetworkPolicies,
        Self::GetServiceAccounts,
    ];
    pub(super) const SESSION_FORBIDDEN: [Self; 9] = [
        Self::UpdatePods,
        Self::CreatePodBinding,
        Self::CreatePodExec,
        Self::GetPodLogs,
        Self::GetSecrets,
        Self::ListSecrets,
        Self::CreateServiceAccountToken,
        Self::PatchPodEphemeralContainers,
        Self::CreatePodPortForward,
    ];
    pub(super) const ALL: [Self; 12] = [
        Self::GetPods,
        Self::CreatePods,
        Self::PatchPods,
        Self::UpdatePods,
        Self::DeletePods,
        Self::CreatePodBinding,
        Self::CreatePodExec,
        Self::CreatePodAttach,
        Self::GetPodLogs,
        Self::GetSecrets,
        Self::ListSecrets,
        Self::CreateServiceAccountToken,
    ];

    pub(super) fn attributes(self, namespace: &str) -> Value {
        let mut group = "";
        let (verb, resource, subresource) = match self {
            Self::GetPods => ("get", "pods", None),
            Self::CreatePods => ("create", "pods", None),
            Self::PatchPods => ("patch", "pods", None),
            Self::UpdatePods => ("update", "pods", None),
            Self::DeletePods => ("delete", "pods", None),
            Self::CreatePodBinding => ("create", "pods", Some("binding")),
            Self::CreatePodExec => ("create", "pods", Some("exec")),
            Self::CreatePodAttach => ("create", "pods", Some("attach")),
            Self::GetPodLogs => ("get", "pods", Some("log")),
            Self::GetSecrets => ("get", "secrets", None),
            Self::ListSecrets => ("list", "secrets", None),
            Self::CreateServiceAccountToken => ("create", "serviceaccounts", Some("token")),
            Self::GetPersistentVolumeClaims => ("get", "persistentvolumeclaims", None),
            Self::CreatePersistentVolumeClaims => ("create", "persistentvolumeclaims", None),
            Self::DeletePersistentVolumeClaims => ("delete", "persistentvolumeclaims", None),
            Self::ListNetworkPolicies => {
                group = "networking.k8s.io";
                ("list", "networkpolicies", None)
            }
            Self::GetServiceAccounts => ("get", "serviceaccounts", None),
            Self::PatchPodEphemeralContainers => ("patch", "pods", Some("ephemeralcontainers")),
            Self::CreatePodPortForward => ("create", "pods", Some("portforward")),
        };
        // No caller-controlled verb, resource, identity or all-namespace scope.
        let mut attributes = json!({
            "namespace":namespace, "group":group, "version":"v1",
            "resource":resource, "verb":verb
        });
        if let Some(subresource) = subresource {
            attributes["subresource"] = json!(subresource);
        }
        attributes
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesPermissionDecisionV1 {
    Allowed,
    Denied,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesAccessReviewV1 {
    pub action: KubernetesPermissionActionV1,
    pub decision: KubernetesPermissionDecisionV1,
    pub evaluation_error_present: bool,
    pub observed_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesPermissionReviewStatusV1 {
    Complete,
    SkippedNamespaceNotActive,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesPermissionCoverageV1 {
    QueriedActionsOnly,
}

/// Descriptive API observations, never a serialized capability or qualification.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesPrerequisitesV1 {
    pub version: u32,
    pub authority_fingerprint: String,
    pub cluster_namespace: KubernetesNamespacePrerequisiteV1,
    pub namespace: KubernetesNamespacePrerequisiteV1,
    pub permission_review_status: KubernetesPermissionReviewStatusV1,
    pub permission_coverage: KubernetesPermissionCoverageV1,
    pub access_reviews: Vec<KubernetesAccessReviewV1>,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

fn namespace_state(namespace: &NamespaceReadiness) -> KubernetesNamespaceStateV1 {
    if namespace.identity.metadata.deletion_timestamp.is_some()
        || namespace
            .status
            .as_ref()
            .and_then(|status| status.phase.as_deref())
            == Some("Terminating")
    {
        KubernetesNamespaceStateV1::Terminating
    } else if namespace
        .status
        .as_ref()
        .and_then(|status| status.phase.as_deref())
        == Some("Active")
    {
        KubernetesNamespaceStateV1::Active
    } else {
        KubernetesNamespaceStateV1::Unknown
    }
}

fn namespace_observation(namespace: &NamespaceReadiness) -> KubernetesNamespacePrerequisiteV1 {
    KubernetesNamespacePrerequisiteV1 {
        name: namespace.identity.metadata.name.clone(),
        uid: namespace.identity.metadata.uid.clone(),
        state: namespace_state(namespace),
    }
}

impl KubernetesRuntime {
    async fn inspect_authority_namespaces(
        &self,
    ) -> KubernetesResult<(NamespaceReadiness, NamespaceReadiness)> {
        let cluster: NamespaceReadiness = self
            .transport
            .get_unique("/api/v1/namespaces/kube-system")
            .await?;
        let namespace: NamespaceReadiness = self
            .transport
            .get_unique(&format!("/api/v1/namespaces/{}", self.binding.namespace()))
            .await?;
        for (actual, name, uid) in [
            (
                &cluster,
                "kube-system",
                self.binding.cluster_namespace_uid.as_str(),
            ),
            (
                &namespace,
                self.binding.namespace(),
                self.binding.namespace_uid.as_str(),
            ),
        ] {
            if actual.identity.api_version != "v1"
                || actual.identity.kind != "Namespace"
                || actual.identity.metadata.name != name
                || actual.identity.metadata.uid != uid
            {
                return Err(KubernetesError::AuthorityChanged);
            }
        }
        Ok((cluster, namespace))
    }

    /// Explicitly performs SSAR POSTs; the legacy doctor remains GET-only.
    /// A successful response does not grant admission or prove confinement.
    pub async fn doctor_with_prerequisites(
        &self,
    ) -> KubernetesResult<(KubernetesDiagnosticsV1, KubernetesPrerequisitesV1)> {
        tokio::time::timeout(DIAGNOSTIC_TIMEOUT, self.inspect_prerequisites())
            .await
            .map_err(|_| KubernetesError::Timeout)?
    }

    async fn inspect_prerequisites(
        &self,
    ) -> KubernetesResult<(KubernetesDiagnosticsV1, KubernetesPrerequisitesV1)> {
        let started_at = Utc::now();
        let (cluster, namespace) = self.inspect_authority_namespaces().await?;
        let diagnostics = self.diagnostics_for(&namespace.identity).await?;
        let mut prerequisites = KubernetesPrerequisitesV1 {
            version: 1,
            authority_fingerprint: self.binding.fingerprint(),
            cluster_namespace: namespace_observation(&cluster),
            namespace: namespace_observation(&namespace),
            permission_review_status: KubernetesPermissionReviewStatusV1::SkippedNamespaceNotActive,
            permission_coverage: KubernetesPermissionCoverageV1::QueriedActionsOnly,
            access_reviews: Vec::new(),
            started_at,
            completed_at: Utc::now(),
        };
        if namespace_state(&cluster) == KubernetesNamespaceStateV1::Active
            && namespace_state(&namespace) == KubernetesNamespaceStateV1::Active
        {
            for action in KubernetesPermissionActionV1::ALL {
                let response = self
                    .transport
                    .self_subject_access_review(self.binding.namespace(), action)
                    .await?;
                prerequisites.access_reviews.push(parse_review(
                    action,
                    self.binding.namespace(),
                    response,
                )?);
            }
            // Separate reads do not form an atomic snapshot or exclude races.
            self.verify_active_authority().await?;
            prerequisites.permission_review_status = KubernetesPermissionReviewStatusV1::Complete;
        }
        prerequisites.completed_at = Utc::now();
        Ok((diagnostics, prerequisites))
    }

    /// Current create/pods opinion only. This is not an activation capability,
    /// a check of other rights, or a guarantee about a later authorization.
    pub(super) async fn verify_preflight_create_permission(&self) -> KubernetesResult<()> {
        let action = KubernetesPermissionActionV1::CreatePods;
        let response = self
            .transport
            .self_subject_access_review(self.binding.namespace(), action)
            .await?;
        let review = parse_review(action, self.binding.namespace(), response)?;
        match review.decision {
            KubernetesPermissionDecisionV1::Allowed => Ok(()),
            KubernetesPermissionDecisionV1::Denied => {
                Err(KubernetesError::PreflightPermissionDenied)
            }
            KubernetesPermissionDecisionV1::Unknown => {
                Err(KubernetesError::PreflightPermissionUnconfirmed)
            }
        }
    }

    pub(super) async fn verify_active_authority(&self) -> KubernetesResult<Namespace> {
        let (cluster, namespace) = self.inspect_authority_namespaces().await?;
        if namespace_state(&cluster) != KubernetesNamespaceStateV1::Active
            || namespace_state(&namespace) != KubernetesNamespaceStateV1::Active
        {
            return Err(KubernetesError::NamespaceNotActive);
        }
        Ok(namespace.identity)
    }
}

pub(super) fn parse_review(
    action: KubernetesPermissionActionV1,
    namespace: &str,
    response: Value,
) -> KubernetesResult<KubernetesAccessReviewV1> {
    let invalid = || KubernetesError::InvalidResponse;
    if response.get("apiVersion").and_then(Value::as_str) != Some("authorization.k8s.io/v1")
        || response.get("kind").and_then(Value::as_str) != Some("SelfSubjectAccessReview")
    {
        return Err(invalid());
    }
    let spec = response
        .get("spec")
        .and_then(Value::as_object)
        .ok_or_else(invalid)?;
    if spec.len() != 1 {
        return Err(invalid());
    }
    let mut actual = spec
        .get("resourceAttributes")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(invalid)?;
    // The API's omitempty encoder omits the empty core group and resource name.
    actual.entry("group").or_insert(json!(""));
    if actual.get("name") == Some(&json!("")) {
        actual.remove("name");
    }
    if Value::Object(actual) != action.attributes(namespace) {
        return Err(invalid());
    }
    let status = response
        .get("status")
        .and_then(Value::as_object)
        .ok_or_else(invalid)?;
    if status
        .keys()
        .any(|key| !["allowed", "denied", "reason", "evaluationError"].contains(&key.as_str()))
    {
        return Err(invalid());
    }
    let allowed = status
        .get("allowed")
        .and_then(Value::as_bool)
        .ok_or_else(invalid)?;
    let denied = match status.get("denied") {
        None => false,
        Some(value) => value.as_bool().ok_or_else(invalid)?,
    };
    let text = |field: &str| -> KubernetesResult<&str> {
        match status.get(field) {
            None => Ok(""),
            Some(Value::String(value)) if value.len() <= 8192 => Ok(value),
            _ => Err(invalid()),
        }
    };
    let _ = text("reason")?; // Never export or retain an untrusted remote reason.
    let evaluation_error_present = !text("evaluationError")?.is_empty();
    if allowed && denied {
        return Err(invalid());
    }
    let decision = if evaluation_error_present {
        KubernetesPermissionDecisionV1::Unknown
    } else if allowed {
        KubernetesPermissionDecisionV1::Allowed
    } else if denied {
        KubernetesPermissionDecisionV1::Denied
    } else {
        KubernetesPermissionDecisionV1::Unknown
    };
    Ok(KubernetesAccessReviewV1 {
        action,
        decision,
        evaluation_error_present,
        observed_at: Utc::now(),
    })
}
