//! The two operator files of the gateway, read as its loaders read them.
//!
//! The loaders (`src/gateway_config.rs`: `load`, `validate_policies`,
//! `prepare_oidc_file`) are private to the gateway binary, and `load` reads
//! every token. This module restates only their closed envelopes and their
//! structural rules, each cited below. Every field-level rule is a call to the
//! library validator the loader itself calls. Parity tests beside the loaders
//! (`src/gateway_config_diagnostics_parity_tests.rs`) bind both halves: the
//! field names of the envelopes, and acceptance on a corpus of files.
//!
//! What is deliberately not restated: token content and token uniqueness
//! (`load`), the stores, and the durable authentication state (`load_oidc`).
//! A conclusion drawn here is therefore the offline half of the loader's, and
//! is never worded as acceptance by the loader.
//!
//! No token, key or store is opened. An inspection is not fail-fast: every
//! independent rule is evaluated, so one run names every defect of a file.

use super::catalogue::FindingCode;
use super::fs_facts::{self, Lookup};
use super::report::{Details, EntryKind, JsonErrorCategory, PolicyKind, Reason};
use crate::{
    api::OidcGatewayConfigV1,
    audit::{
        AuditCapacityPolicyV1, WorkloadAuditPolicyV1,
        authentication::{
            AuthenticationAudit, AuthenticationAuditPolicyV1, AuthenticationCapacityPlanV1,
        },
    },
    control::{
        QuotaPolicyV1,
        kubernetes_admission::KubernetesHeldCreatePolicyV1,
        kubernetes_session::{
            KubernetesSessionControlRecoveryPolicyV2, KubernetesSessionRecoveryPolicyV2,
            KubernetesSessionReservationPolicyV2,
        },
    },
    control_fs::{PrivateFile, absolute_path},
    oidc::{OidcError, OidcProvider, OidcTransportModeV1},
    redaction::Redactor,
    relay::RelayRetentionPolicyV1,
};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
};

/// Field names of the loader's `Registry`.
pub const REGISTRY_FIELDS: &[&str] = &["version", "tenants"];
/// Field names of the loader's `TenantFiles`.
pub const TENANT_FIELDS: &[&str] = &[
    "tenant_id",
    "data",
    "runner_token_file",
    "viewer_token_file",
    "worker_quota",
    "audit_capacity_policy",
    "workload_audit_policy",
    "relay_retention_policy",
    "held_create_policy",
    "session_reservation_policy",
    "session_recovery_policy",
    "session_control_recovery_policy",
];
/// Field names of the loader's `OidcFile`.
pub const OIDC_FILE_FIELDS: &[&str] = &[
    "version",
    "gateway",
    "authentication_audit",
    "ca_bundle_file",
];
/// Field names of the loader's `AuthenticationAuditFile`.
pub const AUTHENTICATION_AUDIT_FIELDS: &[&str] =
    &["path", "installation_id", "policy", "capacity_plan"];

/// `load`: the registry is read up to this size.
pub const MAX_REGISTRY_BYTES: u64 = 1_048_576;
/// `prepare_oidc_file`: bound of the OIDC file.
pub const MAX_OIDC_FILE_BYTES: u64 = 1_048_576;
/// `prepare_oidc_file`: bound of the file named by `ca_bundle_file`.
pub const MAX_CA_BUNDLE_FILE_BYTES: u64 = 262_144;
/// `validate_policies`: the accepted range of the gateway event bound.
pub const MAX_EVENTS_RANGE: std::ops::RangeInclusive<u64> = 1..=1_000_000;

// Mirror of `Registry` (`src/gateway_config.rs`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    version: u32,
    tenants: Vec<TenantFiles>,
}

