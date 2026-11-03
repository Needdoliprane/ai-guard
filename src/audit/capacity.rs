//! Logical admission over the existing, durable audit chain.
use super::*;
use crate::identity::PrincipalRefV1;
use std::collections::HashMap;

pub(super) const PHYSICAL_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditCapacityV1 {
    pub events: u64,
    pub bytes: u64,
}
impl AuditCapacityV1 {
    pub(super) fn add(self, other: Self) -> anyhow::Result<Self> {
        Ok(Self {
            events: self
                .events
                .checked_add(other.events)
                .context("audit event usage overflow")?,
            bytes: self
                .bytes
                .checked_add(other.bytes)
                .context("audit byte usage overflow")?,
        })
    }
    fn subtract(self, other: Self) -> anyhow::Result<Self> {
        Ok(Self {
            events: self
                .events
                .checked_sub(other.events)
                .context("invalid event reserve")?,
            bytes: self
                .bytes
                .checked_sub(other.bytes)
                .context("invalid byte reserve")?,
        })
    }
    fn within(self, limit: Self) -> bool {
        self.events <= limit.events && self.bytes <= limit.bytes
    }
    fn positive(self) -> bool {
        self.events > 0 && self.bytes > 0
    }
    fn fraction(self, divisor: u64) -> Self {
        Self {
            events: self.events / divisor,
            bytes: self.bytes / divisor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditCapacityPolicyV1 {
    pub version: u32,
    pub human_tenant: AuditCapacityV1,
    pub human_principal: AuditCapacityV1,
    pub control_reserve: AuditCapacityV1,
    pub identity_reserve: AuditCapacityV1,
}
impl AuditCapacityPolicyV1 {
    pub(super) fn resolved(max_events: u64) -> Self {
        let physical = AuditCapacityV1 {
            events: max_events,
            bytes: PHYSICAL_BYTES,
        };
        Self {
            version: 1,
            human_tenant: physical.fraction(2),
            human_principal: physical.fraction(8),
            control_reserve: physical.fraction(4),
            identity_reserve: physical.fraction(8),
        }
    }
    /// Explicit policies must leave both reserves and a separate reader share.
    /// Small-capacity automatic defaults are resolved internally, not accepted
    /// as an explicit policy that silently disables human access.
    pub fn validate(&self, max_events: u64) -> anyhow::Result<()> {
        anyhow::ensure!(max_events <= i64::MAX as u64, "invalid audit capacity");
        anyhow::ensure!(
            self.version == 1,
            "unsupported audit capacity policy version"
        );
        let physical = AuditCapacityV1 {
            events: max_events,
            bytes: PHYSICAL_BYTES,
        };
        anyhow::ensure!(
            self.control_reserve.positive()
                && self.identity_reserve.positive()
                && self.human_tenant.positive()
                && self.human_principal.positive(),
            "audit capacity budgets and reserves must be positive"
        );
        let available = physical.subtract(self.control_reserve.add(self.identity_reserve)?)?;
        anyhow::ensure!(
            self.human_tenant.within(available)
                && self.human_principal.events < self.human_tenant.events
                && self.human_principal.bytes < self.human_tenant.bytes,
            "audit capacity policy must preserve reserves and another reader's share"
        );
        Ok(())
    }
    pub fn fingerprint(&self) -> anyhow::Result<String> {
        let mut digest = Sha256::new();
        digest.update(b"agentsandbox:audit-capacity:v1\0");
        digest.update(serde_json::to_vec(self)?);
        Ok(hex::encode(digest.finalize()))
    }
    pub fn human_reads_enabled(&self) -> bool {
        self.control_reserve.positive()
            && self.identity_reserve.positive()
            && self.human_tenant.positive()
            && self.human_principal.positive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditCapacityFailureV1 {
    HumanReadBudgetExhausted,
    OrdinaryWriteBudgetExhausted,
    HumanReadDisabled,
}
impl std::fmt::Display for AuditCapacityFailureV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::HumanReadBudgetExhausted => {
                "cumulative human read audit budget exhausted; operator action required"
            }
            Self::OrdinaryWriteBudgetExhausted => {
                "audit capacity reserved for identity maintenance; operator action required"
            }
            Self::HumanReadDisabled => {
                "human read audit disabled: insufficient capacity for protected budgets"
            }
        })
    }
}
impl std::error::Error for AuditCapacityFailureV1 {}

// No event field, request DTO or external label can construct these classes.
pub(super) enum Class<'a> {
    HumanRead(&'a PrincipalRefV1),
    OrdinaryWrite,
    IdentityMaintenance,
}

#[derive(Default)]
pub(super) struct Cache {
    head: Option<(i64, String)>,
    namespace: Option<String>,
    pub(super) tenant: AuditCapacityV1,
    pub(super) principals: HashMap<String, AuditCapacityV1>,
    #[cfg(test)]
    pub(super) scanned: u64,
}
pub(super) struct Prepared {
    pub(super) head: (i64, String),
    pub(super) namespace: Option<String>,
    pub(super) tenant: AuditCapacityV1,
    pub(super) principals: HashMap<String, AuditCapacityV1>,
}
impl Cache {
    /// Called only after the Store's existing validation, on its stable
    /// transaction snapshot. The suffix may include uncommitted appends, so
    /// its counters stay separate until commit/backing checks have succeeded.
    pub(super) fn prepare(&mut self, db: &Connection) -> anyhow::Result<Prepared> {
        let (after, expected) = self.head.clone().unwrap_or((0, "0".repeat(64)));
        let actual = if after == 0 {
            "0".repeat(64)
        } else {
            db.query_row("SELECT hash FROM events WHERE seq=?1", [after], |r| {
                r.get::<_, String>(0)
            })?
        };
        anyhow::ensure!(actual == expected, "audit capacity prefix diverged");
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let namespace: Option<String> = if version >= 2 {
            Some(db.query_row(
                "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
                [],
                |r| r.get(0),
            )?)
        } else {
            None
        };
        anyhow::ensure!(
            self.namespace.is_none() || self.namespace == namespace,
            "audit capacity namespace diverged"
        );
        let mut prepared = Prepared {
            head: (after, expected),
            namespace,
            tenant: self.tenant,
            principals: HashMap::new(),
        };
        let mut statement =
            db.prepare("SELECT seq,hash,kind,payload FROM events WHERE seq>?1 ORDER BY seq")?;
        let mut rows = statement.query([after])?;
        while let Some(row) = rows.next()? {
            #[cfg(test)]
            {
                self.scanned += 1;
            }
            let seq: i64 = row.get(0)?;
            anyhow::ensure!(
                seq == prepared
                    .head
                    .0
                    .checked_add(1)
                    .context("audit sequence overflow")?,
                "audit capacity suffix is not contiguous"
            );
            prepared.head = (seq, row.get(1)?);
            if row.get::<_, String>(2)? != "control.identity.access" {
                continue;
            }
            let payload: String = row.get(3)?;
            let event: Event = serde_json::from_str(&payload)?;
            let record: human::HumanReadAuditV1 = serde_json::from_value(event.data)?;
            human::validate_read(&record)?;
            let key = record.principal.key()?;
            let cost = AuditCapacityV1 {
                events: 1,
                bytes: u64::try_from(payload.len())?,
            };
            prepared.tenant = prepared.tenant.add(cost)?;
            let prior = prepared
                .principals
                .get(&key)
                .copied()
                .or_else(|| self.principals.get(&key).copied())
                .unwrap_or_default();
            prepared.principals.insert(key, prior.add(cost)?);
        }
        anyhow::ensure!(
            prepared.head == audit_head(db)?,
            "audit capacity suffix head diverged"
        );
        // All map allocations needed by publication happen before commit.
        self.principals.try_reserve(prepared.principals.len())?;
        Ok(prepared)
    }
    pub(super) fn publish(&mut self, prepared: Prepared) {
        self.head = Some(prepared.head);
        self.namespace = prepared.namespace;
        self.tenant = prepared.tenant;
        self.principals.extend(prepared.principals);
    }
    pub(super) fn admit(
        &self,
        prepared: &Prepared,
        policy: &AuditCapacityPolicyV1,
        physical: AuditCapacityV1,
        total: AuditCapacityV1,
        cost: AuditCapacityV1,
        class: Class<'_>,
    ) -> anyhow::Result<()> {
        let next = total.add(cost)?;
        match class {
            Class::IdentityMaintenance => Ok(()),
            Class::OrdinaryWrite => {
                anyhow::ensure!(
                    next.within(physical.subtract(policy.identity_reserve)?),
                    AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted
                );
                Ok(())
            }
            Class::HumanRead(principal) => {
                anyhow::ensure!(
                    policy.human_reads_enabled(),
                    AuditCapacityFailureV1::HumanReadDisabled
                );
                let key = principal.key()?;
                let used = prepared
                    .principals
                    .get(&key)
                    .copied()
                    .or_else(|| self.principals.get(&key).copied())
                    .unwrap_or_default();
                let limit =
                    physical.subtract(policy.control_reserve.add(policy.identity_reserve)?)?;
                anyhow::ensure!(
                    next.within(limit)
                        && prepared.tenant.add(cost)?.within(policy.human_tenant)
                        && used.add(cost)?.within(policy.human_principal),
                    AuditCapacityFailureV1::HumanReadBudgetExhausted
                );
                Ok(())
            }
        }
    }
}

impl Store {
    /// Select an immutable operator policy before creating any sibling or Arc.
    pub fn with_capacity_policy(mut self, policy: AuditCapacityPolicyV1) -> anyhow::Result<Self> {
        policy.validate(self.max_events)?;
        anyhow::ensure!(
            Arc::strong_count(&self.supervision_lease) == 1,
            "audit capacity policy must be selected before sharing the Store"
        );
        self.capacity_policy = policy;
        Ok(self)
    }
    pub fn capacity_policy(&self) -> &AuditCapacityPolicyV1 {
        &self.capacity_policy
    }
    pub fn capacity_policy_fingerprint(&self) -> anyhow::Result<String> {
        self.capacity_policy.fingerprint()
    }
    pub fn load_capacity_policy(path: &Path) -> anyhow::Result<AuditCapacityPolicyV1> {
        let path = crate::control_fs::absolute_path(path)?;
        let held = crate::control_fs::PrivateFile::open(&path, 16_384)?;
        let policy =
            serde_json::from_slice(held.bytes()).context("invalid audit capacity policy file")?;
        held.revalidate()?;
        Ok(policy)
    }
}
