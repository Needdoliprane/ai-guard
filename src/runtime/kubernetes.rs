//! Explicit Kubernetes authority, inspection, preflight and internal held CREATE.
//!
//! Held CREATE remains closed in production. This is not a workload launcher;
//! there is no exec, kubeconfig or ambient context interface, and no public
//! attach interface. The V2 CREATE, release PATCH, DELETE and preamble-only
//! attach mechanics are crate-internal behind a sealed guard that has no
//! production implementor. That attach carries one bootstrap Hello and its
//! Ready echo between two observations of the exact instance, then closes its
//! stdin; it starts nothing and proves no identity by itself. The worker
//! waiting mode that answers it (`session_worker`) is a public library entry
//! for the existing provider binary and executes nothing. The V2
//! before-effect lifecycle (abandonment, successor recovery, observe-only
//! report) is crate-internal and has no production caller yet.
//! Pod API configuration is not proof of runtime enforcement or CPU usage.
use crate::control_fs::PrivateFile;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(crate) mod admission;
pub(crate) mod bootstrap;
mod observation;
mod preflight;
mod prerequisites;
#[cfg(test)]
#[path = "kubernetes/prerequisites_tests.rs"]
mod prerequisites_tests;
pub(crate) mod session;
pub(crate) mod session_admission;
pub(crate) mod session_attach;
pub(crate) mod session_claim;
pub(crate) mod session_driver;
pub(crate) mod session_effect;
pub(crate) mod session_instance;
pub(crate) mod session_lifecycle;
pub(crate) mod session_permit;
pub(crate) mod session_pod;
pub mod session_profile;
pub(crate) mod session_protocol;
pub(crate) mod session_requests;
pub(crate) mod session_supervisor;
#[cfg(test)]
#[path = "kubernetes/session_test_peer.rs"]
pub(crate) mod session_test_peer;
pub mod session_worker;
mod transport;
pub use observation::{
    KubernetesConfiguredLimitsV1, KubernetesPodHandle, KubernetesPodObservationV1,
    KubernetesProcessObservationV1, KubernetesProcessStateV1,
};
pub use preflight::{
    BeforeKubernetesPreflightGuard, KubernetesPreflightObservationV1, KubernetesPreflightPlanV1,
    KubernetesPreflightResourcesV1, KubernetesPreflightTargetV1,
};
pub use prerequisites::{
    KubernetesAccessReviewV1, KubernetesNamespacePrerequisiteV1, KubernetesNamespaceStateV1,
    KubernetesPermissionActionV1, KubernetesPermissionCoverageV1, KubernetesPermissionDecisionV1,
    KubernetesPermissionReviewStatusV1, KubernetesPrerequisitesV1,
};
pub use session_claim::KubernetesSessionStorageRequestV2;
pub use session_instance::KubernetesSessionWorkspaceClaimV2;
use transport::Transport;

pub type KubernetesResult<T> = Result<T, KubernetesError>;