// Mirror of `TenantFiles` (`src/gateway_config.rs`), with the same types.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TenantFiles {
    tenant_id: String,
    data: PathBuf,
    runner_token_file: PathBuf,
    viewer_token_file: PathBuf,
    #[serde(default)]
    worker_quota: QuotaPolicyV1,
    audit_capacity_policy: Option<AuditCapacityPolicyV1>,
    workload_audit_policy: Option<WorkloadAuditPolicyV1>,
    relay_retention_policy: Option<RelayRetentionPolicyV1>,
    held_create_policy: Option<KubernetesHeldCreatePolicyV1>,
    session_reservation_policy: Option<KubernetesSessionReservationPolicyV2>,
    session_recovery_policy: Option<KubernetesSessionRecoveryPolicyV2>,
    session_control_recovery_policy: Option<KubernetesSessionControlRecoveryPolicyV2>,
}

// Mirror of `OidcFile` (`src/gateway_config.rs`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OidcFile {
    version: u32,
    gateway: OidcGatewayConfigV1,
    authentication_audit: AuthenticationAuditFile,
    ca_bundle_file: Option<PathBuf>,
}

// Mirror of `AuthenticationAuditFile` (`src/gateway_config.rs`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthenticationAuditFile {
    path: PathBuf,
    installation_id: uuid::Uuid,
    policy: AuthenticationAuditPolicyV1,
    capacity_plan: Option<AuthenticationCapacityPlanV1>,
}

/// One rule that refuses a file. No refusal carries the text of an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: FindingCode,
    /// The tenant entry the refusal concerns, as the registry declares it.
    pub tenant_id: Option<String>,
    pub details: Option<Details>,
    /// The situation of the code that the texts of the report name, when
    /// `lstat` tells one apart.
    pub variant: Option<&'static str>,
}

impl Refusal {
    fn file(code: FindingCode) -> Self {
        Self {
            code,
            tenant_id: None,
            details: None,
            variant: None,
        }
    }

    fn detailed(code: FindingCode, details: Details) -> Self {
        Self {
            details: Some(details),
            ..Self::file(code)
        }
    }

    fn tenant(code: FindingCode, tenant_id: &str, details: Option<Details>) -> Self {
        Self {
            tenant_id: Some(tenant_id.to_owned()),
            details,
            ..Self::file(code)
        }
    }

    fn situation(mut self, variant: &'static str) -> Self {
        self.variant = Some(variant);
        self
    }
}

/// Category and position of a JSON decoding failure: never its text, which
/// can quote a value or an unknown key of the file.
fn json_error(error: &serde_json::Error) -> Details {
    use serde_json::error::Category;
    Details::JsonError {
        category: match error.classify() {
            Category::Io => JsonErrorCategory::Io,
            Category::Syntax => JsonErrorCategory::Syntax,
            Category::Data => JsonErrorCategory::Data,
            Category::Eof => JsonErrorCategory::Eof,
        },
        line: u64::try_from(error.line()).unwrap_or(u64::MAX),
        column: u64::try_from(error.column()).unwrap_or(u64::MAX),
    }
}

/// One tenant entry. A reference is `None` when the registry leaves it empty;
/// otherwise it is resolved as the loader resolves it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryTenant {
    pub tenant_id: String,
    pub data: Option<PathBuf>,
    pub runner_token_file: Option<PathBuf>,
    pub viewer_token_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegistryInspection {
    /// True once bytes of the file were read.
    pub read: bool,
    /// True when the file is a version 1 registry whose references could be
    /// resolved; `tenants` then holds every entry, in file order.
    pub decoded: bool,
    pub tenants: Vec<RegistryTenant>,
    /// True when a tenant declares a policy whose verdict depends on the
    /// gateway event bound.
    pub uses_max_events: bool,
    /// What `load` followed by `validate_policies` refuses, token content and
    /// token uniqueness aside.
    pub refusals: Vec<Refusal>,
}

/// The gateway's own read of the registry (`load`): a plain open that does not
/// follow a final link and cannot block on a FIFO, a regular file, at most
/// `MAX_REGISTRY_BYTES`. No owner, mode or link count is checked. `Err` says
/// whether bytes were read before the refusal.
fn read_registry(path: &Path) -> Result<Vec<u8>, bool> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| false)?;
    if !file.metadata().map_err(|_| false)?.is_file() {
        return Err(false);
    }
    let mut bytes = Vec::new();
    file.take(MAX_REGISTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| true)?;
    if bytes.len() as u64 > MAX_REGISTRY_BYTES {
        return Err(true);
    }
    Ok(bytes)
}

