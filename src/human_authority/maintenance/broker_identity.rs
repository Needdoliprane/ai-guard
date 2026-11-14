//! Offline owned broker identity maintenance for the proposed
//! `broker_supervision_owned_v1` profile (decision D1). Proposal submitted to
//! Root: nothing here is admitted, activated or qualified.
//!
//! The caller opens the real owned union exactly as the gateway does and
//! hands over its startup bundle. Acquiring the physical owner locks proves
//! that no gateway retains these sources (a running one makes the opening
//! fail with `OwnerContended`); the operator's stop attestation still covers
//! every other consumer. Opening the union records a control incarnation in
//! every source, like any gateway start.
//!
//! Before the first mutation, the operator's backup of the selected tenant
//! Store is verified: private regular file outside every source directory, no
//! SQLite sidecar, the operator's SHA-256, exact format, tenant, namespace,
//! schema and chain integrity, and a head that is exactly the source state
//! before this session's incarnation record. A durable private plan then fixes
//! every operation ID and, for an enrollment or a rotation, an exclusively
//! created 0600 bearer file, before any write. Each step runs through an
//! `IdentityAuthority` writer under a fresh write epoch, and its committed
//! receipt is recorded exclusively. A resume replays the same operations: a
//! committed one returns its original receipt, divergent content is a
//! conflict, and a recorded receipt missing from the durable chain (for
//! example after an older control backup was restored) stops before any
//! write. The public enrollment handoff is built only from committed receipts,
//! cross-checked by authenticating the new bearer through the same scoped
//! Store path as the gateway. The handoff authenticates nothing by itself.
//!
//! The bearer never appears in an argument, a plan, a receipt, the handoff or
//! an output; only its digest reaches the Store. A committed binding change
//! advances the owned discovery generation: the candidate index is rebuilt at
//! the next gateway start. Nothing here opens a route or admits a worker, a
//! relay, a Kubernetes runtime or an egress path, and nothing releases a quota,
//! fences a process, sends a command or meters usage.
use super::super::{
    HumanAuthorityFailure, HumanSourceKindV1, HumanSourcesWriteEpoch, OwnedSourceToken,
    startup::HumanOwnedStartupBundle,
};
use crate::{
    audit::{HumanSourceAccess, OwnedBrokerBindingStepV1, OwnedRetainedSourceFacts, Store},
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerMutationResultV1, BrokerRevocationV1,
        BrokerRuntimeKindV1, BrokerStateV1, format_time,
        owned_supervision::{
            OWNED_SUPERVISION_PROFILE_V1, OWNED_SUPERVISION_VERSION_V1, OwnedBrokerEnrollmentV1,
            OwnedBrokerSubjectV1, OwnedEnrollmentReceiptV1, OwnedSupervisionErrorCodeV1 as Code,
            OwnedSupervisionPinsV1, canonical_gateway_origin,
        },
        parse_time, registry_uuid,
    },
    control_fs::{Directory, PrivateFile, absolute_path},
    workload::{
        WorkloadBindingIssueV1, WorkloadBindingReceiptV1, WorkloadBindingRevocationV1,
        WorkloadSubjectV1,
    },
};
use chrono::{DateTime, Duration, Utc};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fmt,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const PLAN_VERSION: u32 = 1;
const PLAN_FILE: &str = "plan.json";
const BEARER_FILE: &str = "bearer";
const ENROLLMENT_FILE: &str = "enrollment.json";
const MAX_PLAN_BYTES: u64 = 64 * 1024;
const MAX_RECORD_BYTES: u64 = 16 * 1024;
const MAX_ENROLLMENT_FILE_BYTES: u64 = 8 * 1024;
const MAX_BEARER_FILE_BYTES: u64 = 128;
const MAX_PLAN_ENTRIES: u32 = 80;
const MAX_PLAN_LEAF_BYTES: usize = 96;
/// The status projection lists at most 64 bindings per broker subject.
const MAX_REVOKED_BINDINGS: usize = 64;
const MAX_AUTHORITY_ID_BYTES: usize = 128;
const MAX_HISTORY_PAGE: u32 = 100;
/// Upper bound of a grant created by this maintenance, in days.
const MAX_GRANT_LIFETIME_DAYS: i64 = 366;
const MAX_LEASE_SECONDS: u32 = 300;
const BEARER_PREFIX: &str = "aigw1_";
const STEP_DOMAIN: &str = "ai-guard:broker-identity-maintenance:v1:step";

/// Closed, secret-free refusal reason. It never carries a path, a secret, a
/// digest or the text of an underlying error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrokerIdentityFailureV1 {
    InvalidInput,
    UnionMismatch,
    MigrationRequired,
    BackupRefused,
    PlanConflict,
    PlanIncomplete,
    PlanBusy,
    StateRefused,
    CredentialCollision,
    DurableConflict,
    ClockUncertain,
    AuditUnavailable,
    Unavailable,
    HandoffRefused,
    OutputRefused,
}

impl BrokerIdentityFailureV1 {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "broker_identity_invalid_input",
            Self::UnionMismatch => "broker_identity_union_mismatch",
            Self::MigrationRequired => "broker_identity_migration_required",
            Self::BackupRefused => "broker_identity_backup_refused",
            Self::PlanConflict => "broker_identity_plan_conflict",
            Self::PlanIncomplete => "broker_identity_plan_incomplete",
            Self::PlanBusy => "broker_identity_plan_busy",
            Self::StateRefused => "broker_identity_state_refused",
            Self::CredentialCollision => "broker_identity_credential_collision",
            Self::DurableConflict => "broker_identity_durable_conflict",
            Self::ClockUncertain => "broker_identity_clock_uncertain",
            Self::AuditUnavailable => "broker_identity_audit_unavailable",
            Self::Unavailable => "broker_identity_unavailable",
            Self::HandoffRefused => "broker_identity_handoff_refused",
            Self::OutputRefused => "broker_identity_output_refused",
        }
    }

    /// Fixed operator action; no request value, path or secret.
    pub const fn action(self) -> &'static str {
        match self {
            Self::InvalidInput => "correct_the_arguments_or_inputs",
            Self::UnionMismatch => {
                "check_the_owned_union_configuration_and_closure; preserve_the_plan"
            }
            Self::MigrationRequired => "migrate_the_tenant_store_to_format14_or_15_offline_first",
            Self::BackupRefused => {
                "keep_the_control_stopped; take_a_new_private_backup_of_this_tenant_store; retry_with_its_sha256"
            }
            Self::PlanConflict => {
                "preserve_the_plan_directory; reconcile_its_files_with_status; never_edit_or_reuse_it"
            }
            Self::PlanIncomplete => {
                "preserve_the_directory; no_operation_of_it_was_written; start_a_new_plan_directory"
            }
            Self::PlanBusy => "wait_for_the_other_maintenance_process_to_exit",
            Self::StateRefused => "inspect_status; choose_the_action_matching_the_durable_state",
            Self::CredentialCollision => "start_a_new_plan_directory; report_the_collision",
            Self::DurableConflict => {
                "inspect_status; resume_only_the_same_plan_or_revoke_then_plan_again"
            }
            Self::ClockUncertain => "reconcile_the_control_clock_with_the_durable_floor; resume",
            Self::AuditUnavailable => {
                "restore_audit_capacity_or_integrity; resume_the_same_plan"
            }
            Self::Unavailable => "retry_resume_with_the_same_plan",
            Self::HandoffRefused => {
                "do_not_install_any_handoff; preserve_the_plan; inspect_status; resume"
            }
            Self::OutputRefused => "the_durable_outcome_is_unchanged; run_status_or_resume",
        }
    }
}

type Checked<T> = Result<T, BrokerIdentityFailureV1>;

fn check(ok: bool, failure: BrokerIdentityFailureV1) -> Checked<()> {
    if ok { Ok(()) } else { Err(failure) }
}

/// Closed refusal with the progress already confirmed by this run. A partial
/// maintenance (`completed_steps > 0`) keeps its plan for an exact resume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrokerIdentityRefusalV1 {
    failure: BrokerIdentityFailureV1,
    completed_steps: usize,
    total_steps: usize,
}

impl BrokerIdentityRefusalV1 {
    fn at(failure: BrokerIdentityFailureV1, completed_steps: usize, total_steps: usize) -> Self {
        Self {
            failure,
            completed_steps,
            total_steps,
        }
    }

    pub fn failure(&self) -> BrokerIdentityFailureV1 {
        self.failure
    }

    pub fn completed_steps(&self) -> usize {
        self.completed_steps
    }

    pub fn total_steps(&self) -> usize {
        self.total_steps
    }

    /// At least one durable step of the plan is confirmed by this run.
    pub fn partial(&self) -> bool {
        self.completed_steps > 0
    }
}

impl From<BrokerIdentityFailureV1> for BrokerIdentityRefusalV1 {
    fn from(failure: BrokerIdentityFailureV1) -> Self {
        Self::at(failure, 0, 0)
    }
}