/// Closed errors intentionally omit server bodies, URLs and transport chains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KubernetesError {
    InvalidAuthority,
    InvalidCredentials,
    TransportUnavailable,
    Timeout,
    Busy,
    ResponseTooLarge,
    ApiRejected(u16),
    InvalidResponse,
    AuthorityChanged,
    NamespaceNotActive,
    PodChanged,
    InvalidPreflightPlan,
    PreflightAuthorizationDenied,
    PreflightPermissionDenied,
    PreflightPermissionUnconfirmed,
    PreflightProfileChanged,
    HeldCreateAuthorizationDenied,
    HeldCreateControlUnavailable,
    HeldCreateJournalUnavailable,
    WorkloadAdmissionUnavailable,
}
impl std::fmt::Display for KubernetesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAuthority => f.write_str("Kubernetes authority configuration is invalid"),
            Self::InvalidCredentials => f.write_str("Kubernetes private credentials or trust are invalid or unavailable"),
            Self::TransportUnavailable => f.write_str("Kubernetes HTTPS transport is unavailable"),
            Self::Timeout => f.write_str("Kubernetes HTTPS request timed out"),
            Self::Busy => f.write_str("Kubernetes HTTPS request capacity is occupied; retry inspection"),
            Self::ResponseTooLarge => f.write_str("Kubernetes API response exceeds the inspection limit"),
            Self::ApiRejected(status) => write!(f, "Kubernetes API rejected inspection with HTTP {status}"),
            Self::InvalidResponse => f.write_str("Kubernetes API inspection response is invalid or incomplete"),
            Self::AuthorityChanged => f.write_str("Kubernetes cluster or namespace identity differs from the retained authority"),
            Self::NamespaceNotActive => f.write_str("Kubernetes namespace readiness is inactive or unknown; an observed Active phase is required before admission preflight or held create"),
            Self::PodChanged => f.write_str("Kubernetes Pod identity or configuration differs from the retained handle"),
            Self::InvalidPreflightPlan => f.write_str("Kubernetes admission preflight plan is invalid or differs from the retained authority"),
            Self::PreflightAuthorizationDenied => f.write_str("Kubernetes admission preflight was not durably authorized by the control"),
            Self::PreflightPermissionDenied => f.write_str("Kubernetes Pod dry-run create permission is denied; verify the current inspection identity's permissions"),
            Self::PreflightPermissionUnconfirmed => f.write_str("Kubernetes Pod dry-run create permission is unconfirmed; inspect the authorization review and current permissions"),
            Self::PreflightProfileChanged => f.write_str("Kubernetes admission changed the closed preflight profile; review the cluster admission configuration"),
            Self::HeldCreateAuthorizationDenied => f.write_str("Held Kubernetes create is not authorized for this exact retained request"),
            Self::HeldCreateControlUnavailable => f.write_str("Held Kubernetes create control confirmation is unavailable"),
            Self::HeldCreateJournalUnavailable => f.write_str("Held Kubernetes create private journal is unavailable or requires reconciliation"),
            Self::WorkloadAdmissionUnavailable => f.write_str("Kubernetes workload admission is unavailable: confinement, workspace transfer and durable supervision remain unqualified"),
        }
    }
}
impl std::error::Error for KubernetesError {}

/// Private control checkpoint metadata, never a tenant selection from a worker.
/// The cluster anchor is the explicitly provisioned kube-system namespace UID;
/// Kubernetes does not expose a universal cluster UUID.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesAuthorityV1 {
    version: u32,
    authority_id: Uuid,
    endpoint: String,
    cluster_namespace_uid: String,
    namespace: String,
    namespace_uid: String,
    trust_sha256: String,
}
impl std::fmt::Debug for KubernetesAuthorityV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KubernetesAuthorityV1")
            .field("authority_id", &self.authority_id)
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}
impl KubernetesAuthorityV1 {
    pub fn new(
        authority_id: Uuid,
        endpoint: &str,
        cluster_namespace_uid: &str,
        namespace: &str,
        namespace_uid: &str,
        trust_sha256: &str,
    ) -> KubernetesResult<Self> {
        let binding = Self {
            version: 1,
            authority_id,
            endpoint: endpoint.into(),
            cluster_namespace_uid: cluster_namespace_uid.into(),
            namespace: namespace.into(),
            namespace_uid: namespace_uid.into(),
            trust_sha256: trust_sha256.into(),
        };
        binding.validate()?;
        Ok(binding)
    }
    pub fn validate(&self) -> KubernetesResult<()> {
        let url =
            reqwest::Url::parse(&self.endpoint).map_err(|_| KubernetesError::InvalidAuthority)?;
        if self.version != 1
            || self.authority_id.is_nil()
            || self.endpoint.len() > 2048
            || url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || !valid_dns_label(&self.namespace)
            || matches!(
                self.namespace.as_str(),
                "default" | "kube-system" | "kube-public" | "kube-node-lease"
            )
            || !valid_uid(&self.cluster_namespace_uid)
            || !valid_uid(&self.namespace_uid)
            || self.cluster_namespace_uid == self.namespace_uid
            || !valid_digest(&self.trust_sha256)
        {
            return Err(KubernetesError::InvalidAuthority);
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"ai-guard:kubernetes-authority:v1\0");
        for field in [
            self.version.to_string(),
            self.authority_id.to_string(),
            self.endpoint.clone(),
            self.cluster_namespace_uid.clone(),
            self.namespace.clone(),
            self.namespace_uid.clone(),
            self.trust_sha256.clone(),
        ] {
            digest.update((field.len() as u64).to_be_bytes());
            digest.update(field.as_bytes());
        }
        hex::encode(digest.finalize())
    }
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub fn namespace_uid(&self) -> &str {
        &self.namespace_uid
    }
    pub fn authority_id(&self) -> Uuid {
        self.authority_id
    }
    /// An authority binding is never an admission or an enforcement attestation.
    pub fn require_workload_admission(&self) -> KubernetesResult<()> {
        self.validate()?;
        Err(KubernetesError::WorkloadAdmissionUnavailable)
    }
}