/// Why the gateway's read of a registry returned no byte, from `lstat` alone:
/// the reader gives one refusal for every cause. An absent file is named as
/// such. A regular file of another account that this account cannot open may
/// be read by a gateway that runs as its owner: that is not judged. Anything
/// else is what the reader refuses for certain.
fn registry_not_read(lookup: Lookup) -> Refusal {
    match lookup {
        Lookup::Absent => Refusal::file(FindingCode::RegistryFileUnreadable).situation("absent"),
        Lookup::Found(facts)
            if facts.kind == EntryKind::RegularFile && !facts.owner_is_inspector =>
        {
            Refusal::file(FindingCode::ReferenceInspectorIdentityDiffers).situation("registry")
        }
        Lookup::Found(_) => Refusal::file(FindingCode::RegistryFileUnreadable),
        Lookup::Unavailable => Refusal::file(FindingCode::ReferenceMetadataUnavailable),
    }
}

/// Inspect a tenant registry as `gateway --tenants-file` reads it, for the
/// given gateway event bound. Key files are never opened.
pub fn inspect_registry(path: &Path, max_events: u64) -> RegistryInspection {
    let mut inspection = RegistryInspection::default();
    let bytes = match read_registry(path) {
        Ok(bytes) => bytes,
        Err(read) => {
            inspection.read = read;
            inspection.refusals.push(if read {
                // Read, and above the bound or cut short.
                Refusal::file(FindingCode::RegistryFileUnreadable)
            } else {
                registry_not_read(fs_facts::lookup(path))
            });
            return inspection;
        }
    };
    inspection.read = true;
    let registry: Registry = match serde_json::from_slice(&bytes) {
        Ok(registry) => registry,
        Err(error) => {
            inspection.refusals.push(Refusal::detailed(
                FindingCode::RegistryJsonInvalid,
                json_error(&error),
            ));
            return inspection;
        }
    };
    // `load`: "Version de registre de tenants inconnue".
    if registry.version != 1 {
        inspection
            .refusals
            .push(Refusal::file(FindingCode::RegistryVersionUnknown));
        return inspection;
    }
    // `load`: a relative reference is resolved against the canonicalised
    // parent of the registry; an empty parent is the current directory.
    let Ok(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
    else {
        inspection
            .refusals
            .push(Refusal::file(FindingCode::RegistryFileUnreadable));
        return inspection;
    };
    inspection.decoded = true;
    // `load`: "Le registre de tenants est vide".
    if registry.tenants.is_empty() {
        inspection
            .refusals
            .push(Refusal::file(FindingCode::RegistryEmpty));
    }
    // `validate_policies`: the event bound is checked before any policy.
    let bound_in_range = MAX_EVENTS_RANGE.contains(&max_events);
    if !bound_in_range {
        inspection
            .refusals
            .push(Refusal::file(FindingCode::RegistryPolicyInvalid));
    }
    // `load`: a reference is not empty; an absolute one is used as written.
    let resolve = |value: &Path| {
        (!value.as_os_str().is_empty()).then(|| {
            if value.is_absolute() {
                value.to_owned()
            } else {
                parent.join(value)
            }
        })
    };
    let mut seen = BTreeSet::new();
    for tenant in registry.tenants {
        let id = tenant.tenant_id.as_str();
        let mut refuse = |code, details| {
            inspection.refusals.push(Refusal::tenant(code, id, details));
        };
        let mut refuse_policy = |policy| {
            refuse(
                FindingCode::RegistryPolicyInvalid,
                Some(Details::Policy { policy }),
            );
        };
        // `load` and `validate_policies` both call the quota validator.
        if tenant.worker_quota.validate().is_err() {
            refuse_policy(PolicyKind::WorkerQuota);
        }
        if tenant
            .relay_retention_policy
            .as_ref()
            .is_some_and(|policy| policy.validate().is_err())
        {
            refuse_policy(PolicyKind::RelayRetention);
        }
        // `load` canonicalises each Kubernetes policy; `validate_policies`
        // validates the canonical form.
        if tenant
            .held_create_policy
            .as_ref()
            .is_some_and(|policy| policy.canonicalized().and_then(|c| c.validate()).is_err())
        {
            refuse_policy(PolicyKind::KubernetesHeldCreate);
        }
        if tenant
            .session_reservation_policy
            .as_ref()
            .is_some_and(|policy| policy.canonicalized().and_then(|c| c.validate()).is_err())
        {
            refuse_policy(PolicyKind::KubernetesSessionReservation);
        }
        if tenant
            .session_recovery_policy
            .as_ref()
            .is_some_and(|policy| policy.canonicalized().and_then(|c| c.validate()).is_err())
        {
            refuse_policy(PolicyKind::KubernetesSessionRecovery);
        }
        if tenant
            .session_control_recovery_policy
            .as_ref()
            .is_some_and(|policy| policy.canonicalized().and_then(|c| c.validate()).is_err())
        {
            refuse_policy(PolicyKind::KubernetesSessionControlRecovery);
        }
        // `validate_policies`: the audit policies, for the gateway event
        // bound; the workload policy against the explicit capacity policy or
        // the resolved defaults.
        if tenant.audit_capacity_policy.is_some() || tenant.workload_audit_policy.is_some() {
            inspection.uses_max_events = true;
        }
        if bound_in_range {
            if tenant
                .audit_capacity_policy
                .as_ref()
                .is_some_and(|policy| policy.validate(max_events).is_err())
            {
                refuse_policy(PolicyKind::AuditCapacity);
            }
            if let Some(policy) = &tenant.workload_audit_policy {
                let defaults = AuditCapacityPolicyV1::resolved(max_events);
                let control = tenant.audit_capacity_policy.as_ref().unwrap_or(&defaults);
                if policy.validate(max_events, control).is_err() {
                    refuse_policy(PolicyKind::WorkloadAudit);
                }
            }
        }
        if crate::tenancy::validate_tenant_id(id).is_err() {
            refuse(FindingCode::RegistryTenantIdInvalid, None);
        }
        // `load`: "Identifiant de tenant dupliqué".
        if !seen.insert(tenant.tenant_id.clone()) {
            refuse(FindingCode::RegistryTenantIdDuplicate, None);
        }
        let data = resolve(&tenant.data);
        let runner_token_file = resolve(&tenant.runner_token_file);
        let viewer_token_file = resolve(&tenant.viewer_token_file);
        // `load`: "Référence de fichier vide".
        if data.is_none() || runner_token_file.is_none() || viewer_token_file.is_none() {
            refuse(FindingCode::RegistryReferenceEmpty, None);
        }
        inspection.tenants.push(RegistryTenant {
            tenant_id: tenant.tenant_id,
            data,
            runner_token_file,
            viewer_token_file,
        });
    }
    inspection
}

/// What became of the file named by `ca_bundle_file`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaBundle {
    #[default]
    NotDeclared,
    /// The reference is empty or leaves its directory: nothing to open.
    ReferenceInvalid,
    /// The caller withheld the file: it was not opened.
    Withheld,
    /// The private-file reader refused the file: it was not read.
    Unavailable,
    /// Read, and refused by the bundle grammar or by the offline client.
    Refused,
    /// Read and accepted by the bundle grammar. `with_client` is true when the
    /// offline client of the gateway block was also built with it.
    Accepted {
        certificates: u64,
        with_client: bool,
    },
}