impl fmt::Display for BrokerIdentityRefusalV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "broker_identity_refused; reason={}; state={}; completed_steps={}; total_steps={}; action={}",
            self.failure.code(),
            if self.partial() {
                "partial_maintenance"
            } else {
                "no_step_confirmed_by_this_run"
            },
            self.completed_steps,
            self.total_steps,
            self.failure.action()
        )
    }
}

impl std::error::Error for BrokerIdentityRefusalV1 {}

/// One closed JSON document for standard output, already screened.
pub struct BrokerIdentityOutputV1 {
    bytes: Vec<u8>,
}

impl BrokerIdentityOutputV1 {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Operator backup proof: an absolute path and the SHA-256 the operator
/// recorded when the backup was taken with the control stopped.
pub struct BrokerIdentityBackupV1 {
    pub path: PathBuf,
    pub sha256: String,
}

/// Broker targeted by an enrollment.
pub enum BrokerSelectionV1 {
    /// Existing broker, or an operator-chosen ID for a first registration.
    Existing(Uuid),
    /// A new random broker ID.
    New,
}

pub struct BrokerEnrollmentRequestV1 {
    pub tenant_id: String,
    pub broker: BrokerSelectionV1,
    pub authority_id: String,
    pub runtime_kind: BrokerRuntimeKindV1,
    pub lease_seconds: u32,
    pub grant_not_after: String,
    pub credential_expires_at: Option<String>,
    pub gateway_origin: String,
    pub replace_active_grant: bool,
    pub backup: BrokerIdentityBackupV1,
    pub plan_dir: PathBuf,
}

pub struct BrokerRotationRequestV1 {
    pub tenant_id: String,
    pub broker_id: Uuid,
    pub credential_expires_at: Option<String>,
    pub gateway_origin: String,
    pub backup: BrokerIdentityBackupV1,
    pub plan_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrokerRevocationScopeV1 {
    /// Revoke the grant, then every unrevoked binding of its incarnation.
    All,
    Grant,
    Binding,
}

pub struct BrokerRevocationRequestV1 {
    pub tenant_id: String,
    pub broker_id: Uuid,
    pub scope: BrokerRevocationScopeV1,
    pub backup: BrokerIdentityBackupV1,
    pub plan_dir: PathBuf,
}

pub struct BrokerIdentityStatusRequestV1 {
    pub tenant_id: String,
    pub broker_id: Uuid,
    pub credential_id: Option<Uuid>,
    pub history_limit: u32,
    pub before_revision: Option<u64>,
}

/// Generated broker bearer. Deliberately no Debug, Display, Clone or serde.
struct Bearer {
    secret: String,
    digest: String,
    digest_bytes: [u8; 32],
}

impl Bearer {
    /// Same generator and format as the existing workload credential CLI:
    /// 32 bytes from the OS generator, `aigw1_` and lowercase hex.
    fn generate() -> Checked<Self> {
        let mut entropy = [0u8; 32];
        SystemRandom::new()
            .fill(&mut entropy)
            .map_err(|_| BrokerIdentityFailureV1::Unavailable)?;
        Ok(Self::from_secret(format!(
            "{BEARER_PREFIX}{}",
            hex::encode(entropy)
        )))
    }

    fn from_secret(secret: String) -> Self {
        let digest_bytes: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
        Self {
            digest: hex::encode(digest_bytes),
            digest_bytes,
            secret,
        }
    }

    /// Exact file format: the bearer and one newline, nothing else.
    fn parse(bytes: &[u8]) -> Checked<Self> {
        let conflict = BrokerIdentityFailureV1::PlanConflict;
        let text = std::str::from_utf8(bytes).map_err(|_| conflict)?;
        let secret = text.strip_suffix('\n').ok_or(conflict)?;
        let entropy = secret.strip_prefix(BEARER_PREFIX).ok_or(conflict)?;
        check(
            entropy.len() == 64
                && entropy
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            conflict,
        )?;
        Ok(Self::from_secret(secret.to_owned()))
    }

    fn file_bytes(&self) -> Vec<u8> {
        format!("{}\n", self.secret).into_bytes()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PlanCommandV1 {
    Enroll,
    Rotate,
    Revoke,
}

impl PlanCommandV1 {
    const fn name(self) -> &'static str {
        match self {
            Self::Enroll => "enroll",
            Self::Rotate => "rotate",
            Self::Revoke => "revoke",
        }
    }

    /// Enrollment and rotation create a bearer and a public handoff.
    const fn issues_bearer(self) -> bool {
        matches!(self, Self::Enroll | Self::Rotate)
    }

    /// Exact publisher action of a completed command. A rotation keeps the
    /// credential, incarnation and grant: the publisher keeps its original
    /// enrollment and journals (whose pins accept a higher binding revision)
    /// and replaces only its bearer; installing the rotation's enrollment
    /// would make those journals foreign, and fresh journals would refuse
    /// the existing heads of the same incarnation.
    const fn publisher_action(self) -> &'static str {
        match self {
            Self::Enroll => "install_both_files_privately_with_new_owned_journals",
            Self::Rotate => {
                "replace_only_the_bearer_file_while_stopped_keep_enrollment_and_journals"
            }
            Self::Revoke => "stop_the_owned_publisher_and_retain_its_journals",
        }
    }

    /// Private files that go to the publisher host after this command.
    const fn handoff_files(self) -> &'static [&'static str] {
        match self {
            Self::Enroll => &[BEARER_FILE, ENROLLMENT_FILE],
            Self::Rotate => &[BEARER_FILE],
            Self::Revoke => &[],
        }
    }

    /// Files kept by the operator as evidence only, never installed.
    const fn evidence_files(self) -> &'static [&'static str] {
        match self {
            Self::Rotate => &[ENROLLMENT_FILE],
            Self::Enroll | Self::Revoke => &[],
        }
    }
}

/// Pins of the retained source recorded at preparation. A resume requires
/// equal pins and a closure revision that did not regress.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanSourceV1 {
    installation_id: Uuid,
    closure_id: Uuid,
    closure_revision: u64,
    source_id: Uuid,
    tenant_id: String,
    source_format: u32,
    control_namespace_uuid: Uuid,
}

impl PlanSourceV1 {
    fn of(facts: &OwnedRetainedSourceFacts) -> Self {
        Self {
            installation_id: facts.installation_id(),
            closure_id: facts.closure_id(),
            closure_revision: facts.closure_revision(),
            source_id: facts.source_id(),
            tenant_id: facts.tenant_id().to_owned(),
            source_format: facts.format().number(),
            control_namespace_uuid: facts.namespace_id(),
        }
    }

    fn validate(&self) -> Checked<()> {
        check(
            !self.installation_id.is_nil()
                && !self.closure_id.is_nil()
                && !self.source_id.is_nil()
                && !self.control_namespace_uuid.is_nil()
                && self.closure_revision > 0
                && self.closure_revision <= i64::MAX as u64
                && matches!(self.source_format, 14 | 15)
                && crate::tenancy::validate_tenant_id(&self.tenant_id).is_ok(),
            BrokerIdentityFailureV1::PlanConflict,
        )
    }

    /// Same pins; the observed closure revision may only advance.
    fn admits(&self, facts: &OwnedRetainedSourceFacts) -> bool {
        let current = Self::of(facts);
        self.installation_id == current.installation_id
            && self.closure_id == current.closure_id
            && self.source_id == current.source_id
            && self.tenant_id == current.tenant_id
            && self.source_format == current.source_format
            && self.control_namespace_uuid == current.control_namespace_uuid
            && current.closure_revision >= self.closure_revision
    }
}

/// Verified backup evidence: the restore point that precedes the plan.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanBackupV1 {
    path: PathBuf,
    sha256: String,
    audit_seq: i64,
    audit_hash: String,
}

impl PlanBackupV1 {
    fn validate(&self) -> Checked<()> {
        check(
            absolute_path(&self.path).is_ok_and(|path| path == self.path)
                && crate::control::hash(&self.sha256).is_ok()
                && self.audit_seq > 0
                && crate::control::hash(&self.audit_hash).is_ok(),
            BrokerIdentityFailureV1::PlanConflict,
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PlanBindingKindV1 {
    Issue,
    Rotate,
}

impl PlanBindingKindV1 {
    const fn step(self) -> OwnedBrokerBindingStepV1 {
        match self {
            Self::Issue => OwnedBrokerBindingStepV1::Issue,
            Self::Rotate => OwnedBrokerBindingStepV1::Rotate,
        }
    }
}

/// One planned identity-authority operation, with its fixed operation ID.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case", deny_unknown_fields)]
enum PlanStepV1 {
    Grant {
        mutation: BrokerGrantMutationV1,
    },
    Bind {
        kind: PlanBindingKindV1,
        input: WorkloadBindingIssueV1,
    },
    RevokeGrant {
        mutation: BrokerRevocationV1,
    },
    RevokeBinding {
        input: WorkloadBindingRevocationV1,
    },
}

fn broker_subject(broker_id: &str, incarnation_id: &str) -> WorkloadSubjectV1 {
    WorkloadSubjectV1::Broker {
        broker_id: broker_id.to_owned(),
        incarnation_id: incarnation_id.to_owned(),
    }
}

/// Exactly one authority, never of the Kubernetes runtime kind, with a short
/// non-path label: the owned profile v1 pins one admitted authority.
fn admitted_authorities(authorities: &[BrokerAuthorityV1]) -> bool {
    matches!(authorities, [only]
        if only.runtime_kind != BrokerRuntimeKindV1::Kubernetes
            && !only.authority_id.is_empty()
            && only.authority_id.len() <= MAX_AUTHORITY_ID_BYTES
            && only
                .authority_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte)))
}

fn hex64(value: &str) -> bool {
    crate::control::hash(value).is_ok()
}

impl PlanStepV1 {
    fn operation_id(&self) -> &str {
        match self {
            Self::Grant { mutation } => &mutation.operation_id,
            Self::Bind { input, .. } => &input.operation_id,
            Self::RevokeGrant { mutation } => &mutation.operation_id,
            Self::RevokeBinding { input } => &input.operation_id,
        }
    }

