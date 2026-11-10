//! Operator-private authentication audit. This is neither a tenant Store nor
//! an execution authority. No token, code, cookie, verifier or provider body is
//! accepted for persistence. Fingerprint constructors hash borrowed credentials
//! immediately. Persistence is not atomic with the identity provider, and a
//! whole-database rollback across restart requires an independent witness.

use crate::{identity::PrincipalRefV1, redaction::Redactor};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

mod budget;
mod durable;
mod format;
pub(crate) use budget::AuthenticationTenantCandidatesV1;
pub use budget::{
    AuthenticationCapacityPlanV1, AuthenticationCapacityV1, AuthenticationTenantCapacityV1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OidcSessionKindV1 {
    WebPkce,
    NativeCli,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FingerprintDomainV1 {
    Access,
    Refresh,
}

/// A domain-separated digest, never an access or refresh credential. The
/// constructor hashes immediately and does not retain the borrowed token.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticationTokenFingerprintV1 {
    domain: FingerprintDomainV1,
    digest: [u8; 32],
}
impl AuthenticationTokenFingerprintV1 {
    pub fn access_token(token: &str) -> Self {
        Self::new(FingerprintDomainV1::Access, token)
    }

    pub fn refresh_token(token: &str) -> Self {
        Self::new(FingerprintDomainV1::Refresh, token)
    }

    fn new(domain: FingerprintDomainV1, token: &str) -> Self {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(match domain {
            FingerprintDomainV1::Access => b"ai-guard:oidc-access-fingerprint:v1\0".as_slice(),
            FingerprintDomainV1::Refresh => b"ai-guard:oidc-refresh-fingerprint:v1\0".as_slice(),
        });
        hash.update(token.as_bytes());
        Self {
            domain,
            digest: hash.finalize().into(),
        }
    }
}
impl std::fmt::Debug for AuthenticationTokenFingerprintV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthenticationTokenFingerprintV1([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticationSessionBindingV1 {
    pub session_id: Uuid,
    pub generation: u32,
    pub principal: PrincipalRefV1,
    pub issuer: String,
    pub client_id: String,
    pub kind: OidcSessionKindV1,
    pub access_fingerprint: AuthenticationTokenFingerprintV1,
    pub refresh_fingerprint: Option<AuthenticationTokenFingerprintV1>,
    pub expires_at_unix_seconds: i64,
}
impl std::fmt::Debug for AuthenticationSessionBindingV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthenticationSessionBindingV1([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthenticationSessionStateV1 {
    Active,
    RefreshPending {
        operation_id: Uuid,
        intention: AuthenticationReceiptV1,
    },
    Closed {
        reason: AuthenticationReasonV1,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticationSessionCurrentV1 {
    pub binding: AuthenticationSessionBindingV1,
    pub state: AuthenticationSessionStateV1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationMutationV1 {
    pub receipt: AuthenticationReceiptV1,
    /// Only an inserted pending transition permits one provider exchange.
    pub inserted: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedAuthenticationClientV1 {
    pub issuer: String,
    pub client_id: String,
}
impl std::fmt::Debug for ApprovedAuthenticationClientV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApprovedAuthenticationClientV1([REDACTED])")
    }
}

/// Limits on persisted JSON bytes and record count, including genesis. These
/// are not filesystem quotas: SQLite pages, WAL/SHM and backups add overhead.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticationAuditPolicyV1 {
    pub max_records: u64,
    pub max_serialized_bytes: u64,
    pub approved_clients: Vec<ApprovedAuthenticationClientV1>,
}
impl std::fmt::Debug for AuthenticationAuditPolicyV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthenticationAuditPolicyV1")
            .field("max_records", &self.max_records)
            .field("max_serialized_bytes", &self.max_serialized_bytes)
            .field("approved_clients", &"[REDACTED]")
            .finish()
    }
}

impl AuthenticationAuditPolicyV1 {
    /// Offline operator preflight, identical to initialization's policy checks.
    pub(crate) fn validate_for_installation(
        &self,
        installation_id: Uuid,
        redactor: &Redactor,
    ) -> anyhow::Result<()> {
        format::validate_policy(installation_id, self, redactor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationActionV1 {
    Login,
    /// The gateway observed access-token validation, not the CLI PKCE flow.
    SessionAdmission,
    Refresh,
    Logout,
    Expiration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationOutcomeV1 {
    Succeeded,
    Rejected,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationReasonV1 {
    Verified,
    NoMembership,
    InvalidResponse,
    ProviderRejected,
    ProviderUnavailable,
    Expired,
    Revoked,
    UserRequested,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthenticationPhaseV1 {
    Intention,
    Result {
        outcome: AuthenticationOutcomeV1,
        reason: AuthenticationReasonV1,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticationReceiptV1 {
    pub namespace_id: Uuid,
    pub sequence: u64,
    pub hash: String,
}
impl std::fmt::Debug for AuthenticationReceiptV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthenticationReceiptV1")
            .field("namespace_id", &self.namespace_id)
            .field("sequence", &self.sequence)
            .field("hash", &"[REDACTED]")
            .finish()
    }
}

/// The trusted verifier supplies `principal` only after verification. Login
/// intentions and unverified/rejected provider responses cannot carry one.
/// A result's causal receipt must identify its exact prior intention.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticationRecordV1 {
    pub operation_id: Uuid,
    pub action: AuthenticationActionV1,
    pub phase: AuthenticationPhaseV1,
    pub issuer: String,
    pub client_id: String,
    pub principal: Option<PrincipalRefV1>,
    pub transaction_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub causal: Option<AuthenticationReceiptV1>,
}
impl std::fmt::Debug for AuthenticationRecordV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthenticationRecordV1")
            .field("operation_id", &self.operation_id)
            .field("action", &self.action)
            .field("phase", &self.phase)
            .field("identity_and_context", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationVerificationV1 {
    pub namespace_id: Uuid,
    pub records: u64,
    pub serialized_bytes: u64,
    pub head: AuthenticationReceiptV1,
}

/// Equality uses only the retained device/inode pair, never the path spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationStorageIdentityV1(crate::control_fs::PrivateEntryIdentity);
impl AuthenticationStorageIdentityV1 {
    pub(crate) fn from_checked_entry(identity: crate::control_fs::PrivateEntryIdentity) -> Self {
        Self(identity)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationAuditFailureV1 {
    InvalidInput,
    Format,
    Integrity,
    Conflict,
    PhaseOrder,
    Capacity,
    SubjectCapacity,
}
impl std::fmt::Display for AuthenticationAuditFailureV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "invalid authentication audit input",
            Self::Format => "unsupported authentication audit format or binding",
            Self::Integrity => "authentication audit integrity failure; recovery required",
            Self::Conflict => "authentication operation phase conflicts with durable content",
            Self::PhaseOrder => "authentication phase lacks its exact durable intention",
            Self::Capacity => "authentication audit capacity exhausted",
            Self::SubjectCapacity => {
                "verified subject authentication budget exhausted; operator recovery required"
            }
        })
    }
}
impl std::error::Error for AuthenticationAuditFailureV1 {}

pub struct AuthenticationAudit {
    inner: durable::DurableAuthenticationAudit,
}

impl AuthenticationAudit {
    /// Pure setup preflight using the same canonical payloads and budget checks
    /// as actual V4 initialization. This grants no identity or authority.
    pub(crate) fn validate_new_v4(
        installation_id: Uuid,
        policy: &AuthenticationAuditPolicyV1,
        plan: &AuthenticationCapacityPlanV1,
        redactor: &Redactor,
    ) -> anyhow::Result<()> {
        format::validate_new_v4(installation_id, policy, plan, redactor)
    }

    pub(crate) fn bootstrap_receipt_v4(&self) -> anyhow::Result<AuthenticationReceiptV1> {
        self.inner.bootstrap_receipt_v4()
    }

    /// Only the retained offline preparation may initialize its reserved inode.
    /// The callback receives the SQLite-verified storage witness, never a fresh
    /// independent pathname observation. Ordinary opens never use this port.
    pub(crate) fn initialize_prepared_v4(
        path: &Path,
        installation_id: Uuid,
        namespace_id: Uuid,
        policy: AuthenticationAuditPolicyV1,
        plan: AuthenticationCapacityPlanV1,
        redactor: Redactor,
        verify_reserved: impl FnOnce(AuthenticationStorageIdentityV1) -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            inner: durable::DurableAuthenticationAudit::initialize_prepared_v4(
                path,
                installation_id,
                namespace_id,
                policy,
                plan,
                redactor,
                verify_reserved,
            )?,
        })
    }

    /// The trusted gateway supplies only memberships it actually verified.
    /// An empty snapshot can fund a rejection, never a session admission.
    pub(crate) fn begin_verified_initial_scoped(
        &self,
        record: AuthenticationRecordV1,
        principal: PrincipalRefV1,
        access_fingerprint: AuthenticationTokenFingerprintV1,
        candidates: AuthenticationTenantCandidatesV1,
    ) -> anyhow::Result<AuthenticationMutationV1> {
        self.inner
            .begin_verified_initial_scoped(record, principal, access_fingerprint, candidates)
    }

    pub fn capacity_plan(&self) -> anyhow::Result<Option<AuthenticationCapacityPlanV1>> {
        self.inner.capacity_plan()
    }

    /// Common gateway assembly preflight, including library callers. This
    /// checks accounting scope and format, not memberships or permissions.
    pub(crate) fn validate_capacity_tenants(
        &self,
        tenants: &std::collections::BTreeSet<String>,
    ) -> anyhow::Result<()> {
        self.inner.validate_capacity_tenants(tenants)
    }

    /// Explicit offline V3 -> V4 migration with an exclusively new V3 backup.
    /// Commits the finite plan and format atomically. Existing receipts, G and
    /// outstanding historical terminal promises are never rewritten.
    pub fn migrate_tenant_capacity_v4(
        &self,
        backup: &Path,
        plan: AuthenticationCapacityPlanV1,
    ) -> anyhow::Result<AuthenticationVerificationV1> {
        self.inner.migrate_tenant_capacity_v4(backup, plan)
    }

    /// V2: atomically consume a verified initial access proof and reserve its
    /// subject budget with the mandatory intention. Neither a proof replay nor
    /// a gateway restart grants a second attempt. This is not membership or
    /// execution authority; current permissions must still be checked.
    pub fn begin_verified_initial(
        &self,
        record: AuthenticationRecordV1,
        principal: PrincipalRefV1,
        access_fingerprint: AuthenticationTokenFingerprintV1,
    ) -> anyhow::Result<AuthenticationMutationV1> {
        self.inner
            .begin_verified_initial(record, principal, access_fingerprint, false)
    }

    /// Only the trusted gateway may recover an exhausted subject after a fresh
    /// Rust membership preflight. This records that observation, not authority:
    /// the regular audited membership lookup and session CAS remain mandatory.
    pub(crate) fn begin_verified_initial_after_membership(
        &self,
        record: AuthenticationRecordV1,
        principal: PrincipalRefV1,
        access_fingerprint: AuthenticationTokenFingerprintV1,
    ) -> anyhow::Result<AuthenticationMutationV1> {
        self.inner
            .begin_verified_initial(record, principal, access_fingerprint, true)
    }

    /// Explicit offline V1 -> V2 format migration. Creates and verifies an
    /// exclusively new V1 backup while holding the source write transaction,
    /// then changes only user_version. Existing payloads, hashes, namespace,
    /// quota and policy are preserved. Reopening never performs this migration.
    /// Stop gateway/brokers first; the SQLite lock is not runtime fencing.
    pub fn migrate_verified_initial_v2(
        &self,
        backup: &Path,
    ) -> anyhow::Result<AuthenticationVerificationV1> {
        self.inner.migrate_verified_initial_v2(backup)
    }

    /// Explicit offline V2 -> V3 migration with an exclusively new verified
    /// backup. No payload, receipt, quota or historical reservation is rewritten.
    /// Stop all writers first; this method does not provide runtime fencing.
    pub fn migrate_native_logout_v3(
        &self,
        backup: &Path,
    ) -> anyhow::Result<AuthenticationVerificationV1> {
        self.inner.migrate_native_logout_v3(backup)
    }

    /// Consume the exact prior intention and record generation one atomically.
    /// WebPkce uses Login; NativeCli uses SessionAdmission. This observation
    /// does not grant membership, an API credential or execution authority.
    pub fn admit_session(
        &self,
        record: AuthenticationRecordV1,
        binding: AuthenticationSessionBindingV1,
    ) -> anyhow::Result<AuthenticationReceiptV1> {
        Ok(self
            .inner
            .mutate(format::SessionMutation::Admit { record, binding })?
            .receipt)
    }

    /// Commit the intention, Active -> Pending CAS and an allowance for one
    /// terminal record/up to 32_768 payload bytes together. Every new write
    /// preserves outstanding allowances reconstructed from durable Pending.
    /// Only `inserted=true` permits the caller to begin one provider exchange.
    pub fn refresh_pending(
        &self,
        record: AuthenticationRecordV1,
        expected_generation: u32,
        expected_access_fingerprint: AuthenticationTokenFingerprintV1,
    ) -> anyhow::Result<AuthenticationMutationV1> {
        self.inner.mutate(format::SessionMutation::RefreshPending {
            record,
            expected_generation,
            expected_access_fingerprint,
        })
    }

    /// Consume the exact pending intention, retain the identity/client/kind
    /// binding and install the checked next generation and unseen access hash.
    pub fn rotate_session(
        &self,
        record: AuthenticationRecordV1,
        binding: AuthenticationSessionBindingV1,
        expected_generation: u32,
        expected_access_fingerprint: AuthenticationTokenFingerprintV1,
    ) -> anyhow::Result<AuthenticationReceiptV1> {
        Ok(self
            .inner
            .mutate(format::SessionMutation::Rotate {
                record,
                binding,
                expected_generation,
                expected_access_fingerprint,
            })?
            .receipt)
    }

    /// Close terminally using its exact Logout/Expiration intention, or a
    /// non-success Refresh result consuming the current pending intention.
    pub fn close_session(
        &self,
        record: AuthenticationRecordV1,
        expected_generation: u32,
        expected_access_fingerprint: AuthenticationTokenFingerprintV1,
    ) -> anyhow::Result<AuthenticationReceiptV1> {
        Ok(self
            .inner
            .mutate(format::SessionMutation::Close {
                record,
                expected_generation,
                expected_access_fingerprint,
            })?
            .receipt)
    }

    /// Persist the original native logout intention and its own terminal
    /// allowance while retaining the exact pending refresh and its allowance.
    /// A replay observes the same receipt; it never grants a provider exchange.
    pub(crate) fn begin_native_logout_during_refresh(
        &self,
        record: AuthenticationRecordV1,
        expected_binding: AuthenticationSessionBindingV1,
        refresh_operation_id: Uuid,
        refresh_intention: AuthenticationReceiptV1,
    ) -> anyhow::Result<AuthenticationReceiptV1> {
        Ok(self
            .inner
            .mutate(format::SessionMutation::BeginNativeLogoutDuringRefresh {
                record,
                expected_binding,
                refresh_operation_id,
                refresh_intention,
            })?
            .receipt)
    }

    /// Consume only the typed logout promise after the original refresh Close
    /// is durably proved. This records logout without changing a closed binding.
    pub(crate) fn record_native_logout_after_refresh_closed(
        &self,
        record: AuthenticationRecordV1,
        expected_binding: AuthenticationSessionBindingV1,
        refresh_operation_id: Uuid,
        refresh_intention: AuthenticationReceiptV1,
        refresh_result: AuthenticationReceiptV1,
    ) -> anyhow::Result<AuthenticationReceiptV1> {
        Ok(self
            .inner
            .mutate(
                format::SessionMutation::RecordNativeLogoutAfterRefreshClosed {
                    record,
                    expected_binding,
                    refresh_operation_id,
                    refresh_intention,
                    refresh_result,
                },
            )?
            .receipt)
    }

    /// Revalidate the durable chain before returning its phase. Active is not
    /// proof of unexpired tokens or of a live private in-memory token owner.
    /// Restarting that owner never restores credentials from this ledger.
    pub fn session_current(
        &self,
        session_id: Uuid,
    ) -> anyhow::Result<Option<AuthenticationSessionCurrentV1>> {
        self.inner.session_current(session_id)
    }

    /// Both access and optional refresh domains are remembered durably.
    pub fn fingerprint_seen(
        &self,
        fingerprint: &AuthenticationTokenFingerprintV1,
    ) -> anyhow::Result<bool> {
        self.inner.fingerprint_seen(fingerprint)
    }
    pub fn init_new(
        path: &Path,
        installation_id: Uuid,
        policy: AuthenticationAuditPolicyV1,
        redactor: Redactor,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            inner: durable::DurableAuthenticationAudit::init_new(
                path,
                installation_id,
                policy,
                redactor,
            )?,
        })
    }

    /// Exclusively new V4 ledger. Genesis and its finite capacity plan commit
    /// atomically; existing files are never adopted or migrated here.
    pub fn init_new_v4(
        path: &Path,
        installation_id: Uuid,
        policy: AuthenticationAuditPolicyV1,
        plan: AuthenticationCapacityPlanV1,
        redactor: Redactor,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            inner: durable::DurableAuthenticationAudit::init_new_v4(
                path,
                installation_id,
                policy,
                plan,
                redactor,
            )?,
        })
    }

    pub fn open_existing(
        path: &Path,
        installation_id: Uuid,
        policy: AuthenticationAuditPolicyV1,
        redactor: Redactor,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            inner: durable::DurableAuthenticationAudit::open_existing(
                path,
                installation_id,
                policy,
                redactor,
            )?,
        })
    }

    pub fn append_typed(
        &self,
        record: AuthenticationRecordV1,
    ) -> anyhow::Result<AuthenticationReceiptV1> {
        self.inner.append_typed(record)
    }

    pub fn verify(&self) -> anyhow::Result<AuthenticationVerificationV1> {
        self.inner.verify()
    }

    /// Validates the complete chain and returns the closed format generation.
    /// V1 can be inspected/backed up/migrated, but cannot reserve a V2 proof.
    /// Native logout terminal promises require V3.
    pub fn format_version(&self) -> anyhow::Result<u32> {
        self.inner.format_version()
    }

    pub fn backup_new(&self, path: &Path) -> anyhow::Result<AuthenticationVerificationV1> {
        self.inner.backup_new(path)
    }

    pub fn storage_identity(&self) -> anyhow::Result<AuthenticationStorageIdentityV1> {
        self.inner.storage_identity()
    }

    /// Compare retained device/inode identities, without opening an extra raw
    /// database FD (which could release SQLite POSIX locks when closed).
    pub fn ensure_distinct_from_business(&self, store: &super::Store) -> anyhow::Result<()> {
        self.inner.ensure_distinct_from_business(store)
    }
}

#[cfg(all(test, unix))]
#[path = "authentication/native_logout_tests.rs"]
mod native_logout_tests;
