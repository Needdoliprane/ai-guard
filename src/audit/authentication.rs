//! Operator-private authentication audit. This is neither a tenant Store nor
//! an execution authority. No token, code, cookie, verifier or provider body is
//! accepted by this API. Persistence is not atomic with the identity provider.

use crate::{identity::PrincipalRefV1, redaction::Redactor};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

mod durable;
mod format;

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
        Self { domain, digest: hash.finalize().into() }
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
    RefreshPending { operation_id: Uuid, intention: AuthenticationReceiptV1 },
    Closed { reason: AuthenticationReasonV1 },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationAuditFailureV1 {
    InvalidInput,
    Format,
    Integrity,
    Conflict,
    PhaseOrder,
    Capacity,
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
        })
    }
}
impl std::error::Error for AuthenticationAuditFailureV1 {}

pub struct AuthenticationAudit {
    inner: durable::DurableAuthenticationAudit,
}

impl AuthenticationAudit {
    pub fn admit_session(&self, _record: AuthenticationRecordV1, _binding: AuthenticationSessionBindingV1) -> anyhow::Result<AuthenticationReceiptV1> {
        Err(AuthenticationAuditFailureV1::PhaseOrder.into())
    }

    pub fn refresh_pending(&self, _record: AuthenticationRecordV1, _expected_generation: u32, _expected_access_fingerprint: AuthenticationTokenFingerprintV1) -> anyhow::Result<AuthenticationMutationV1> {
        Err(AuthenticationAuditFailureV1::PhaseOrder.into())
    }

    pub fn rotate_session(&self, _record: AuthenticationRecordV1, _binding: AuthenticationSessionBindingV1, _expected_generation: u32, _expected_access_fingerprint: AuthenticationTokenFingerprintV1) -> anyhow::Result<AuthenticationReceiptV1> {
        Err(AuthenticationAuditFailureV1::PhaseOrder.into())
    }

    pub fn close_session(&self, _record: AuthenticationRecordV1, _expected_generation: u32, _expected_access_fingerprint: AuthenticationTokenFingerprintV1) -> anyhow::Result<AuthenticationReceiptV1> {
        Err(AuthenticationAuditFailureV1::PhaseOrder.into())
    }

    pub fn session_current(&self, _session_id: Uuid) -> anyhow::Result<Option<AuthenticationSessionCurrentV1>> {
        Err(AuthenticationAuditFailureV1::Integrity.into())
    }

    pub fn fingerprint_seen(&self, _fingerprint: &AuthenticationTokenFingerprintV1) -> anyhow::Result<bool> {
        Err(AuthenticationAuditFailureV1::Integrity.into())
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
