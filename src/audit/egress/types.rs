use super::*;

pub(super) const POLICY_BYTES: usize = 65_536;
pub(super) const EVENT_BYTES: usize = 98_304;
pub(super) const RECEIPT_BYTES: usize = 4_096;
pub(super) const GENESIS_KIND: &str = "control.egress_policy.genesis";
pub(super) const REPLACE_KIND: &str = "control.egress_policy.replaced";
pub(super) const REVOKE_KIND: &str = "control.egress_policy.revoke_requested";
// Canonical ASCII LDH names need no JSON escaping; every other field is fixed
// or a bounded integer/UUID/digest. Check before cloning/serialization as well.
const MAX_CLOSED_POLICY_BYTES: usize = 128 * (253 + 96) + 1_024;
const _: () = assert!(MAX_CLOSED_POLICY_BYTES <= POLICY_BYTES);
const _: () = assert!(MAX_CLOSED_POLICY_BYTES + 12_288 < EVENT_BYTES);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EgressStoreFailure {
    Invalid,
    Conflict,
    Capacity,
    AuditReservationExhausted,
    Integrity,
    Permission,
    MigrationRequired,
    ScopeConflict,
    DiscoveryStale,
}
impl std::fmt::Display for EgressStoreFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid bounded egress policy request",
            Self::Conflict => "egress policy revision or operation conflict",
            Self::Capacity => "retained egress policy or terminal audit capacity exhausted",
            Self::AuditReservationExhausted => {
                "audit capacity protected by held egress terminal reservations"
            }
            Self::Integrity => "egress policy audit projection mismatch",
            Self::Permission => "egress policy source or control authority unavailable",
            Self::MigrationRequired => "explicit backed-up egress policy migration required",
            Self::ScopeConflict => "egress expected scope conflicts with authenticated source",
            Self::DiscoveryStale => "owned control discovery requires rebuilding",
        })
    }
}
impl std::error::Error for EgressStoreFailure {}
impl EgressStoreFailure {
    pub(crate) fn admin_error(
        error: &anyhow::Error,
    ) -> crate::control::egress_policy::EgressPolicyAdminErrorV1 {
        use crate::control::egress_policy::EgressPolicyAdminErrorV1 as E;
        if let Some(kind) = error.downcast_ref::<Self>() {
            return match kind {
                Self::Invalid => E::InvalidRequest,
                Self::Conflict => E::Conflict,
                Self::ScopeConflict => E::ScopeConflict,
                Self::MigrationRequired => E::MigrationRequired,
                Self::Capacity | Self::AuditReservationExhausted => E::Capacity,
                Self::DiscoveryStale | Self::Integrity | Self::Permission => E::Unavailable,
            };
        }
        if error.downcast_ref::<review_access::ReviewAccessFailure>()
            == Some(&review_access::ReviewAccessFailure::Busy)
            || error.downcast_ref::<crate::human_authority::HumanAuthorityFailure>()
                == Some(&crate::human_authority::HumanAuthorityFailure::GateBusy)
        {
            return E::Busy;
        }
        if error.downcast_ref::<crate::workload::WorkloadFailureV1>()
            == Some(&crate::workload::WorkloadFailureV1::ClockRegression)
        {
            return E::Clock;
        }
        if error
            .downcast_ref::<capacity::AuditCapacityFailureV1>()
            .is_some()
        {
            return E::Capacity;
        }
        E::Unavailable
    }
}
pub(super) fn ensure(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(EgressStoreFailure::Integrity.into())
    }
}
pub(super) fn valid(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(EgressStoreFailure::Invalid.into())
    }
}
pub(super) fn cas(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(EgressStoreFailure::Conflict.into())
    }
}
pub(super) fn canonical_id(value: &str) -> anyhow::Result<()> {
    valid(uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value))
}
pub(super) fn bounded_number(value: u64, zero: bool) -> anyhow::Result<()> {
    valid((zero || value > 0) && value <= i64::MAX as u64)
}
pub(super) fn bounded_json<T: Serialize>(value: &T, bound: usize) -> anyhow::Result<String> {
    let json = serde_json::to_string(value)?;
    valid(json.len() <= bound)?;
    Ok(json)
}
pub(super) fn content_digest<T: Serialize>(domain: &str, value: &T) -> anyhow::Result<String> {
    crate::control::content_hash(domain, value)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EgressStoreLimitsV1 {
    version: u32,
    active_heads: u32,
    retained_entries: u32,
    snapshot_bytes: u64,
}
impl Default for EgressStoreLimitsV1 {
    fn default() -> Self {
        Self {
            version: 1,
            active_heads: 128,
            retained_entries: 16_384,
            snapshot_bytes: 16 * 1_048_576,
        }
    }
}
impl EgressStoreLimitsV1 {
    pub(crate) fn new(
        active_heads: u32,
        retained_entries: u32,
        snapshot_bytes: u64,
    ) -> anyhow::Result<Self> {
        let result = Self {
            version: 1,
            active_heads,
            retained_entries,
            snapshot_bytes,
        };
        result.validate()?;
        Ok(result)
    }
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        valid(
            self.version == 1
                && (1..=4096).contains(&self.active_heads)
                && (1..=65_536).contains(&self.retained_entries)
                && (1..=67_108_864).contains(&self.snapshot_bytes),
        )
    }
    pub(super) fn admits(
        &self,
        operations: usize,
        revisions: usize,
        active: usize,
        bytes: usize,
    ) -> anyhow::Result<()> {
        let entries = operations
            .checked_add(revisions)
            .and_then(|n| n.checked_add(active))
            .ok_or(EgressStoreFailure::Capacity)?;
        if active <= self.active_heads as usize
            && entries <= self.retained_entries as usize
            && u64::try_from(bytes)? <= self.snapshot_bytes
        {
            Ok(())
        } else {
            Err(EgressStoreFailure::Capacity.into())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "session_id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum EgressPolicySelectorV1 {
    Tenant,
    Session(String),
}
impl EgressPolicySelectorV1 {
    pub fn session(id: &str) -> anyhow::Result<Self> {
        canonical_id(id)?;
        Ok(Self::Session(id.into()))
    }
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Tenant => Ok(()),
            Self::Session(id) => canonical_id(id),
        }
    }
    pub(super) fn sql(&self) -> (&'static str, &str) {
        match self {
            Self::Tenant => ("tenant", ""),
            Self::Session(id) => ("session", id),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressPolicyStateV1 {
    Active,
    RevokeRequested,
}
impl EgressPolicyStateV1 {
    pub(super) fn sql(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::RevokeRequested => "revoke_requested",
        }
    }
}

/// Immutable requests contain data only. No Deserialize, tenant, actor, clock,
/// per-request Store limits or reserve consumption switch is accepted.
pub(crate) struct EgressReplacePolicyV1 {
    operation_id: uuid::Uuid,
    selector: EgressPolicySelectorV1,
    expected_revision: u64,
    expected_epoch: u64,
    policy: EgressPolicyV1,
    tenant_ceiling: Option<Ceiling>,
}
impl EgressReplacePolicyV1 {
    pub(crate) fn new(
        operation_id: uuid::Uuid,
        selector: EgressPolicySelectorV1,
        expected_revision: u64,
        expected_epoch: u64,
        policy: EgressPolicyV1,
        tenant_ceiling: Option<Ceiling>,
    ) -> anyhow::Result<Self> {
        let result = Self {
            operation_id,
            selector,
            expected_revision,
            expected_epoch,
            policy,
            tenant_ceiling,
        };
        result.change()?.validate()?;
        Ok(result)
    }
    pub(crate) fn operation_id(&self) -> uuid::Uuid {
        self.operation_id
    }
    pub(super) fn change(&self) -> anyhow::Result<Change> {
        Ok(Change::Replace {
            operation_id: self.operation_id,
            selector: self.selector.clone(),
            expected_revision: self.expected_revision,
            expected_epoch: self.expected_epoch,
            policy: WirePolicy::from_core(&self.policy)?,
            tenant_ceiling: self.tenant_ceiling.clone(),
        })
    }
}
pub(crate) struct EgressRevokePolicyV1 {
    operation_id: uuid::Uuid,
    selector: EgressPolicySelectorV1,
    expected_revision: u64,
    expected_epoch: u64,
}
impl EgressRevokePolicyV1 {
    pub(crate) fn new(
        operation_id: uuid::Uuid,
        selector: EgressPolicySelectorV1,
        expected_revision: u64,
        expected_epoch: u64,
    ) -> anyhow::Result<Self> {
        let result = Self {
            operation_id,
            selector,
            expected_revision,
            expected_epoch,
        };
        result.change().validate()?;
        Ok(result)
    }
    pub(super) fn change(&self) -> Change {
        Change::Revoke {
            operation_id: self.operation_id,
            selector: self.selector.clone(),
            expected_revision: self.expected_revision,
            expected_epoch: self.expected_epoch,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WireDestination {
    name: String,
    port: u16,
    transport: WireTransport,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireTransport {
    ConnectTcpOpaque,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WirePolicy {
    revision: u64,
    not_before_ms: u64,
    expires_at_ms: u64,
    destinations: Vec<WireDestination>,
    max_destinations: u16,
    max_resolution_ips: u8,
    digest: String,
}
impl WirePolicy {
    fn from_core(policy: &EgressPolicyV1) -> anyhow::Result<Self> {
        valid(
            policy.destinations().len() <= 128
                && policy.destinations().iter().all(|d| d.name().len() <= 253),
        )?;
        let result = Self {
            revision: policy.revision(),
            not_before_ms: policy.not_before_ms(),
            expires_at_ms: policy.expires_at_ms(),
            destinations: policy
                .destinations()
                .iter()
                .map(|d| WireDestination {
                    name: d.name().into(),
                    port: d.port(),
                    transport: WireTransport::ConnectTcpOpaque,
                })
                .collect(),
            max_destinations: u16::try_from(policy.limits().max_destinations())?,
            max_resolution_ips: u8::try_from(policy.limits().max_resolution_ips())?,
            digest: hex::encode(policy.digest()),
        };
        result.core()?;
        bounded_json(&result, POLICY_BYTES)?;
        Ok(result)
    }
    pub(super) fn core(&self) -> anyhow::Result<EgressPolicyV1> {
        bounded_number(self.revision, false)?;
        bounded_number(self.not_before_ms, true)?;
        bounded_number(self.expires_at_ms, false)?;
        valid(
            self.destinations.len() <= 128 && self.destinations.iter().all(|d| d.name.len() <= 253),
        )?;
        let limits = PolicyLimits::new(
            self.max_destinations as usize,
            self.max_resolution_ips as usize,
        )?;
        let destinations = self
            .destinations
            .iter()
            .map(|d| {
                Authority::new(&d.name, d.port, EgressTransport::ConnectTcpOpaque)
                    .map_err(Into::into)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let policy = EgressPolicyV1::new(
            self.revision,
            self.not_before_ms,
            self.expires_at_ms,
            &destinations,
            limits,
        )?;
        ensure(
            hex::encode(policy.digest()) == self.digest
                && policy.destinations() == destinations.as_slice(),
        )?;
        Ok(policy)
    }
    pub(super) fn active_at(&self, at: u64) -> bool {
        self.not_before_ms <= at && at < self.expires_at_ms
    }
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }
    pub(super) fn digest(&self) -> &str {
        &self.digest
    }
    pub(super) fn within(&self, tenant: &Self) -> anyhow::Result<()> {
        cas(self.not_before_ms >= tenant.not_before_ms
            && self.expires_at_ms <= tenant.expires_at_ms
            && self.max_destinations <= tenant.max_destinations
            && self.max_resolution_ips <= tenant.max_resolution_ips
            && self
                .destinations
                .iter()
                .all(|d| tenant.destinations.contains(d)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Change {
    Replace {
        operation_id: uuid::Uuid,
        selector: EgressPolicySelectorV1,
        expected_revision: u64,
        expected_epoch: u64,
        policy: WirePolicy,
        tenant_ceiling: Option<Ceiling>,
    },
    Revoke {
        operation_id: uuid::Uuid,
        selector: EgressPolicySelectorV1,
        expected_revision: u64,
        expected_epoch: u64,
    },
}
impl Change {
    pub(super) fn id(&self) -> uuid::Uuid {
        match self {
            Self::Replace { operation_id, .. } | Self::Revoke { operation_id, .. } => *operation_id,
        }
    }
    pub(super) fn selector(&self) -> &EgressPolicySelectorV1 {
        match self {
            Self::Replace { selector, .. } | Self::Revoke { selector, .. } => selector,
        }
    }
    pub(super) fn expected(&self) -> (u64, u64) {
        match self {
            Self::Replace {
                expected_revision,
                expected_epoch,
                ..
            }
            | Self::Revoke {
                expected_revision,
                expected_epoch,
                ..
            } => (*expected_revision, *expected_epoch),
        }
    }
    pub(super) fn action(&self) -> &'static str {
        match self {
            Self::Replace { .. } => "replace",
            Self::Revoke { .. } => "revoke",
        }
    }
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Replace { .. } => REPLACE_KIND,
            Self::Revoke { .. } => REVOKE_KIND,
        }
    }
    pub(super) fn validate(&self) -> anyhow::Result<()> {
        valid(!self.id().is_nil())?;
        self.selector().validate()?;
        let (r, e) = self.expected();
        bounded_number(r, true)?;
        bounded_number(e, true)?;
        valid((r == 0) == (e == 0))?;
        if let Self::Replace {
            policy,
            selector,
            tenant_ceiling,
            ..
        } = self
        {
            policy.core()?;
            valid(r.checked_add(1) == Some(policy.revision))?;
            valid(matches!(
                (selector, tenant_ceiling),
                (EgressPolicySelectorV1::Tenant, None)
                    | (EgressPolicySelectorV1::Session(_), Some(_))
            ))?;
            if let Some(ceiling) = tenant_ceiling {
                ceiling.validate()?;
            }
        } else {
            valid(r > 0)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MachineActor {
    pub(super) kind: MachineActorKind,
    pub(super) source_id: uuid::Uuid,
    pub(super) credential_id: uuid::Uuid,
    pub(super) credential_generation: u64,
    pub(super) binding_id: String,
    pub(super) binding_revision: u64,
    pub(super) binding_digest: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MachineActorKind {
    MachineControl,
}
impl MachineActor {
    pub(super) fn validate(&self) -> anyhow::Result<()> {
        valid(!self.source_id.is_nil() && !self.credential_id.is_nil())?;
        canonical_id(&self.binding_id)?;
        bounded_number(self.credential_generation, false)?;
        bounded_number(self.binding_revision, false)?;
        valid(
            self.binding_digest.len() == 64
                && self
                    .binding_digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ceiling {
    pub(super) revision: u64,
    pub(super) epoch: u64,
    pub(super) digest: String,
}
impl Ceiling {
    pub(crate) fn new(revision: u64, epoch: u64, digest: [u8; 32]) -> anyhow::Result<Self> {
        let result = Self {
            revision,
            epoch,
            digest: hex::encode(digest),
        };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> anyhow::Result<()> {
        bounded_number(self.revision, false)?;
        bounded_number(self.epoch, false)?;
        valid(
            self.digest.len() == 64
                && self
                    .digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        )
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MutationRecord {
    pub(super) version: u32,
    pub(super) tenant_id: String,
    pub(super) actor: MachineActor,
    pub(super) input_hash: String,
    pub(super) change: Change,
    pub(super) tenant_ceiling: Option<Ceiling>,
    pub(super) after_epoch: u64,
}
impl MutationRecord {
    pub(super) fn input_hash(
        tenant: &str,
        actor: &MachineActor,
        change: &Change,
    ) -> anyhow::Result<String> {
        content_digest("ai-guard:egress-policy-input:v1", &(tenant, actor, change))
    }
    pub(super) fn validate(&self, tenant: &str) -> anyhow::Result<()> {
        ensure(self.version == 1 && self.tenant_id == tenant)?;
        self.actor.validate()?;
        self.change.validate()?;
        ensure(
            self.input_hash == Self::input_hash(tenant, &self.actor, &self.change)?
                && self.change.expected().1.checked_add(1) == Some(self.after_epoch),
        )?;
        bounded_number(self.after_epoch, false)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Genesis {
    pub(super) version: u32,
    pub(super) from_format: u32,
    pub(super) format: u32,
    pub(super) migration_id: uuid::Uuid,
    pub(super) tenant_id: String,
    pub(super) source_id: uuid::Uuid,
    pub(super) namespace: uuid::Uuid,
    pub(super) source_anchor: review_authority::AuthorityReceiptV1,
    pub(super) previous_seq: i64,
    pub(super) previous_hash: String,
    pub(super) limits: EgressStoreLimitsV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressPolicyReceiptV1 {
    version: u32,
    operation_id: uuid::Uuid,
    action: String,
    selector: EgressPolicySelectorV1,
    revision: u64,
    policy_digest: String,
    before_epoch: u64,
    after_epoch: u64,
    state: EgressPolicyStateV1,
    event_id: String,
    event_seq: i64,
    event_hash: String,
    received_at: String,
}
impl EgressPolicyReceiptV1 {
    pub fn state(&self) -> EgressPolicyStateV1 {
        self.state
    }
    pub fn before_epoch(&self) -> u64 {
        self.before_epoch
    }
    pub fn after_epoch(&self) -> u64 {
        self.after_epoch
    }
    pub fn canonical_bytes(&self) -> anyhow::Result<Vec<u8>> {
        Ok(bounded_json(self, RECEIPT_BYTES)?.into_bytes())
    }
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn operation_id(&self) -> uuid::Uuid {
        self.operation_id
    }
    pub fn action(&self) -> &str {
        &self.action
    }
    pub fn selector(&self) -> &EgressPolicySelectorV1 {
        &self.selector
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn policy_digest(&self) -> &str {
        &self.policy_digest
    }
    pub fn event_id(&self) -> &str {
        &self.event_id
    }
    pub fn event_seq(&self) -> i64 {
        self.event_seq
    }
    pub fn event_hash(&self) -> &str {
        &self.event_hash
    }
    pub fn received_at(&self) -> &str {
        &self.received_at
    }
    pub(super) fn from_event(
        stored: &StoredEvent,
        record: &MutationRecord,
        policy: &WirePolicy,
    ) -> Self {
        Self {
            version: 1,
            operation_id: record.change.id(),
            action: record.change.action().into(),
            selector: record.change.selector().clone(),
            revision: policy.revision,
            policy_digest: policy.digest.clone(),
            before_epoch: record.change.expected().1,
            after_epoch: record.after_epoch,
            state: match record.change {
                Change::Replace { .. } => EgressPolicyStateV1::Active,
                Change::Revoke { .. } => EgressPolicyStateV1::RevokeRequested,
            },
            event_id: stored.event.id.clone(),
            event_seq: stored.seq,
            event_hash: stored.hash.clone(),
            received_at: stored.received_at.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EgressPolicySnapshotV1 {
    pub(super) selector: EgressPolicySelectorV1,
    pub(super) policy: EgressPolicyV1,
    pub(super) epoch: u64,
    pub(super) state: EgressPolicyStateV1,
}
impl EgressPolicySnapshotV1 {
    pub(crate) fn policy(&self) -> &EgressPolicyV1 {
        &self.policy
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }
    pub(crate) fn state(&self) -> EgressPolicyStateV1 {
        self.state
    }
}