#[derive(Clone, Default)]
pub struct OidcInspection {
    /// True once bytes of the file were read.
    pub read: bool,
    /// The decoded gateway block, valid or not.
    pub gateway: Option<OidcGatewayConfigV1>,
    pub version: Option<u32>,
    /// Tenant identifiers of the capacity plan, in file order.
    pub plan_tenants: Option<Vec<String>>,
    /// The resolved `authentication_audit.path`.
    pub audit_path: Option<PathBuf>,
    pub ca_bundle_declared: bool,
    /// The resolved `ca_bundle_file`.
    pub ca_bundle_file: Option<PathBuf>,
    pub ca_bundle: CaBundle,
    /// What `prepare_oidc_file` refuses.
    pub refusals: Vec<Refusal>,
    /// What the gateway refuses afterwards: the authentication audit policy,
    /// before it opens or creates the audit, and a plan that the policy
    /// cannot fund, once the audit is open. Kept apart so that `refusals`
    /// stays the exact counterpart of `prepare_oidc_file`.
    pub extra_refusals: Vec<Refusal>,
    /// The public bundle that the offline client accepted, kept so that the
    /// same client can be built again.
    ca_bundle_pem: Option<Vec<u8>>,
}

impl OidcInspection {
    /// The client the gateway builds from this file (`OidcGateway::new`, in
    /// `src/api/oidc.rs`): with the CA bundle when the file declares one.
    /// `None` unless the file passes every offline rule. Building the client
    /// contacts nothing.
    pub fn client(&self) -> Option<OidcProvider> {
        if !(self.refusals.is_empty() && self.extra_refusals.is_empty()) {
            return None;
        }
        let provider = self.gateway.as_ref()?.provider.clone();
        match (self.ca_bundle, &self.ca_bundle_pem) {
            (CaBundle::NotDeclared, _) => OidcProvider::new(provider).ok(),
            (
                CaBundle::Accepted {
                    with_client: true, ..
                },
                Some(pem),
            ) => OidcProvider::new_with_ca(provider, pem).ok(),
            _ => None,
        }
    }
}

