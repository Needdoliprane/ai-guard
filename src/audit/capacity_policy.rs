//! Capacity policy shared by the SQLite and MySQL audit writers.
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(crate) const PHYSICAL_BYTES: u64 = 256 * 1024 * 1024;
/// Largest payload cap that an operator may select for a SQLite tenant store.
/// A larger store lengthens every full validation at open and after an
/// external change; MySQL keeps `PHYSICAL_BYTES` until its verification is
/// incremental.
pub const MAX_SQLITE_PHYSICAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Default and smallest selectable payload cap of a tenant store.
pub const DEFAULT_PHYSICAL_BYTES: u64 = PHYSICAL_BYTES;
/// CLI default for the gateway and its Legacy session maintenance commands.
pub const DEFAULT_GATEWAY_AUDIT_MAX_EVENTS_ARG: &str = "100000";

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
    pub(super) fn subtract(self, other: Self) -> anyhow::Result<Self> {
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
    pub(super) fn within(self, limit: Self) -> bool {
        self.events <= limit.events && self.bytes <= limit.bytes
    }
    pub(super) fn positive(self) -> bool {
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
    /// Automatic defaults, including disabled reads for very small capacities.
    /// An explicit operator policy must separately pass `validate`.
    pub fn resolved(max_events: u64) -> Self {
        Self::resolved_for(max_events, PHYSICAL_BYTES)
    }
    /// Defaults for an explicitly selected payload cap.
    pub fn resolved_for(max_events: u64, physical_bytes: u64) -> Self {
        let physical = AuditCapacityV1 {
            events: max_events,
            bytes: physical_bytes,
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
        self.validate_for(max_events, PHYSICAL_BYTES)
    }
    /// `validate` against an explicitly selected payload cap.
    pub fn validate_for(&self, max_events: u64, physical_bytes: u64) -> anyhow::Result<()> {
        anyhow::ensure!(max_events <= i64::MAX as u64, "invalid audit capacity");
        anyhow::ensure!(
            self.version == 1,
            "unsupported audit capacity policy version"
        );
        let physical = AuditCapacityV1 {
            events: max_events,
            bytes: physical_bytes,
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
    WorkloadAuditBudgetExhausted,
    WorkloadAuditDisabled,
}
impl std::fmt::Display for AuditCapacityFailureV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::HumanReadBudgetExhausted => {
                "cumulative human read audit budget exhausted; operator action required"
            }
            Self::OrdinaryWriteBudgetExhausted => {
                "audit capacity protected by control or identity reserves; operator action required"
            }
            Self::HumanReadDisabled => {
                "human read audit disabled: insufficient capacity for protected budgets"
            }
            Self::WorkloadAuditBudgetExhausted => "cumulative workload audit budget exhausted; control and identity reserves remain protected",
            Self::WorkloadAuditDisabled => "workload audit disabled: insufficient capacity for protected budgets",
        })
    }
}
impl std::error::Error for AuditCapacityFailureV1 {}

/// The write path selects the class after validating the typed operation.
pub(crate) enum ReservedWrite {
    Ordinary,
    Control,
    #[cfg(feature = "mysql-storage")]
    IdentityMaintenance,
}

impl AuditCapacityPolicyV1 {
    pub(crate) fn admit_write(
        &self,
        physical: AuditCapacityV1,
        next: AuditCapacityV1,
        class: ReservedWrite,
    ) -> anyhow::Result<()> {
        let reserves = match class {
            ReservedWrite::Ordinary => self.control_reserve.add(self.identity_reserve)?,
            ReservedWrite::Control => self.identity_reserve,
            #[cfg(feature = "mysql-storage")]
            ReservedWrite::IdentityMaintenance => AuditCapacityV1::default(),
        };
        anyhow::ensure!(
            next.within(physical.subtract(reserves)?),
            AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted
        );
        Ok(())
    }
}