    fn validate(&self, broker_id: &str) -> Checked<()> {
        let conflict = BrokerIdentityFailureV1::PlanConflict;
        match self {
            Self::Grant { mutation } => {
                mutation.validate().map_err(|_| conflict)?;
                check(
                    mutation.broker_id == broker_id && admitted_authorities(&mutation.authorities),
                    conflict,
                )
            }
            Self::Bind { input, .. } => {
                input.validate().map_err(|_| conflict)?;
                let this_broker = match &input.subject {
                    WorkloadSubjectV1::Broker {
                        broker_id: subject, ..
                    } => subject == broker_id,
                    WorkloadSubjectV1::Worker { .. } => false,
                };
                check(this_broker, conflict)
            }
            Self::RevokeGrant { mutation } => {
                mutation.validate().map_err(|_| conflict)?;
                check(mutation.broker_id == broker_id, conflict)
            }
            Self::RevokeBinding { input } => input.validate().map_err(|_| conflict),
        }
    }

    /// Binds a recorded receipt to this exact step content.
    fn commitment(&self) -> Checked<String> {
        crate::control::content_hash(STEP_DOMAIN, self)
            .map_err(|_| BrokerIdentityFailureV1::PlanConflict)
    }
}

/// Durable private plan, written before any mutation and never rewritten.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerIdentityPlanV1 {
    version: u32,
    profile: String,
    command: PlanCommandV1,
    plan_id: Uuid,
    source: PlanSourceV1,
    broker_id: String,
    gateway_origin: Option<String>,
    backup: PlanBackupV1,
    steps: Vec<PlanStepV1>,
}

impl BrokerIdentityPlanV1 {
    fn new(
        command: PlanCommandV1,
        facts: &OwnedRetainedSourceFacts,
        broker_id: String,
        gateway_origin: Option<String>,
        backup: PlanBackupV1,
        steps: Vec<PlanStepV1>,
    ) -> Self {
        Self {
            version: PLAN_VERSION,
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            command,
            plan_id: Uuid::new_v4(),
            source: PlanSourceV1::of(facts),
            broker_id,
            gateway_origin,
            backup,
            steps,
        }
    }

    fn validate(&self) -> Checked<()> {
        let conflict = BrokerIdentityFailureV1::PlanConflict;
        check(
            self.version == PLAN_VERSION
                && self.profile == OWNED_SUPERVISION_PROFILE_V1
                && !self.plan_id.is_nil()
                && registry_uuid(&self.broker_id).is_ok(),
            conflict,
        )?;
        self.source.validate()?;
        self.backup.validate()?;
        let mut operations = BTreeSet::new();
        for step in &self.steps {
            step.validate(&self.broker_id)?;
            check(operations.insert(step.operation_id()), conflict)?;
        }
        let origin_valid = |origin: &str| {
            canonical_gateway_origin(origin).is_ok_and(|canonical| canonical == origin)
        };
        match (self.command, self.steps.as_slice(), self.gateway_origin.as_deref()) {
            (
                PlanCommandV1::Enroll,
                [
                    PlanStepV1::Grant { mutation },
                    PlanStepV1::Bind {
                        kind: PlanBindingKindV1::Issue,
                        input,
                    },
                ],
                Some(origin),
            ) => {
                let not_after = parse_time(&mutation.not_after).map_err(|_| conflict)?;
                let expires = parse_time(&input.expires_at).map_err(|_| conflict)?;
                let subject = broker_subject(&self.broker_id, &mutation.incarnation_id);
                check(
                    origin_valid(origin)
                        && input.subject == subject
                        && input.credential_id == mutation.credential_id
                        && input.expected_revision == 0
                        && expires <= not_after,
                    conflict,
                )
            }
            (
                PlanCommandV1::Rotate,
                [
                    PlanStepV1::Bind {
                        kind: PlanBindingKindV1::Rotate,
                        input,
                    },
                ],
                Some(origin),
            ) => check(origin_valid(origin) && input.expected_revision > 0, conflict),
            (PlanCommandV1::Revoke, steps, None) => {
                let bindings = match steps {
                    [PlanStepV1::RevokeGrant { .. }, rest @ ..] => rest,
                    rest => rest,
                };
                check(
                    !steps.is_empty()
                        && bindings.len() <= MAX_REVOKED_BINDINGS
                        && bindings
                            .iter()
                            .all(|step| matches!(step, PlanStepV1::RevokeBinding { .. })),
                    conflict,
                )
            }
            _ => Err(conflict),
        }
    }

    /// The single binding step of an enrollment or a rotation (its last one).
    fn binding(&self) -> Option<(usize, &WorkloadBindingIssueV1)> {
        let index = self.steps.len();
        match self.steps.last() {
            Some(PlanStepV1::Bind { input, .. }) => Some((index, input)),
            _ => None,
        }
    }

    /// The grant step of an enrollment, always its first one.
    fn grant(&self) -> Option<&BrokerGrantMutationV1> {
        match (self.command, self.steps.first()) {
            (PlanCommandV1::Enroll, Some(PlanStepV1::Grant { mutation })) => Some(mutation),
            _ => None,
        }
    }
}

/// Committed receipt of one step, normalized (`replayed = false`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StepReceiptV1 {
    /// Boxed: a registry result (broker snapshot and receipt) is much larger
    /// than a binding receipt.
    Registry { result: Box<BrokerMutationResultV1> },
    Binding { receipt: WorkloadBindingReceiptV1 },
}

impl StepReceiptV1 {
    fn replayed(&self) -> bool {
        match self {
            Self::Registry { result } => result.replayed,
            Self::Binding { receipt } => receipt.replayed,
        }
    }

    fn normalize(&mut self) {
        match self {
            Self::Registry { result } => result.replayed = false,
            Self::Binding { receipt } => receipt.replayed = false,
        }
    }

    fn audit_position(&self) -> (i64, &str) {
        match self {
            Self::Registry { result } => (
                result.receipt.audit_seq,
                result.receipt.audit_hash.as_str(),
            ),
            Self::Binding { receipt } => (receipt.audit_seq, receipt.audit_hash.as_str()),
        }
    }