/// Closed reason of a refusal of the gateway block. The validator's own
/// literals are compared whole; its text is never returned.
fn gateway_block_reason(error: &anyhow::Error) -> Reason {
    if error
        .downcast_ref::<<reqwest::Url as std::str::FromStr>::Err>()
        .is_some()
    {
        return Reason::UrlUnparseable;
    }
    if error.downcast_ref::<OidcError>().is_some() {
        return Reason::ProviderBlockInvalid;
    }
    // Literals of `OidcGatewayConfigV1::validate` (`src/api/oidc.rs`).
    match error.to_string().as_str() {
        "unsupported OIDC gateway configuration or token bound" => Reason::VersionOrTokenBound,
        "invalid OIDC web origin or callback" => Reason::WebOriginOrCallback,
        "invalid OIDC claim mapping" => Reason::ClaimMappingInvalid,
        "ambiguous OIDC claim mapping" => Reason::ClaimMappingAmbiguous,
        _ => Reason::Unclassified,
    }
}

/// Inspect an OIDC file as `gateway --oidc-config` prepares it. The identity
/// provider is never contacted and the authentication audit is never opened.
pub fn inspect_oidc_file(path: &Path) -> OidcInspection {
    inspect_oidc_file_guarded(path, &|_, _| true)
}

/// The same inspection for a caller that knows which files must never be
/// opened: `may_open_ca` receives the resolved CA reference before any open,
/// with the resolved authentication audit of the same file when it has one,
/// and a refusal leaves the file unopened.
pub fn inspect_oidc_file_guarded(
    path: &Path,
    may_open_ca: &dyn Fn(&Path, Option<&Path>) -> bool,
) -> OidcInspection {
    let mut inspection = OidcInspection::default();
    // `prepare_oidc_file`: the private-file reader, bounded.
    let opened = absolute_path(path).and_then(|path| {
        let held = PrivateFile::open(&path, MAX_OIDC_FILE_BYTES)?;
        Ok((path, held))
    });
    let Ok((path, held)) = opened else {
        // The reader gives one refusal for an absent and for an unsafe file;
        // `lstat` tells them apart. A file of another account may be read by
        // a gateway that runs as that account: that is not judged here.
        inspection
            .refusals
            .push(Refusal::file(match fs_facts::lookup(path) {
                Lookup::Absent => FindingCode::OidcFileAbsent,
                Lookup::Found(facts) if !facts.owner_is_inspector => {
                    FindingCode::ReferenceInspectorIdentityDiffers
                }
                _ => FindingCode::OidcFileUnavailableOrNotPrivate,
            }));
        return inspection;
    };
    inspection.read = true;
    let file: OidcFile = match serde_json::from_slice(held.bytes()) {
        Ok(file) => file,
        Err(error) => {
            inspection.refusals.push(Refusal::detailed(
                FindingCode::OidcJsonInvalid,
                json_error(&error),
            ));
            return inspection;
        }
    };
    inspection.version = Some(file.version);
    inspection.ca_bundle_declared = file.ca_bundle_file.is_some();
    let audit = &file.authentication_audit;
    inspection.plan_tenants = audit.capacity_plan.as_ref().map(|plan| {
        plan.tenants
            .iter()
            .map(|tenant| tenant.tenant_id.clone())
            .collect()
    });
    // `prepare_oidc_file`: version 1 without a plan, or version 2 with one.
    if !matches!(
        (file.version, audit.capacity_plan.as_ref()),
        (1, None) | (2, Some(_))
    ) {
        inspection
            .refusals
            .push(Refusal::file(FindingCode::OidcVersionPlanPairInvalid));
    }
    if audit
        .capacity_plan
        .as_ref()
        .is_some_and(|plan| plan.validate().is_err())
    {
        inspection
            .refusals
            .push(Refusal::file(FindingCode::OidcCapacityPlanInvalid));
    }
    let gateway_valid = match file.gateway.validate() {
        Ok(()) => true,
        Err(error) => {
            inspection.refusals.push(Refusal::detailed(
                FindingCode::OidcGatewayBlockInvalid,
                Details::Reason {
                    reason: gateway_block_reason(&error),
                },
            ));
            false
        }
    };
    // `prepare_oidc_file`: a reference is not empty; a relative one is
    // resolved lexically against the parent of the file; traversal is refused.
    let resolve = |reference: &Path| {
        let parent = path.parent()?;
        if reference.as_os_str().is_empty() {
            return None;
        }
        absolute_path(&if reference.is_absolute() {
            reference.to_owned()
        } else {
            parent.join(reference)
        })
        .ok()
    };
    let mut reference_invalid = false;
    let audit_path = resolve(&audit.path);
    if let Some(reference) = &file.ca_bundle_file {
        match resolve(reference) {
            Some(ca_path) => {
                (inspection.ca_bundle, inspection.ca_bundle_pem) = inspect_ca_bundle(
                    &ca_path,
                    &file,
                    gateway_valid,
                    &|ca| may_open_ca(ca, audit_path.as_deref()),
                    &mut inspection.refusals,
                );
                inspection.ca_bundle_file = Some(ca_path);
            }
            None => {
                inspection.ca_bundle = CaBundle::ReferenceInvalid;
                reference_invalid = true;
            }
        }
    }
    match audit_path {
        Some(audit_path) => inspection.audit_path = Some(audit_path),
        None => reference_invalid = true,
    }
    if reference_invalid {
        inspection
            .refusals
            .push(Refusal::file(FindingCode::OidcReferenceInvalid));
    }
    // The gateway validates the policy before it opens the audit
    // (`AuthenticationAudit::open_existing`), and a version 2 file must also
    // fund its plan (`AuthenticationAudit::validate_new_v4`, as setup does).
    let redactor = Redactor::new(Vec::new());
    let policy_valid = audit
        .policy
        .validate_for_installation(audit.installation_id, &redactor)
        .and_then(|()| match (&audit.capacity_plan, file.version) {
            (Some(plan), 2) => AuthenticationAudit::validate_new_v4(
                audit.installation_id,
                &audit.policy,
                plan,
                &redactor,
            ),
            _ => Ok(()),
        })
        .is_ok();
    if !policy_valid {
        inspection
            .extra_refusals
            .push(Refusal::file(FindingCode::OidcAuthenticationPolicyInvalid));
    }
    inspection.gateway = Some(file.gateway);
    inspection
}