pub(super) fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
pub(crate) fn valid_immutable_image(image: &str) -> bool {
    image.split_once("@sha256:").is_some_and(|(name, digest)| {
        !name.is_empty()
            && image.len() <= 256
            && (name.as_bytes()[0].is_ascii_lowercase() || name.as_bytes()[0].is_ascii_digit())
            && !name.contains("..")
            && !name.contains("//")
            && name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"/._-:".contains(&byte)
            })
            && valid_digest(digest)
    })
}
pub(super) fn valid_uid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|uid| !uid.is_nil() && uid.to_string() == value)
}
pub(super) fn valid_dns_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && (value.as_bytes()[0].is_ascii_lowercase() || value.as_bytes()[0].is_ascii_digit())
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Retained static credential capabilities. No kubeconfig, exec plugin,
/// ambient file lookup, automatic reload, serialization or credential Debug.
pub struct KubernetesCredentials {
    ca: PrivateFile,
    authentication: Authentication,
}
enum Authentication {
    Bearer(PrivateFile),
    MutualTls {
        certificate: PrivateFile,
        key: PrivateFile,
    },
}
impl std::fmt::Debug for KubernetesCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KubernetesCredentials(<private retained capabilities>)")
    }
}
impl KubernetesCredentials {
    pub fn bearer(ca: PrivateFile, token: PrivateFile) -> Self {
        Self {
            ca,
            authentication: Authentication::Bearer(token),
        }
    }
    pub fn mutual_tls(ca: PrivateFile, certificate: PrivateFile, key: PrivateFile) -> Self {
        Self {
            ca,
            authentication: Authentication::MutualTls { certificate, key },
        }
    }
    fn revalidate(&self) -> KubernetesResult<()> {
        let check = |file: &PrivateFile| {
            file.revalidate()
                .map_err(|_| KubernetesError::InvalidCredentials)
        };
        check(&self.ca)?;
        match &self.authentication {
            Authentication::Bearer(token) => check(token),
            Authentication::MutualTls { certificate, key } => {
                check(certificate)?;
                check(key)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesAdmissionGapV1 {
    NetworkEnforcementNotProven,
    ContainerConfinementNotQualified,
    PerPodPidLimitNotProven,
    TemporaryFilesystemNoexecNotProven,
    WorkerLogPersistenceNotPrevented,
    WorkspaceTransferNotImplemented,
    ExecAttachNotImplemented,
    DurableDeadlineNotImplemented,
    ControlAuditAdmissionNotConnected,
    ServiceAccountRbacNotQualified,
}
impl KubernetesAdmissionGapV1 {
    pub fn reason(self) -> &'static str {
        match self {
            Self::NetworkEnforcementNotProven => {
                "A NetworkPolicy object does not prove ingress, egress, host or cross-tenant isolation; real CNI enforcement tests are required."
            }
            Self::ContainerConfinementNotQualified => {
                "Pod security declarations have not been qualified by runtime observations and bypass tests."
            }
            Self::PerPodPidLimitNotProven => {
                "The Kubernetes Pod API alone does not establish the required per-worker process limit."
            }
            Self::TemporaryFilesystemNoexecNotProven => {
                "Temporary storage noexec, nosuid and nodev enforcement has not been established."
            }
            Self::WorkerLogPersistenceNotPrevented => {
                "Preventing kubelet/container-runtime persistence of unredacted worker output has not been established."
            }
            Self::WorkspaceTransferNotImplemented => {
                "Filtered workspace import and verified export to a fresh destination are not connected to this adapter."
            }
            Self::ExecAttachNotImplemented => {
                "The authenticated bounded Kubernetes streaming transport is not implemented."
            }
            Self::DurableDeadlineNotImplemented => {
                "A durable independently enforced execution deadline and recovery have not been qualified."
            }
            Self::ControlAuditAdmissionNotConnected => {
                "Durable decision, audit and single-use start authorization are not connected to Kubernetes workload creation."
            }
            Self::ServiceAccountRbacNotQualified => {
                "Control-plane RBAC and absence of worker service-account credentials require qualification."
            }
        }
    }
}

/// These are authenticated API observations, not runtime enforcement proofs.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KubernetesDiagnosticsV1 {
    pub authority_fingerprint: String,
    pub server_version: String,
    pub namespace: String,
    pub namespace_uid: String,
    pub cluster_namespace_uid: String,
    /// Observed namespace label, not proof of effective runtime confinement.
    pub namespace_pod_security_enforce_label: Option<String>,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub admission_gaps: Vec<KubernetesAdmissionGapV1>,
}

/// Inspection and internal held CREATE; production workload admission stays closed.
pub struct KubernetesRuntime {
    binding: KubernetesAuthorityV1,
    transport: Transport,
}
impl std::fmt::Debug for KubernetesRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KubernetesRuntime")
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}
impl KubernetesRuntime {
    pub fn connect(
        binding: KubernetesAuthorityV1,
        credentials: KubernetesCredentials,
    ) -> KubernetesResult<Self> {
        binding.validate()?;
        let transport = Transport::new(&binding, credentials)?;
        Ok(Self { binding, transport })
    }
    pub fn binding(&self) -> &KubernetesAuthorityV1 {
        &self.binding
    }
    async fn verify_authority(&self) -> KubernetesResult<Namespace> {
        let cluster: Namespace = self.transport.get("/api/v1/namespaces/kube-system").await?;
        let namespace: Namespace = self
            .transport
            .get(&format!("/api/v1/namespaces/{}", self.binding.namespace))
            .await?;
        for (actual, name, uid) in [
            (
                &cluster,
                "kube-system",
                self.binding.cluster_namespace_uid.as_str(),
            ),
            (
                &namespace,
                self.binding.namespace.as_str(),
                self.binding.namespace_uid.as_str(),
            ),
        ] {
            if actual.api_version != "v1"
                || actual.kind != "Namespace"
                || actual.metadata.name != name
                || actual.metadata.uid != uid
                || actual.metadata.deletion_timestamp.is_some()
            {
                return Err(KubernetesError::AuthorityChanged);
            }
        }
        Ok(namespace)
    }
    pub async fn doctor(&self) -> KubernetesResult<KubernetesDiagnosticsV1> {
        let namespace = self.verify_authority().await?;
        self.diagnostics_for(&namespace).await
    }
    async fn diagnostics_for(
        &self,
        namespace: &Namespace,
    ) -> KubernetesResult<KubernetesDiagnosticsV1> {
        #[derive(Deserialize)]
        struct Version {
            #[serde(rename = "gitVersion")]
            git_version: String,
        }
        let version: Version = self.transport.get("/version").await?;
        if version.git_version.is_empty()
            || version.git_version.len() > 128
            || !version
                .git_version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".+-_".contains(&byte))
        {
            return Err(KubernetesError::InvalidResponse);
        }
        Ok(KubernetesDiagnosticsV1 {
            authority_fingerprint: self.binding.fingerprint(),
            server_version: version.git_version,
            namespace: self.binding.namespace.clone(),
            namespace_uid: self.binding.namespace_uid.clone(),
            cluster_namespace_uid: self.binding.cluster_namespace_uid.clone(),
            namespace_pod_security_enforce_label: namespace
                .metadata
                .labels
                .get("pod-security.kubernetes.io/enforce")
                .filter(|value| matches!(value.as_str(), "restricted" | "baseline" | "privileged"))
                .cloned(),
            observed_at: chrono::Utc::now(),
            admission_gaps: admission_gaps(),
        })
    }
    pub async fn observe_owned(
        &self,
        handle: &KubernetesPodHandle,
    ) -> KubernetesResult<KubernetesPodObservationV1> {
        handle.validate(&self.binding)?;
        self.verify_authority().await?;
        let pod: serde_json::Value = self
            .transport
            .get(&format!(
                "/api/v1/namespaces/{}/pods/{}",
                self.binding.namespace,
                handle.name()
            ))
            .await?;
        observation::parse(&self.binding, handle, pod)
    }
}

fn admission_gaps() -> Vec<KubernetesAdmissionGapV1> {
    use KubernetesAdmissionGapV1::*;
    vec![
        NetworkEnforcementNotProven,
        ContainerConfinementNotQualified,
        PerPodPidLimitNotProven,
        TemporaryFilesystemNoexecNotProven,
        WorkerLogPersistenceNotPrevented,
        WorkspaceTransferNotImplemented,
        ExecAttachNotImplemented,
        DurableDeadlineNotImplemented,
        ControlAuditAdmissionNotConnected,
        ServiceAccountRbacNotQualified,
    ]
}

#[derive(Deserialize)]
struct Namespace {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    metadata: NamespaceMetadata,
}
#[derive(Deserialize)]
struct NamespaceMetadata {
    name: String,
    uid: String,
    #[serde(default)]
    labels: std::collections::BTreeMap<String, String>,
    #[serde(default, rename = "deletionTimestamp")]
    deletion_timestamp: Option<String>,
}