    /// The receipt is the outcome of exactly this step.
    fn validate_for(&self, step: &PlanStepV1, broker_id: &str) -> Checked<()> {
        let (seq, hash) = self.audit_position();
        let audited = seq > 0 && hex64(hash);
        let ok = match (self, step) {
            (Self::Registry { result }, PlanStepV1::Grant { mutation }) => {
                let broker = &result.broker;
                result.receipt.operation_id == mutation.operation_id
                    && result.receipt.broker_id == broker_id
                    && broker.broker_id == broker_id
                    && broker.incarnation_id == mutation.incarnation_id
                    && broker.credential_id == mutation.credential_id
                    && broker.authorities == mutation.authorities
                    && !broker.revoked
                    && mutation.expected_revision.unwrap_or(0).checked_add(1)
                        == Some(broker.grant_revision)
            }
            (Self::Registry { result }, PlanStepV1::RevokeGrant { mutation }) => {
                result.receipt.operation_id == mutation.operation_id
                    && result.receipt.broker_id == broker_id
                    && result.broker.broker_id == broker_id
                    && result.broker.revoked
                    && mutation.expected_revision.checked_add(1)
                        == Some(result.broker.grant_revision)
            }
            (Self::Binding { receipt }, PlanStepV1::Bind { input, .. }) => {
                receipt.version == 1
                    && receipt.credential_id == input.credential_id
                    && input.expected_revision.checked_add(1) == Some(receipt.revision)
            }
            (Self::Binding { receipt }, PlanStepV1::RevokeBinding { input }) => {
                receipt.version == 1
                    && receipt.credential_id == input.credential_id
                    && input.expected_revision.checked_add(1) == Some(receipt.revision)
            }
            _ => false,
        };
        check(audited && ok, BrokerIdentityFailureV1::DurableConflict)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepRecordV1 {
    version: u32,
    plan_id: Uuid,
    index: u32,
    step_commitment: String,
    receipt: StepReceiptV1,
}

fn record_name(index: usize) -> String {
    format!("step-{index}.json")
}

/// Index of a well-formed `step-<n>.json` record name, without leading zero.
fn record_index(name: &str) -> Option<usize> {
    let digits = name.strip_prefix("step-")?.strip_suffix(".json")?;
    if digits.is_empty()
        || digits.len() > 3
        || digits.starts_with('0')
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    digits.parse().ok()
}

fn plan_leaf(leaf: &str) -> bool {
    !leaf.is_empty()
        && leaf.len() <= MAX_PLAN_LEAF_BYTES
        && leaf != "."
        && leaf != ".."
        && leaf
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// Exclusive immutable record: created, synchronized with its directory and
/// revalidated before it counts. Partial failures are retained, never removed.
fn save(directory: &Directory, name: &str, bytes: &[u8]) -> Checked<PrivateFile> {
    let file = directory
        .persistent_create(name, bytes)
        .map_err(|_| BrokerIdentityFailureV1::PlanConflict)?;
    file.persistent_sync()
        .map_err(|_| BrokerIdentityFailureV1::Unavailable)?;
    directory
        .sync()
        .map_err(|_| BrokerIdentityFailureV1::Unavailable)?;
    file.persistent_revalidate()
        .map_err(|_| BrokerIdentityFailureV1::PlanConflict)?;
    Ok(file)
}

/// One maintenance process per plan: a non-blocking lock on its plan file.
fn lock(file: &PrivateFile) -> Checked<()> {
    match file.persistent_try_lock() {
        Ok(true) => Ok(()),
        Ok(false) => Err(BrokerIdentityFailureV1::PlanBusy),
        Err(_) => Err(BrokerIdentityFailureV1::PlanConflict),
    }
}

/// Canonical bytes of a closed document: a re-encoding must be identical.
fn decode_exact<T>(bytes: &[u8]) -> Checked<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    let conflict = BrokerIdentityFailureV1::PlanConflict;
    let value: T = serde_json::from_slice(bytes).map_err(|_| conflict)?;
    check(
        serde_json::to_vec(&value).map_err(|_| conflict)? == bytes,
        conflict,
    )?;
    Ok(value)
}

/// The private plan directory, its locked plan file and, for an enrollment
/// or a rotation, its bearer. The bearer file precedes the plan file: once
/// the plan is visible, a missing or different bearer is an integrity
/// failure, never a late issuance.
struct PlanDirectory {
    directory: Directory,
    plan_file: PrivateFile,
    bearer_file: Option<PrivateFile>,
    plan: BrokerIdentityPlanV1,
    bearer: Option<Bearer>,
}

impl PlanDirectory {
    fn create(path: &Path, plan: BrokerIdentityPlanV1, bearer: Option<Bearer>) -> Checked<Self> {
        let invalid = BrokerIdentityFailureV1::InvalidInput;
        plan.validate()?;
        check(
            bearer.is_some() == plan.command.issues_bearer()
                && bearer.as_ref().map(|bearer| bearer.digest.as_str())
                    == plan.binding().map(|(_, input)| input.sha256_digest.as_str()),
            invalid,
        )?;
        let path = absolute_path(path).map_err(|_| invalid)?;
        let leaf = path
            .file_name()
            .and_then(OsStr::to_str)
            .filter(|leaf| plan_leaf(leaf))
            .ok_or(invalid)?;
        let parent =
            Directory::open_existing(path.parent().ok_or(invalid)?).map_err(|_| invalid)?;
        // An existing directory is never adopted: it may be another plan.
        let directory = parent
            .exclusive_child(leaf)
            .map_err(|_| BrokerIdentityFailureV1::PlanConflict)?;
        let bearer_file = bearer
            .as_ref()
            .map(|bearer| save(&directory, BEARER_FILE, &bearer.file_bytes()))
            .transpose()?;
        let bytes = serde_json::to_vec(&plan).map_err(|_| invalid)?;
        check(bytes.len() as u64 <= MAX_PLAN_BYTES, invalid)?;
        let plan_file = save(&directory, PLAN_FILE, &bytes)?;
        lock(&plan_file)?;
        directory
            .durable_bootstrap()
            .map_err(|_| BrokerIdentityFailureV1::Unavailable)?;
        Ok(Self {
            directory,
            plan_file,
            bearer_file,
            plan,
            bearer,
        })
    }

    fn open(path: &Path) -> Checked<Self> {
        let conflict = BrokerIdentityFailureV1::PlanConflict;
        let path = absolute_path(path).map_err(|_| BrokerIdentityFailureV1::InvalidInput)?;
        let directory = Directory::open_existing(&path).map_err(|_| conflict)?;
        let plan_file = directory
            .persistent_read(PLAN_FILE, MAX_PLAN_BYTES, &[])
            .map_err(|_| conflict)?
            .ok_or(BrokerIdentityFailureV1::PlanIncomplete)?;
        lock(&plan_file)?;
        let plan: BrokerIdentityPlanV1 = decode_exact(plan_file.bytes())?;
        plan.validate()?;
        let issues = plan.command.issues_bearer();
        let entries = directory
            .persistent_entries(MAX_PLAN_ENTRIES)
            .map_err(|_| conflict)?;
        for entry in &entries {
            let known = entry.name == PLAN_FILE
                || (issues && (entry.name == BEARER_FILE || entry.name == ENROLLMENT_FILE))
                || record_index(&entry.name).is_some_and(|index| index <= plan.steps.len());
            check(!entry.directory && known, conflict)?;
        }
        let bearer_file = directory
            .persistent_read(BEARER_FILE, MAX_BEARER_FILE_BYTES, &[])
            .map_err(|_| conflict)?;
        let bearer = match (issues, &bearer_file) {
            (true, Some(file)) => {
                let bearer = Bearer::parse(file.bytes())?;
                check(
                    plan.binding()
                        .is_some_and(|(_, input)| input.sha256_digest == bearer.digest),
                    conflict,
                )?;
                Some(bearer)
            }
            (false, None) => None,
            _ => return Err(conflict),
        };
        Ok(Self {
            directory,
            plan_file,
            bearer_file,
            plan,
            bearer,
        })
    }

    fn revalidate(&self) -> Checked<()> {
        let conflict = BrokerIdentityFailureV1::PlanConflict;
        self.plan_file
            .persistent_revalidate()
            .map_err(|_| conflict)?;
        if let Some(file) = &self.bearer_file {
            file.persistent_revalidate().map_err(|_| conflict)?;
        }
        Ok(())
    }

    fn record(&self, index: usize, step: &PlanStepV1) -> Checked<Option<StepRecordV1>> {
        let conflict = BrokerIdentityFailureV1::PlanConflict;
        let Some(file) = self
            .directory
            .persistent_read(&record_name(index), MAX_RECORD_BYTES, &[])
            .map_err(|_| conflict)?
        else {
            return Ok(None);
        };
        let record: StepRecordV1 = decode_exact(file.bytes())?;
        check(
            record.version == PLAN_VERSION
                && record.plan_id == self.plan.plan_id
                && usize::try_from(record.index).is_ok_and(|recorded| recorded == index)
                && record.step_commitment == step.commitment()?
                && record.receipt.validate_for(step, &self.plan.broker_id).is_ok(),
            conflict,
        )?;
        Ok(Some(record))
    }

    fn save_record(&self, index: usize, step: &PlanStepV1, receipt: &StepReceiptV1) -> Checked<()> {
        let record = StepRecordV1 {
            version: PLAN_VERSION,
            plan_id: self.plan.plan_id,
            index: u32::try_from(index).map_err(|_| BrokerIdentityFailureV1::PlanConflict)?,
            step_commitment: step.commitment()?,
            receipt: receipt.clone(),
        };
        let bytes =
            serde_json::to_vec(&record).map_err(|_| BrokerIdentityFailureV1::PlanConflict)?;
        check(
            bytes.len() as u64 <= MAX_RECORD_BYTES,
            BrokerIdentityFailureV1::PlanConflict,
        )?;
        save(&self.directory, &record_name(index), &bytes).map(|_| ())
    }

    fn enrollment(&self) -> Checked<Option<OwnedBrokerEnrollmentV1>> {
        let conflict = BrokerIdentityFailureV1::PlanConflict;
        let Some(file) = self
            .directory
            .persistent_read(ENROLLMENT_FILE, MAX_ENROLLMENT_FILE_BYTES, &[])
            .map_err(|_| conflict)?
        else {
            return Ok(None);
        };
        let enrollment: OwnedBrokerEnrollmentV1 = decode_exact(file.bytes())?;
        enrollment.validate().map_err(|_| conflict)?;
        Ok(Some(enrollment))
    }

    fn save_enrollment(&self, enrollment: &OwnedBrokerEnrollmentV1) -> Checked<()> {
        let bytes =
            serde_json::to_vec(enrollment).map_err(|_| BrokerIdentityFailureV1::HandoffRefused)?;
        save(&self.directory, ENROLLMENT_FILE, &bytes).map(|_| ())
    }
}

/// Secret screen of an output or a handoff: no literal occurrence of the
/// bearer or of its digest in any string or key, before JSON escaping.
fn exposes(value: &serde_json::Value, bearer: &Bearer) -> bool {
    let protected = |text: &str| text.contains(&bearer.secret) || text.contains(&bearer.digest);
    match value {
        serde_json::Value::String(text) => protected(text),
        serde_json::Value::Array(values) => values.iter().any(|value| exposes(value, bearer)),
        serde_json::Value::Object(values) => values
            .iter()
            .any(|(name, value)| protected(name) || exposes(value, bearer)),
        _ => false,
    }
}

fn output<T: Serialize>(value: &T, bearer: Option<&Bearer>) -> Checked<BrokerIdentityOutputV1> {
    let refused = BrokerIdentityFailureV1::OutputRefused;
    let value = serde_json::to_value(value).map_err(|_| refused)?;
    check(
        bearer.is_none_or(|bearer| !exposes(&value, bearer)),
        refused,
    )?;
    let mut bytes = serde_json::to_vec_pretty(&value).map_err(|_| refused)?;
    bytes.push(b'\n');
    Ok(BrokerIdentityOutputV1 { bytes })
}

/// Canonical control-time form (UTC, milliseconds) of an operator time.
fn operator_time(value: &str) -> Checked<DateTime<Utc>> {
    let invalid = BrokerIdentityFailureV1::InvalidInput;
    let time = parse_time(value).map_err(|_| invalid)?;
    parse_time(&format_time(time)).map_err(|_| invalid)
}

fn control_now() -> Checked<DateTime<Utc>> {
    parse_time(&format_time(Utc::now())).map_err(|_| BrokerIdentityFailureV1::ClockUncertain)
}

fn authority(failure: HumanAuthorityFailure) -> BrokerIdentityFailureV1 {
    match failure {
        HumanAuthorityFailure::InvalidManifest
        | HumanAuthorityFailure::ConfigurationMismatch
        | HumanAuthorityFailure::StorageBindingMismatch
        | HumanAuthorityFailure::OwnerInvalid
        | HumanAuthorityFailure::SourceMismatch => BrokerIdentityFailureV1::UnionMismatch,
        HumanAuthorityFailure::StorageUnavailable
        | HumanAuthorityFailure::OwnerContended
        | HumanAuthorityFailure::Closing
        | HumanAuthorityFailure::GateBusy
        | HumanAuthorityFailure::EpochExpired
        | HumanAuthorityFailure::TaskFailed
        | HumanAuthorityFailure::OpeningTimedOut => BrokerIdentityFailureV1::Unavailable,
    }
}

fn owned(code: Code) -> BrokerIdentityFailureV1 {
    match code {
        Code::AuthenticationRequired | Code::Forbidden => BrokerIdentityFailureV1::StateRefused,
        Code::InvalidRequest | Code::ProtocolMismatch => BrokerIdentityFailureV1::InvalidInput,
        Code::IdentityMismatch | Code::OperationConflict | Code::SequenceConflict => {
            BrokerIdentityFailureV1::DurableConflict
        }
        Code::ClockUncertain => BrokerIdentityFailureV1::ClockUncertain,
        Code::MigrationRequired => BrokerIdentityFailureV1::MigrationRequired,
        Code::SourceRebuildRequired => BrokerIdentityFailureV1::UnionMismatch,
        Code::SupervisionUnavailable | Code::ResponseWithheld => {
            BrokerIdentityFailureV1::Unavailable
        }
        Code::AuditUnavailable => BrokerIdentityFailureV1::AuditUnavailable,
        Code::ResponseUnpublishable => BrokerIdentityFailureV1::OutputRefused,
        Code::CredentialAmbiguous => BrokerIdentityFailureV1::CredentialCollision,
    }
}

/// Closed classification of a Store refusal, with the single owned mapping.
fn classify(store: &Store, error: &anyhow::Error) -> BrokerIdentityFailureV1 {
    if let Some(code) = error.downcast_ref::<Code>() {
        return owned(*code);
    }
    if let Some(failure) = error.downcast_ref::<HumanAuthorityFailure>() {
        return authority(*failure);
    }
    owned(store.owned_failure_code(error))
}

/// The handoff read-back keeps temporary and audit refusals; any other
/// refusal means that the new credential does not authenticate as planned.
fn handoff_failure(failure: BrokerIdentityFailureV1) -> BrokerIdentityFailureV1 {
    match failure {
        BrokerIdentityFailureV1::Unavailable
        | BrokerIdentityFailureV1::AuditUnavailable
        | BrokerIdentityFailureV1::ClockUncertain => failure,
        _ => BrokerIdentityFailureV1::HandoffRefused,
    }
}

/// Selected tenant source of the retained union.
struct Selected<'a> {
    store: &'a Store,
    source: OwnedSourceToken,
    facts: OwnedRetainedSourceFacts,
}

/// `files` are the private files to transfer to the publisher host;
/// `evidence_files` stay with the operator. After a rotation the enrollment
/// (and its commitment below) is evidence only: the publisher keeps its
/// original enrollment.
#[derive(Serialize)]
struct HandoffReportV1<'a> {
    files: &'static [&'static str],
    evidence_files: &'static [&'static str],
    enrollment_commitment: String,
    subject: &'a OwnedBrokerSubjectV1,
    closure_revision: u64,
    grant_not_after: &'a str,
    credential_expires_at: &'a str,
    issued_at: &'a str,
    publisher_action: &'static str,
}