/// `prepare_oidc_file`: the CA bundle is a private file of at most
/// `MAX_CA_BUNDLE_FILE_BYTES`, UTF-8, accepted by the offline client of the
/// provider block. The bundle grammar is asked first, so that a refusal by
/// the client can be told apart from a refusal of the bundle itself. The
/// bytes are returned with a bundle that the client accepted, and with no
/// other.
fn inspect_ca_bundle(
    ca_path: &Path,
    file: &OidcFile,
    gateway_valid: bool,
    may_open_ca: &dyn Fn(&Path) -> bool,
    refusals: &mut Vec<Refusal>,
) -> (CaBundle, Option<Vec<u8>>) {
    if !may_open_ca(ca_path) {
        refusals.push(Refusal::file(
            FindingCode::TrustCaReferenceIsACredentialReference,
        ));
        return (CaBundle::Withheld, None);
    }
    let Ok(ca) = PrivateFile::open(ca_path, MAX_CA_BUNDLE_FILE_BYTES) else {
        // The reader gives one refusal for an absent and for a refused file;
        // `lstat` tells them apart.
        let refusal = Refusal::file(FindingCode::TrustCaUnavailableOrNotPrivate);
        refusals.push(match fs_facts::lookup(ca_path) {
            Lookup::Absent => refusal.situation("absent"),
            _ => refusal,
        });
        return (CaBundle::Unavailable, None);
    };
    let certificates = std::str::from_utf8(ca.bytes())
        .ok()
        .and_then(|_| crate::tls::certificates(ca.bytes()).ok());
    let Some(certificates) = certificates else {
        refusals.push(Refusal::file(FindingCode::TrustCaBundleInvalid));
        return (CaBundle::Refused, None);
    };
    let certificates = u64::try_from(certificates.len()).unwrap_or(u64::MAX);
    if !gateway_valid {
        // The loader stops at the gateway block; the client is not built.
        return (
            CaBundle::Accepted {
                certificates,
                with_client: false,
            },
            None,
        );
    }
    if OidcProvider::new_with_ca(file.gateway.provider.clone(), ca.bytes()).is_ok() {
        return (
            CaBundle::Accepted {
                certificates,
                with_client: true,
            },
            Some(ca.bytes().to_vec()),
        );
    }
    // The client refuses explicit roots with any transport but HTTPS
    // (`src/oidc/transport.rs`, `Transport::new`).
    refusals.push(Refusal::file(
        if matches!(
            file.gateway.provider.transport,
            OidcTransportModeV1::HttpsOnly
        ) {
            FindingCode::TrustCaBundleInvalid
        } else {
            FindingCode::TrustCaIncompatibleWithTransport
        },
    ));
    (CaBundle::Refused, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::fs_facts::Facts;

    // A registry of another account cannot be built by one account: the
    // classification is exercised on the facts themselves.
    #[test]
    fn a_registry_that_was_not_read_is_classified_from_its_metadata() {
        let found = |kind, owner_is_inspector| {
            Lookup::Found(Facts {
                kind,
                mode: 0o600,
                owner_is_inspector,
                links: 1,
                identity: (0, 0),
            })
        };
        let classified = |lookup| {
            let refusal = registry_not_read(lookup);
            (refusal.code, refusal.variant)
        };
        assert_eq!(
            classified(Lookup::Absent),
            (FindingCode::RegistryFileUnreadable, Some("absent"))
        );
        assert_eq!(
            classified(found(EntryKind::RegularFile, false)),
            (
                FindingCode::ReferenceInspectorIdentityDiffers,
                Some("registry")
            ),
            "a file of another account that this account cannot open is not judged"
        );
        // The gateway reader refuses these for certain, whoever owns them.
        for lookup in [
            found(EntryKind::RegularFile, true),
            found(EntryKind::SymbolicLink, false),
            found(EntryKind::Directory, true),
            found(EntryKind::Other, false),
        ] {
            assert_eq!(
                classified(lookup),
                (FindingCode::RegistryFileUnreadable, None)
            );
        }
        assert_eq!(
            classified(Lookup::Unavailable),
            (FindingCode::ReferenceMetadataUnavailable, None)
        );
    }
}
