//! Logical admission over the existing, durable audit chain.
use super::capacity_policy::ReservedWrite;
pub(super) use super::capacity_policy::{AuditCapacityFailureV1, PHYSICAL_BYTES};
use super::*;
use crate::identity::PrincipalRefV1;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadAuditPolicyV1 {
    pub version: u32,
    pub tenant: AuditCapacityV1,
    pub principal: AuditCapacityV1,
}
impl WorkloadAuditPolicyV1 {
    /// Automatic defaults fit the operator's existing C+I reserves. A very
    /// small remainder disables workload admission without invalidating F1.
    pub fn resolved(max_events: u64, control: &AuditCapacityPolicyV1) -> Self {
        Self::resolved_for(max_events, PHYSICAL_BYTES, control)
    }
    pub fn resolved_for(
        max_events: u64,
        physical_bytes: u64,
        control: &AuditCapacityPolicyV1,
    ) -> Self {
        let physical = AuditCapacityV1 {
            events: max_events,
            bytes: physical_bytes,
        };
        let available = AuditCapacityV1 {
            events: physical.events.saturating_sub(
                control
                    .control_reserve
                    .events
                    .saturating_add(control.identity_reserve.events),
            ),
            bytes: physical.bytes.saturating_sub(
                control
                    .control_reserve
                    .bytes
                    .saturating_add(control.identity_reserve.bytes),
            ),
        };
        let tenant = AuditCapacityV1 {
            events: (physical.events / 4).min(available.events),
            bytes: (physical.bytes / 4).min(available.bytes),
        };
        let principal = AuditCapacityV1 {
            events: (physical.events / 16).min(tenant.events / 2),
            bytes: (physical.bytes / 16).min(tenant.bytes / 2),
        };
        let result = Self {
            version: 1,
            tenant,
            principal,
        };
        if result.enabled()
            && control.control_reserve.positive()
            && control.identity_reserve.positive()
        {
            result
        } else {
            Self {
                version: 1,
                tenant: AuditCapacityV1::default(),
                principal: AuditCapacityV1::default(),
            }
        }
    }
    pub fn enabled(&self) -> bool {
        self.tenant.positive()
            && self.principal.positive()
            && self.principal.events < self.tenant.events
            && self.principal.bytes < self.tenant.bytes
    }
    pub fn validate(&self, max_events: u64, control: &AuditCapacityPolicyV1) -> anyhow::Result<()> {
        self.validate_for(max_events, PHYSICAL_BYTES, control)
    }
    pub fn validate_for(
        &self,
        max_events: u64,
        physical_bytes: u64,
        control: &AuditCapacityPolicyV1,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            max_events <= i64::MAX as u64 && self.version == 1 && self.enabled(),
            "invalid workload audit policy version or budgets"
        );
        let physical = AuditCapacityV1 {
            events: max_events,
            bytes: physical_bytes,
        };
        let available =
            physical.subtract(control.control_reserve.add(control.identity_reserve)?)?;
        anyhow::ensure!(
            control.control_reserve.positive()
                && control.identity_reserve.positive()
                && self.tenant.within(available),
            "workload audit policy must preserve control and identity reserves and another principal's share"
        );
        Ok(())
    }
    pub fn fingerprint(&self) -> anyhow::Result<String> {
        let mut digest = Sha256::new();
        digest.update(b"agentsandbox:workload-audit:v1\0");
        digest.update(serde_json::to_vec(self)?);
        Ok(hex::encode(digest.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WorkloadAuditPrincipalV1 {
    Worker { worker_id: String },
    Broker { broker_id: String },
}
impl WorkloadAuditPrincipalV1 {
    fn key(&self) -> anyhow::Result<String> {
        let (kind, id) = match self {
            Self::Worker { worker_id } => ("worker", worker_id),
            Self::Broker { broker_id } => ("broker", broker_id),
        };
        crate::control::uuid(id)?;
        Ok(format!("{kind}:{id}"))
    }
}
pub(super) fn workload_identity(event: &Event) -> anyhow::Result<Option<WorkloadAuditPrincipalV1>> {
    if let Some(identity) = broker_registry::workload_audit_identity(event)? {
        return Ok(Some(identity));
    }
    if let Some(identity) = relay::workload_audit_identity(event)? {
        return Ok(Some(identity));
    }
    if let Some(identity) = super::kubernetes_admission::workload_audit_identity(event)? {
        return Ok(Some(identity));
    }
    super::kubernetes_session::workload_audit_identity(event)
}

// No event field, request DTO or external label can construct these classes.
pub(super) enum Class<'a> {
    HumanRead(&'a PrincipalRefV1),
    OrdinaryWrite,
    ControlWrite,
    IdentityMaintenance,
    /// Offline retention records, admitted after their own reclaim.
    RetentionMaintenance,
    EgressTerminal(&'a super::egress::TerminalReservation<'a>),
    Workload(WorkloadAuditPrincipalV1),
}

#[derive(Default)]
pub(super) struct Cache {
    head: Option<(i64, String)>,
    namespace: Option<String>,
    pub(super) tenant: AuditCapacityV1,
    pub(super) principals: HashMap<String, AuditCapacityV1>,
    pub(super) workload_tenant: AuditCapacityV1,
    pub(super) workload_principals: HashMap<String, AuditCapacityV1>,
    #[cfg(test)]
    pub(super) scanned: u64,
}
pub(super) struct Prepared {
    pub(super) head: (i64, String),
    pub(super) namespace: Option<String>,
    pub(super) tenant: AuditCapacityV1,
    pub(super) principals: HashMap<String, AuditCapacityV1>,
    pub(super) workload_tenant: AuditCapacityV1,
    pub(super) workload_principals: HashMap<String, AuditCapacityV1>,
}
impl Cache {
    /// Called only after the Store's existing validation, on its stable
    /// transaction snapshot. The suffix may include uncommitted appends, so
    /// its counters stay separate until commit/backing checks have succeeded.
    pub(super) fn prepare(&mut self, db: &Connection) -> anyhow::Result<Prepared> {
        // A retained store starts its budgets at the last retention boundary:
        // the closed prefix below it is sparse and no longer counted.
        let (after, expected) = match self.head.clone() {
            Some(head) => head,
            None => super::retention::ledger::base(db)?,
        };
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
            workload_tenant: self.workload_tenant,
            workload_principals: HashMap::new(),
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
            let kind: String = row.get(2)?;
            if !human_brokers::is_human_read(&kind)
                && !kind.starts_with("control.broker_registry.")
                && !kind.starts_with("control.relay.")
                && !kind.starts_with("control.kubernetes.create.")
                && !kind.starts_with("control.kubernetes.session.")
            {
                continue;
            }
            let payload: String = row.get(3)?;
            let event: Event = serde_json::from_str(&payload)?;
            let cost = AuditCapacityV1 {
                events: 1,
                bytes: u64::try_from(payload.len())?,
            };
            if !human_brokers::is_human_read(&kind) {
                if let Some(identity) = workload_identity(&event)? {
                    let key = identity.key()?;
                    prepared.workload_tenant = prepared.workload_tenant.add(cost)?;
                    let prior = prepared
                        .workload_principals
                        .get(&key)
                        .copied()
                        .or_else(|| self.workload_principals.get(&key).copied())
                        .unwrap_or_default();
                    prepared.workload_principals.insert(key, prior.add(cost)?);
                }
                continue;
            }
            let key = human_brokers::read_principal(&event)?.key()?;
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
        self.workload_principals
            .try_reserve(prepared.workload_principals.len())?;
        Ok(prepared)
    }
    pub(super) fn publish(&mut self, prepared: Prepared) {
        self.head = Some(prepared.head);
        self.namespace = prepared.namespace;
        self.tenant = prepared.tenant;
        self.principals.extend(prepared.principals);
        self.workload_tenant = prepared.workload_tenant;
        self.workload_principals
            .extend(prepared.workload_principals);
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
            Class::Workload(_) => {
                anyhow::bail!("workload audit requires its private admission policy")
            }
            Class::IdentityMaintenance | Class::RetentionMaintenance | Class::EgressTerminal(_) => {
                Ok(())
            }
            Class::OrdinaryWrite => policy.admit_write(physical, next, ReservedWrite::Ordinary),
            Class::ControlWrite => policy.admit_write(physical, next, ReservedWrite::Control),
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
    #[allow(clippy::too_many_arguments)]
    pub(super) fn admit_workload(
        &self,
        prepared: &Prepared,
        control: &AuditCapacityPolicyV1,
        policy: &WorkloadAuditPolicyV1,
        physical: AuditCapacityV1,
        total: AuditCapacityV1,
        cost: AuditCapacityV1,
        principal: &WorkloadAuditPrincipalV1,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            policy.enabled(),
            AuditCapacityFailureV1::WorkloadAuditDisabled
        );
        let key = principal.key()?;
        let used = prepared
            .workload_principals
            .get(&key)
            .copied()
            .or_else(|| self.workload_principals.get(&key).copied())
            .unwrap_or_default();
        let limit = physical.subtract(control.control_reserve.add(control.identity_reserve)?)?;
        anyhow::ensure!(
            total.add(cost)?.within(limit)
                && prepared.workload_tenant.add(cost)?.within(policy.tenant)
                && used.add(cost)?.within(policy.principal),
            AuditCapacityFailureV1::WorkloadAuditBudgetExhausted
        );
        Ok(())
    }
}

impl Store {
    /// Select an immutable operator policy before creating any sibling or Arc.
    /// Select the payload cap of this SQLite store, between 256 MiB and 2 GiB,
    /// before any capacity policy and before sharing the Store. Defaults are
    /// resolved again for that cap.
    pub fn with_physical_bytes(mut self, physical_bytes: u64) -> anyhow::Result<Self> {
        anyhow::ensure!(
            (super::capacity_policy::DEFAULT_PHYSICAL_BYTES
                ..=super::capacity_policy::MAX_SQLITE_PHYSICAL_BYTES)
                .contains(&physical_bytes),
            "the audit payload cap must be between 256 MiB and 2 GiB"
        );
        anyhow::ensure!(
            Arc::strong_count(&self.supervision_lease) == 1
                && self.capacity_policy
                    == AuditCapacityPolicyV1::resolved_for(self.max_events, self.physical_bytes)
                && !self.workload_audit_policy_explicit,
            "select the audit payload cap before any capacity policy and before sharing the Store"
        );
        self.physical_bytes = physical_bytes;
        self.capacity_policy = AuditCapacityPolicyV1::resolved_for(self.max_events, physical_bytes);
        self.workload_audit_policy = WorkloadAuditPolicyV1::resolved_for(
            self.max_events,
            physical_bytes,
            &self.capacity_policy,
        );
        Ok(self)
    }
    pub fn physical_bytes(&self) -> u64 {
        self.physical_bytes
    }
    pub fn with_capacity_policy(mut self, policy: AuditCapacityPolicyV1) -> anyhow::Result<Self> {
        policy.validate_for(self.max_events, self.physical_bytes)?;
        anyhow::ensure!(
            Arc::strong_count(&self.supervision_lease) == 1,
            "audit capacity policy must be selected before sharing the Store"
        );
        self.capacity_policy = policy;
        if self.workload_audit_policy_explicit {
            self.workload_audit_policy.validate_for(
                self.max_events,
                self.physical_bytes,
                &self.capacity_policy,
            )?;
        } else {
            self.workload_audit_policy = WorkloadAuditPolicyV1::resolved_for(
                self.max_events,
                self.physical_bytes,
                &self.capacity_policy,
            );
        }
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
    pub fn with_workload_audit_policy(
        mut self,
        policy: WorkloadAuditPolicyV1,
    ) -> anyhow::Result<Self> {
        policy.validate_for(self.max_events, self.physical_bytes, &self.capacity_policy)?;
        anyhow::ensure!(
            Arc::strong_count(&self.supervision_lease) == 1,
            "workload audit policy must be selected before sharing the Store"
        );
        self.workload_audit_policy = policy;
        self.workload_audit_policy_explicit = true;
        Ok(self)
    }
    pub fn workload_audit_policy(&self) -> &WorkloadAuditPolicyV1 {
        &self.workload_audit_policy
    }
    pub fn workload_audit_policy_fingerprint(&self) -> anyhow::Result<String> {
        self.workload_audit_policy.fingerprint()
    }
    pub fn load_workload_audit_policy(path: &Path) -> anyhow::Result<WorkloadAuditPolicyV1> {
        let path = crate::control_fs::absolute_path(path)?;
        let held = crate::control_fs::PrivateFile::open(&path, 16_384)?;
        let policy =
            serde_json::from_slice(held.bytes()).context("invalid workload audit policy file")?;
        held.revalidate()?;
        Ok(policy)
    }
}

#[cfg(test)]
#[path = "../workload/capacity_tests.rs"]
mod workload_tests;