#[derive(Serialize)]
struct ReportV1<'a> {
    version: u32,
    profile: &'static str,
    command: &'static str,
    state: &'static str,
    tenant_id: &'a str,
    broker_id: &'a str,
    completed_steps: usize,
    total_steps: usize,
    handoff: Option<HandoffReportV1<'a>>,
    candidate_index: &'static str,
    qualification: &'static str,
}

#[derive(Serialize)]
struct HistoryReportV1<'a> {
    credential_id: &'a str,
    count_scope: &'static str,
    items: serde_json::Value,
    has_more: bool,
}

#[derive(Serialize)]
struct StatusReportV1<'a> {
    version: u32,
    profile: &'static str,
    command: &'static str,
    tenant_id: &'a str,
    broker_id: &'a str,
    provenance: &'static str,
    evaluated_at: &'a str,
    broker_presence: &'static str,
    broker: serde_json::Value,
    bindings: serde_json::Value,
    history: Option<HistoryReportV1<'a>>,
    liveness: &'static str,
    qualification: &'static str,
}

/// Offline owned broker identity maintenance over one opened owned union.
/// Dropping it closes the retained union like an aborted gateway start.
pub struct BrokerIdentityMaintenance {
    bundle: HumanOwnedStartupBundle,
    technical: Vec<[u8; 32]>,
}

impl BrokerIdentityMaintenance {
    /// Second consumer of the owned startup bundle, besides the gateway. The
    /// technical digests are those of the configured runner/viewer tokens
    /// (SHA-256 of their bytes): a new bearer must collide with none of them.
    pub fn open(
        bundle: HumanOwnedStartupBundle,
        technical_digests: Vec<[u8; 32]>,
    ) -> Result<Self, BrokerIdentityRefusalV1> {
        let tenants = bundle
            .owner
            .closure()
            .sources
            .iter()
            .filter(|source| source.kind == HumanSourceKindV1::TenantStore)
            .count();
        check(
            tenants > 0 && tenants == bundle.stores.len() && !technical_digests.is_empty(),
            BrokerIdentityFailureV1::UnionMismatch,
        )?;
        Ok(Self {
            bundle,
            technical: technical_digests,
        })
    }

    /// Enrollment: fresh incarnation and credential IDs, a grant with exactly
    /// one admitted authority, then a digest-only broker binding of a new
    /// bearer, then the private handoff.
    pub fn enroll(
        &self,
        request: BrokerEnrollmentRequestV1,
    ) -> Result<BrokerIdentityOutputV1, BrokerIdentityRefusalV1> {
        let invalid = BrokerIdentityFailureV1::InvalidInput;
        let selected = self.select(&request.tenant_id)?;
        let now = control_now()?;
        let origin = canonical_gateway_origin(&request.gateway_origin).map_err(|_| invalid)?;
        let authority = BrokerAuthorityV1 {
            authority_id: request.authority_id,
            runtime_kind: request.runtime_kind,
        };
        let authorities = vec![authority];
        check(
            admitted_authorities(&authorities)
                && (1..=MAX_LEASE_SECONDS).contains(&request.lease_seconds),
            invalid,
        )?;
        let latest = now
            .checked_add_signed(Duration::days(MAX_GRANT_LIFETIME_DAYS))
            .ok_or(invalid)?;
        let not_after = operator_time(&request.grant_not_after)?;
        check(not_after > now && not_after <= latest, invalid)?;
        let expires = match &request.credential_expires_at {
            Some(value) => operator_time(value)?,
            None => not_after,
        };
        check(expires > now && expires <= not_after, invalid)?;
        let broker_id = match request.broker {
            BrokerSelectionV1::Existing(id) if !id.is_nil() => id,
            BrokerSelectionV1::Existing(_) => return Err(invalid.into()),
            BrokerSelectionV1::New => Uuid::new_v4(),
        }
        .to_string();
        let backup = self.verify_backup(&selected, &request.backup)?;
        let status = self.read(&selected, |store, access| {
            store.owned_broker_identity_status(access, &selected.facts, &broker_id)
        })?;
        let expected_revision = match (&request.broker, &status.broker) {
            (_, None) => None,
            (BrokerSelectionV1::New, Some(_)) => {
                return Err(BrokerIdentityFailureV1::StateRefused.into());
            }
            (BrokerSelectionV1::Existing(_), Some(view)) => {
                check(
                    view.state != BrokerStateV1::ClockUncertain,
                    BrokerIdentityFailureV1::ClockUncertain,
                )?;
                let active = !view.broker.revoked
                    && !matches!(view.state, BrokerStateV1::Expired | BrokerStateV1::Revoked);
                check(
                    !active || request.replace_active_grant,
                    BrokerIdentityFailureV1::StateRefused,
                )?;
                Some(view.broker.grant_revision)
            }
        };
        let bearer = Bearer::generate()?;
        self.require_unique(&bearer)?;
        let incarnation_id = Uuid::new_v4().to_string();
        let credential_id = Uuid::new_v4().to_string();
        let grant = BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: broker_id.clone(),
            expected_revision,
            credential_id: credential_id.clone(),
            incarnation_id: incarnation_id.clone(),
            authorities,
            lease_seconds: request.lease_seconds,
            not_after: format_time(not_after),
        };
        let binding = WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id,
            expected_revision: 0,
            subject: broker_subject(&broker_id, &incarnation_id),
            sha256_digest: bearer.digest.clone(),
            expires_at: format_time(expires),
        };
        let plan = BrokerIdentityPlanV1::new(
            PlanCommandV1::Enroll,
            &selected.facts,
            broker_id,
            Some(origin),
            backup,
            vec![
                PlanStepV1::Grant { mutation: grant },
                PlanStepV1::Bind {
                    kind: PlanBindingKindV1::Issue,
                    input: binding,
                },
            ],
        );
        let directory = PlanDirectory::create(&request.plan_dir, plan, Some(bearer))?;
        self.complete(&selected, &directory)
    }

    /// Rotation: a new bearer digest for the same credential, broker,
    /// incarnation and grant; the binding revision advances and a new private
    /// handoff is written. The previous bearer stops authenticating.
    pub fn rotate(
        &self,
        request: BrokerRotationRequestV1,
    ) -> Result<BrokerIdentityOutputV1, BrokerIdentityRefusalV1> {
        let invalid = BrokerIdentityFailureV1::InvalidInput;
        let refused = BrokerIdentityFailureV1::StateRefused;
        let selected = self.select(&request.tenant_id)?;
        let now = control_now()?;
        let origin = canonical_gateway_origin(&request.gateway_origin).map_err(|_| invalid)?;
        check(!request.broker_id.is_nil(), invalid)?;
        let broker_id = request.broker_id.to_string();
        let backup = self.verify_backup(&selected, &request.backup)?;
        let status = self.read(&selected, |store, access| {
            store.owned_broker_identity_status(access, &selected.facts, &broker_id)
        })?;
        let view = status.broker.as_ref().ok_or(refused)?;
        check(
            view.state != BrokerStateV1::ClockUncertain,
            BrokerIdentityFailureV1::ClockUncertain,
        )?;
        let broker = &view.broker;
        check(
            !broker.revoked
                && !matches!(view.state, BrokerStateV1::Expired | BrokerStateV1::Revoked)
                && admitted_authorities(&broker.authorities),
            refused,
        )?;
        let not_after =
            parse_time(&broker.not_after).map_err(|_| BrokerIdentityFailureV1::DurableConflict)?;
        let expires = match &request.credential_expires_at {
            Some(value) => operator_time(value)?,
            None => not_after,
        };
        check(expires > now && expires <= not_after, invalid)?;
        let current = status
            .bindings
            .iter()
            .find(|binding| binding.credential_id == broker.credential_id)
            .ok_or(refused)?;
        let current_expiry = parse_time(&current.expires_at)
            .map_err(|_| BrokerIdentityFailureV1::DurableConflict)?;
        check(!current.revoked && current_expiry > now, refused)?;
        let bearer = Bearer::generate()?;
        self.require_unique(&bearer)?;
        let binding = WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: broker.credential_id.clone(),
            expected_revision: current.revision,
            subject: broker_subject(&broker_id, &broker.incarnation_id),
            sha256_digest: bearer.digest.clone(),
            expires_at: format_time(expires),
        };
        let plan = BrokerIdentityPlanV1::new(
            PlanCommandV1::Rotate,
            &selected.facts,
            broker_id,
            Some(origin),
            backup,
            vec![PlanStepV1::Bind {
                kind: PlanBindingKindV1::Rotate,
                input: binding,
            }],
        );
        let directory = PlanDirectory::create(&request.plan_dir, plan, Some(bearer))?;
        self.complete(&selected, &directory)
    }

    /// Revocation of the grant and/or the unrevoked bindings of its current
    /// incarnation. History and digests are kept: a revoked digest stays a
    /// collision and is never reissued.
    pub fn revoke(
        &self,
        request: BrokerRevocationRequestV1,
    ) -> Result<BrokerIdentityOutputV1, BrokerIdentityRefusalV1> {
        let refused = BrokerIdentityFailureV1::StateRefused;
        let selected = self.select(&request.tenant_id)?;
        check(
            !request.broker_id.is_nil(),
            BrokerIdentityFailureV1::InvalidInput,
        )?;
        let broker_id = request.broker_id.to_string();
        let backup = self.verify_backup(&selected, &request.backup)?;
        let status = self.read(&selected, |store, access| {
            store.owned_broker_identity_status(access, &selected.facts, &broker_id)
        })?;
        let view = status.broker.as_ref().ok_or(refused)?;
        let broker = &view.broker;
        let mut steps = Vec::new();
        if matches!(
            request.scope,
            BrokerRevocationScopeV1::All | BrokerRevocationScopeV1::Grant
        ) && !broker.revoked
        {
            steps.push(PlanStepV1::RevokeGrant {
                mutation: BrokerRevocationV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    broker_id: broker_id.clone(),
                    expected_revision: broker.grant_revision,
                },
            });
        }
        if matches!(
            request.scope,
            BrokerRevocationScopeV1::All | BrokerRevocationScopeV1::Binding
        ) {
            for binding in status.bindings.iter().filter(|binding| !binding.revoked) {
                steps.push(PlanStepV1::RevokeBinding {
                    input: WorkloadBindingRevocationV1 {
                        version: 1,
                        operation_id: Uuid::new_v4().to_string(),
                        credential_id: binding.credential_id.clone(),
                        expected_revision: binding.revision,
                    },
                });
            }
        }
        check(!steps.is_empty(), refused)?;
        let plan = BrokerIdentityPlanV1::new(
            PlanCommandV1::Revoke,
            &selected.facts,
            broker_id,
            None,
            backup,
            steps,
        );
        let directory = PlanDirectory::create(&request.plan_dir, plan, None)?;
        self.complete(&selected, &directory)
    }

    /// Finish an existing plan: equal pins, the same restore point still a
    /// prefix and its file unchanged, then the exact operations in order.
    pub fn resume(
        &self,
        plan_dir: &Path,
    ) -> Result<BrokerIdentityOutputV1, BrokerIdentityRefusalV1> {
        let directory = PlanDirectory::open(plan_dir)?;
        let selected = self.select(&directory.plan.source.tenant_id)?;
        check(
            directory.plan.source.admits(&selected.facts),
            BrokerIdentityFailureV1::UnionMismatch,
        )?;
        self.reverify_backup(&selected, &directory.plan.backup)?;
        self.complete(&selected, &directory)
    }

    /// Durable grant, current bindings and one digest-free history page of a
    /// broker credential. An absent broker is a complete durable absence in
    /// this tenant Store; an empty page is never evidence of no history.
    pub fn status(
        &self,
        request: BrokerIdentityStatusRequestV1,
    ) -> Result<BrokerIdentityOutputV1, BrokerIdentityRefusalV1> {
        let selected = self.select(&request.tenant_id)?;
        check(
            !request.broker_id.is_nil()
                && (1..=MAX_HISTORY_PAGE).contains(&request.history_limit)
                && request.before_revision.is_none_or(|revision| revision > 0)
                && request.credential_id.is_none_or(|id| !id.is_nil()),
            BrokerIdentityFailureV1::InvalidInput,
        )?;
        let broker_id = request.broker_id.to_string();
        let facts = &selected.facts;
        let (status, history) = self.read(&selected, |store, access| {
            let status = store.owned_broker_identity_status(access, facts, &broker_id)?;
            let credential = request.credential_id.map(|id| id.to_string()).or_else(|| {
                status
                    .broker
                    .as_ref()
                    .map(|view| view.broker.credential_id.clone())
            });
            let history = match credential {
                Some(credential) => {
                    let page = store.owned_broker_binding_history(
                        access,
                        facts,
                        &credential,
                        request.before_revision,
                        request.history_limit,
                    )?;
                    Some((credential, page))
                }
                None => None,
            };
            Ok((status, history))
        })?;
        let refused = BrokerIdentityFailureV1::OutputRefused;
        let history = match &history {
            Some((credential, page)) => Some(HistoryReportV1 {
                credential_id: credential,
                count_scope: "page",
                items: serde_json::to_value(&page.items).map_err(|_| refused)?,
                has_more: page.has_more,
            }),
            None => None,
        };
        let report = StatusReportV1 {
            version: 1,
            profile: OWNED_SUPERVISION_PROFILE_V1,
            command: "status",
            tenant_id: &request.tenant_id,
            broker_id: &broker_id,
            provenance: "durable_control_read",
            evaluated_at: &status.evaluated_at,
            broker_presence: if status.broker.is_some() {
                "present"
            } else {
                "absent_from_complete_durable_read"
            },
            broker: serde_json::to_value(&status.broker).map_err(|_| refused)?,
            bindings: serde_json::to_value(&status.bindings).map_err(|_| refused)?,
            history,
            liveness: "heartbeat_declared_by_broker_reception_observed_by_control",
            qualification: "not_run",
        };
        Ok(output(&report, None)?)
    }

    fn select(&self, tenant_id: &str) -> Checked<Selected<'_>> {
        crate::tenancy::validate_tenant_id(tenant_id)
            .map_err(|_| BrokerIdentityFailureV1::InvalidInput)?;
        let mismatch = BrokerIdentityFailureV1::UnionMismatch;
        let descriptor = self
            .bundle
            .owner
            .closure()
            .sources
            .iter()
            .find(|source| {
                source.kind == HumanSourceKindV1::TenantStore
                    && source.tenant_id.as_deref() == Some(tenant_id)
            })
            .ok_or(mismatch)?;
        let source = self
            .bundle
            .owner
            .source_token(descriptor.source_id)
            .map_err(authority)?;
        let store = self.bundle.stores.get(&descriptor.source_id).ok_or(mismatch)?;
        let facts = OwnedRetainedSourceFacts::from_token(&source).map_err(owned)?;
        check(
            facts.tenant_id() == tenant_id && store.tenant_id() == tenant_id,
            mismatch,
        )?;
        Ok(Selected {
            store,
            source,
            facts,
        })
    }

    /// One read under a fresh read epoch of the selected source.
    fn read<T>(
        &self,
        selected: &Selected<'_>,
        work: impl FnOnce(&Store, &HumanSourceAccess<'_>) -> anyhow::Result<T>,
    ) -> Checked<T> {
        let epoch = self.bundle.owner.try_read_epoch().map_err(authority)?;
        let access = HumanSourceAccess::read(&epoch, &selected.source)
            .map_err(|error| classify(selected.store, &error))?;
        let value =
            work(selected.store, &access).map_err(|error| classify(selected.store, &error))?;
        epoch.check().map_err(authority)?;
        Ok(value)
    }

    /// One identity-authority write under a fresh write epoch. The writer
    /// validates the epoch before replay and around its commit; no further
    /// check here may discard a committed receipt.
    fn write<T>(
        &self,
        selected: &Selected<'_>,
        work: impl FnOnce(&Store, &HumanSourcesWriteEpoch, &OwnedSourceToken) -> anyhow::Result<T>,
    ) -> Checked<T> {
        let epoch = self.bundle.owner.try_write_epoch().map_err(authority)?;
        work(selected.store, &epoch, &selected.source)
            .map_err(|error| classify(selected.store, &error))
    }

    /// The backup must be a private file outside every source directory and
    /// distinct from every configuration file of the union.
    fn backup_outside_union(&self, path: &Path) -> Checked<()> {
        let refused = BrokerIdentityFailureV1::BackupRefused;
        let name = path.file_name().ok_or(refused)?;
        let parent = Directory::open_existing(path.parent().ok_or(refused)?).map_err(|_| refused)?;
        let entry = parent
            .private_entry(name)
            .map_err(|_| refused)?
            .ok_or(refused)?
            .physical_identity();
        let directory = parent.physical_identity().map_err(|_| refused)?;
        let closure = self.bundle.owner.closure();
        for source in &closure.sources {
            check(
                source.storage_binding.source_identity() != entry
                    && source.storage_binding.parent_identity() != directory,
                refused,
            )?;
        }
        for file in &closure.configuration.files {
            check(file.storage_binding.source_identity() != entry, refused)?;
        }
        Ok(())
    }

    /// Preparation: the verified backup head is exactly the source state
    /// before this session's own incarnation record, the current chain head.
    fn verify_backup(
        &self,
        selected: &Selected<'_>,
        proof: &BrokerIdentityBackupV1,
    ) -> Checked<PlanBackupV1> {
        let refused = BrokerIdentityFailureV1::BackupRefused;
        let path = absolute_path(&proof.path).map_err(|_| refused)?;
        check(hex64(&proof.sha256), refused)?;
        self.backup_outside_union(&path)?;
        let inspection = selected
            .store
            .owned_inspect_backup(&selected.facts, &path, &proof.sha256)
            .map_err(|_| refused)?;
        let seq = inspection.head_seq();
        let position = self.read(selected, |store, access| {
            store.owned_chain_position(access, &selected.facts, seq)
        })?;
        // The only event after the backup head is this session's own
        // incarnation record, which is the current chain head.
        let head = (position.head_seq, position.head_hash.as_str());
        let incarnation_is_head = position
            .incarnation
            .as_ref()
            .is_some_and(|(at, hash)| (*at, hash.as_str()) == head);
        check(
            position.hash_at.as_deref() == Some(inspection.head_hash())
                && seq.checked_add(1) == Some(position.head_seq)
                && incarnation_is_head,
            refused,
        )?;
        Ok(PlanBackupV1 {
            path,
            sha256: proof.sha256.clone(),
            audit_seq: seq,
            audit_hash: inspection.head_hash().to_owned(),
        })
    }

    /// Resume: the recorded restore point is unchanged and still a prefix.
    fn reverify_backup(&self, selected: &Selected<'_>, backup: &PlanBackupV1) -> Checked<()> {
        let refused = BrokerIdentityFailureV1::BackupRefused;
        self.backup_outside_union(&backup.path)?;
        let inspection = selected
            .store
            .owned_inspect_backup(&selected.facts, &backup.path, &backup.sha256)
            .map_err(|_| refused)?;
        check(
            inspection.head_seq() == backup.audit_seq
                && inspection.head_hash() == backup.audit_hash,
            refused,
        )?;
        let position = self.read(selected, |store, access| {
            store.owned_chain_position(access, &selected.facts, backup.audit_seq)
        })?;
        check(
            position.hash_at.as_deref() == Some(backup.audit_hash.as_str()),
            refused,
        )
    }

    /// The new bearer's digest belongs to no technical credential and to no
    /// human or workload credential (current or historical) of any source.
    fn require_unique(&self, bearer: &Bearer) -> Checked<()> {
        let collision = BrokerIdentityFailureV1::CredentialCollision;
        check(!self.technical.contains(&bearer.digest_bytes), collision)?;
        for (source_id, store) in &self.bundle.stores {
            let source = self
                .bundle
                .owner
                .source_token(*source_id)
                .map_err(authority)?;
            let epoch = self.bundle.owner.try_read_epoch().map_err(authority)?;
            let access = HumanSourceAccess::read(&epoch, &source)
                .map_err(|error| classify(store, &error))?;
            let mut seen = false;
            store
                .modern_visit_candidates(&access, |digest, _| {
                    seen |= digest == bearer.digest_bytes;
                    Ok(())
                })
                .map_err(|error| classify(store, &error))?;
            epoch.check().map_err(authority)?;
            check(!seen, collision)?;
        }
        Ok(())
    }

    /// Every step in order, then the handoff and the report. A refusal names
    /// how many steps this run confirmed.
    fn complete(
        &self,
        selected: &Selected<'_>,
        directory: &PlanDirectory,
    ) -> Result<BrokerIdentityOutputV1, BrokerIdentityRefusalV1> {
        let total = directory.plan.steps.len();
        let mut receipts = Vec::with_capacity(total);
        for (offset, step) in directory.plan.steps.iter().enumerate() {
            let receipt = self
                .run_step(selected, directory, offset + 1, step)
                .map_err(|failure| BrokerIdentityRefusalV1::at(failure, offset, total))?;
            receipts.push(receipt);
        }
        let partial = |failure| BrokerIdentityRefusalV1::at(failure, total, total);
        let enrollment = if directory.plan.command.issues_bearer() {
            Some(
                self.handoff(selected, directory, &receipts)
                    .map_err(partial)?,
            )
        } else {
            None
        };
        directory.revalidate().map_err(partial)?;
        self.report(directory, enrollment.as_ref()).map_err(partial)
    }

    /// One step. A recorded receipt must still be durable before its exact
    /// replay; an absence (an older control backup restored) stops before any
    /// write. A new receipt is validated, then recorded exclusively.
    fn run_step(
        &self,
        selected: &Selected<'_>,
        directory: &PlanDirectory,
        index: usize,
        step: &PlanStepV1,
    ) -> Checked<StepReceiptV1> {
        let recorded = directory.record(index, step)?;
        if let Some(record) = &recorded {
            let (seq, hash) = record.receipt.audit_position();
            let position = self.read(selected, |store, access| {
                store.owned_chain_position(access, &selected.facts, seq)
            })?;
            check(
                position.hash_at.as_deref() == Some(hash),
                BrokerIdentityFailureV1::PlanConflict,
            )?;
        }
        let mut receipt = self.apply(selected, &directory.plan.broker_id, step)?;
        let replayed = receipt.replayed();
        receipt.normalize();
        receipt.validate_for(step, &directory.plan.broker_id)?;
        match recorded {
            Some(record) => check(
                replayed && record.receipt == receipt,
                BrokerIdentityFailureV1::PlanConflict,
            )?,
            None => directory.save_record(index, step, &receipt)?,
        }
        Ok(receipt)
    }

    /// Applies one step. A binding revocation is bound to the plan's broker
    /// in the Store, so a step naming another broker's credential is refused.
    fn apply(
        &self,
        selected: &Selected<'_>,
        broker_id: &str,
        step: &PlanStepV1,
    ) -> Checked<StepReceiptV1> {
        match step {
            PlanStepV1::Grant { mutation } => self
                .write(selected, |store, epoch, source| {
                    store.owned_broker_grant(epoch, source, mutation)
                })
                .map(|result| StepReceiptV1::Registry {
                    result: Box::new(result),
                }),
            PlanStepV1::RevokeGrant { mutation } => self
                .write(selected, |store, epoch, source| {
                    store.owned_broker_revoke(epoch, source, mutation)
                })
                .map(|result| StepReceiptV1::Registry {
                    result: Box::new(result),
                }),
            PlanStepV1::Bind { kind, input } => self
                .write(selected, |store, epoch, source| {
                    store.owned_broker_binding_issue(epoch, source, kind.step(), input)
                })
                .map(|receipt| StepReceiptV1::Binding { receipt }),
            PlanStepV1::RevokeBinding { input } => self
                .write(selected, |store, epoch, source| {
                    store.owned_broker_binding_revoke(epoch, source, broker_id, input)
                })
                .map(|receipt| StepReceiptV1::Binding { receipt }),
        }
    }

    /// Public handoff from committed receipts only. The new bearer is
    /// authenticated through the gateway's scoped Store path and the engaged
    /// identity is rebuilt there; grant, binding and receipts must agree. An
    /// existing handoff is checked against the receipts, never rewritten.
    fn handoff(
        &self,
        selected: &Selected<'_>,
        directory: &PlanDirectory,
        receipts: &[StepReceiptV1],
    ) -> Checked<OwnedBrokerEnrollmentV1> {
        let refused = BrokerIdentityFailureV1::HandoffRefused;
        let plan = &directory.plan;
        let bearer = directory
            .bearer
            .as_ref()
            .ok_or(BrokerIdentityFailureV1::PlanIncomplete)?;
        let origin = plan.gateway_origin.as_deref().ok_or(refused)?;
        let (index, input) = plan.binding().ok_or(refused)?;
        let Some(StepReceiptV1::Binding { receipt: binding }) = receipts.get(index - 1) else {
            return Err(refused);
        };
        let recorded_grant = match plan.grant() {
            Some(_) => match receipts.first() {
                Some(StepReceiptV1::Registry { result }) => Some(&result.receipt),
                _ => return Err(refused),
            },
            None => None,
        };
        if let Some(existing) = directory.enrollment()? {
            check(
                existing.gateway_origin == origin
                    && existing.subject.broker_id == plan.broker_id
                    && existing.subject.credential_id == input.credential_id
                    && existing.subject.binding_revision == binding.revision
                    && existing.binding_receipt.operation_id == input.operation_id
                    && existing.binding_receipt.audit_seq == binding.audit_seq
                    && existing.binding_receipt.audit_hash == binding.audit_hash
                    && recorded_grant.is_none_or(|grant| {
                        existing.grant_receipt.operation_id == grant.operation_id
                            && existing.grant_receipt.audit_seq == grant.audit_seq
                            && existing.grant_receipt.audit_hash == grant.audit_hash
                    }),
                BrokerIdentityFailureV1::PlanConflict,
            )?;
            return Ok(existing);
        }
        let facts = &selected.facts;
        let (identity, grant, status) = self
            .read(selected, |store, access| {
                let proof = store.owned_authenticate_broker(access, facts, &bearer.secret)?;
                let identity = store.owned_broker_identity(access, facts, &proof)?;
                let grant = store.owned_broker_grant_receipt(access, facts, &plan.broker_id)?;
                let status = store.owned_broker_identity_status(access, facts, &plan.broker_id)?;
                Ok((identity, grant, status))
            })
            .map_err(handoff_failure)?;
        let subject = &identity.subject;
        let view = status.broker.as_ref().ok_or(refused)?;
        let broker = &view.broker;
        let row = status
            .bindings
            .iter()
            .find(|row| row.credential_id == subject.credential_id)
            .ok_or(refused)?;
        let authority = match broker.authorities.as_slice() {
            [only] if admitted_authorities(&broker.authorities) => only.clone(),
            _ => return Err(refused),
        };
        let incarnation = match &input.subject {
            WorkloadSubjectV1::Broker { incarnation_id, .. } => incarnation_id,
            WorkloadSubjectV1::Worker { .. } => return Err(refused),
        };
        // The subject rebuilt by the Store carries the plan's source pins.
        let pins = &plan.source;
        check(
            pins.admits(facts)
                && subject.installation_id == pins.installation_id.to_string()
                && subject.closure_id == pins.closure_id.to_string()
                && subject.source_id == pins.source_id.to_string()
                && identity.closure_revision >= pins.closure_revision,
            refused,
        )?;
        check(
            subject.tenant_id == pins.tenant_id
                && subject.source_format == pins.source_format
                && subject.control_namespace_uuid == pins.control_namespace_uuid.to_string()
                && subject.broker_id == plan.broker_id
                && subject.credential_id == input.credential_id
                && subject.incarnation_id == *incarnation
                && subject.binding_revision == binding.revision
                && subject.grant_revision == broker.grant_revision
                && !broker.revoked
                && broker.credential_id == input.credential_id
                && grant.broker_id == plan.broker_id
                && grant.grant_revision == broker.grant_revision
                && recorded_grant.is_none_or(|recorded| *recorded == grant)
                && row.revision == binding.revision
                && row.audit_seq == binding.audit_seq
                && !row.revoked
                && row.expires_at == input.expires_at,
            refused,
        )?;
        let enrollment = OwnedBrokerEnrollmentV1 {
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            version: OWNED_SUPERVISION_VERSION_V1,
            gateway_origin: origin.to_owned(),
            closure_revision: identity.closure_revision,
            subject: identity.subject.clone(),
            authority,
            lease_seconds: broker.lease_seconds,
            grant_not_after: broker.not_after.clone(),
            credential_expires_at: row.expires_at.clone(),
            issued_at: row.updated_at.clone(),
            grant_receipt: OwnedEnrollmentReceiptV1 {
                operation_id: grant.operation_id.clone(),
                audit_seq: grant.audit_seq,
                audit_hash: grant.audit_hash.clone(),
            },
            binding_receipt: OwnedEnrollmentReceiptV1 {
                operation_id: input.operation_id.clone(),
                audit_seq: binding.audit_seq,
                audit_hash: binding.audit_hash.clone(),
            },
        };
        enrollment.validate().map_err(|_| refused)?;
        OwnedSupervisionPinsV1::from_enrollment(&enrollment).map_err(|_| refused)?;
        let value = serde_json::to_value(&enrollment).map_err(|_| refused)?;
        check(!exposes(&value, bearer), refused)?;
        directory.save_enrollment(&enrollment)?;
        Ok(enrollment)
    }

    fn report(
        &self,
        directory: &PlanDirectory,
        enrollment: Option<&OwnedBrokerEnrollmentV1>,
    ) -> Checked<BrokerIdentityOutputV1> {
        let plan = &directory.plan;
        let handoff = match enrollment {
            Some(enrollment) => Some(HandoffReportV1 {
                files: plan.command.handoff_files(),
                evidence_files: plan.command.evidence_files(),
                enrollment_commitment: enrollment
                    .commitment()
                    .map_err(|_| BrokerIdentityFailureV1::HandoffRefused)?,
                subject: &enrollment.subject,
                closure_revision: enrollment.closure_revision,
                grant_not_after: &enrollment.grant_not_after,
                credential_expires_at: &enrollment.credential_expires_at,
                issued_at: &enrollment.issued_at,
                publisher_action: plan.command.publisher_action(),
            }),
            None => None,
        };
        let report = ReportV1 {
            version: 1,
            profile: OWNED_SUPERVISION_PROFILE_V1,
            command: plan.command.name(),
            state: "complete",
            tenant_id: &plan.source.tenant_id,
            broker_id: &plan.broker_id,
            completed_steps: plan.steps.len(),
            total_steps: plan.steps.len(),
            handoff,
            candidate_index: "rebuilt_at_next_gateway_start",
            qualification: "not_run",
        };
        output(&report, directory.bearer.as_ref())
    }
}
